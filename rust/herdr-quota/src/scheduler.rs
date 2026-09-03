//! Completion-relative quota refresh scheduling.
//!
//! This module owns only the refresh lifecycle. A future collector implements
//! [`RefreshWorker`], while the terminal loop can translate its callbacks into
//! [`crate::app_state`] actions. Keeping those seams narrow avoids coupling
//! timing to provider credentials, command execution, or rendering.

use std::future::Future;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Duration;

use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinSet;
use tokio::time::{Instant, Sleep};

/// The delay after successful collection and post-collection work.
pub const NORMAL_REFRESH: Duration = Duration::from_secs(5 * 60);
/// Bounded delays after consecutive whole-collector failures.
pub const FAILURE_BACKOFF: [Duration; 3] = [
    Duration::from_secs(10 * 60),
    Duration::from_secs(20 * 60),
    Duration::from_secs(30 * 60),
];
/// The renderer-only interval used to age relative timestamps and countdowns.
pub const AGE_TICK: Duration = Duration::from_secs(30);

/// A generation fence passed to work that can finish after a manual refresh or
/// close. It lets persistence publish only state from the current attempt.
#[derive(Clone, Debug)]
pub struct RefreshAttempt {
    generation: u64,
    current_generation: Arc<AtomicU64>,
    closed: Arc<AtomicBool>,
}

impl RefreshAttempt {
    /// Return true only while this attempt can still publish current state.
    pub fn is_current(&self) -> bool {
        !self.closed.load(Ordering::Acquire)
            && self.current_generation.load(Ordering::Acquire) == self.generation
    }
}

/// The collector-facing refresh interface.
///
/// `on_success` is intentionally synchronous: use it for immediately visible
/// live data. `after_success` is serialized across attempts and must complete
/// before the next normal five-minute timer is armed. It receives the same
/// attempt fence for late-result protection.
pub trait RefreshWorker: Send + Sync + 'static {
    type Value: Send + 'static;
    type Error: Send + 'static;

    fn collect(&self) -> impl Future<Output = Result<Self::Value, Self::Error>> + Send;
    fn on_start(&self);
    fn on_success(&self, value: &Self::Value, attempt: &RefreshAttempt);
    fn after_success(
        &self,
        value: Self::Value,
        attempt: RefreshAttempt,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn on_failure(&self, error: &Self::Error);
    fn on_scheduled(&self, delay: Duration, after_failure: bool);
    fn on_settled(&self);
    /// Redraw relative-age text without collecting, retrying, or mutating quota
    /// data. A terminal adapter normally maps this to `AppAction::AgeTick`.
    fn on_age_tick(&self);
    /// Stop the currently running collector process, if any. The scheduler
    /// still fences and ignores a result that arrives after cancellation.
    fn cancel_active(&self);
}

/// A handle for manual refresh and lifecycle cancellation.
#[derive(Clone)]
pub struct RefreshHandle {
    commands: mpsc::UnboundedSender<Command>,
}

impl RefreshHandle {
    /// Preempt active collection and start a fresh attempt immediately.
    pub async fn manual(&self) {
        let (acknowledged, received) = oneshot::channel();
        if self.commands.send(Command::Manual(acknowledged)).is_ok() {
            let _ = received.await;
        }
    }

    /// Cancel timers and active collection. This is idempotent.
    pub async fn close(&self) {
        let (acknowledged, received) = oneshot::channel();
        if self.commands.send(Command::Close(acknowledged)).is_ok() {
            let _ = received.await;
        }
    }
}

/// Starts one immediate refresh loop and returns its control handle.
pub fn open<W>(worker: W) -> RefreshHandle
where
    W: RefreshWorker,
{
    let (commands, command_receiver) = mpsc::unbounded_channel();
    let handle = RefreshHandle { commands };
    tokio::spawn(run(Arc::new(worker), command_receiver));
    handle
}

enum Command {
    Manual(oneshot::Sender<()>),
    Close(oneshot::Sender<()>),
}

enum Event<W: RefreshWorker> {
    Collected {
        generation: u64,
        result: Result<W::Value, W::Error>,
    },
    PostCollectionFinished {
        generation: u64,
        result: Result<(), W::Error>,
    },
}

struct PublicationState {
    generation: u64,
    closed: bool,
}

struct PostCollection<W: RefreshWorker> {
    generation: u64,
    attempt: RefreshAttempt,
    value: W::Value,
}

fn begin_collection<W>(
    worker: Arc<W>,
    events: mpsc::UnboundedSender<Event<W>>,
    tasks: &mut JoinSet<()>,
    generation: &mut u64,
    current_generation: &Arc<AtomicU64>,
) where
    W: RefreshWorker,
{
    *generation = generation.saturating_add(1);
    current_generation.store(*generation, Ordering::Release);
    worker.on_start();
    let worker_for_task = Arc::clone(&worker);
    let sequence = *generation;
    tasks.spawn(async move {
        let result = worker_for_task.collect().await;
        let _ = events.send(Event::Collected {
            generation: sequence,
            result,
        });
    });
}

async fn run_post_collections<W>(
    worker: Arc<W>,
    events: mpsc::UnboundedSender<Event<W>>,
    mut post_collections: mpsc::UnboundedReceiver<PostCollection<W>>,
    publication_state: Arc<Mutex<PublicationState>>,
) where
    W: RefreshWorker,
{
    while let Some(work) = post_collections.recv().await {
        let state = publication_state.lock().await;
        if state.closed || state.generation != work.generation {
            continue;
        }
        let result = worker.after_success(work.value, work.attempt).await;
        drop(state);
        let _ = events.send(Event::PostCollectionFinished {
            generation: work.generation,
            result,
        });
    }
}

fn backoff(failures: usize) -> Duration {
    FAILURE_BACKOFF[failures.saturating_sub(1).min(FAILURE_BACKOFF.len() - 1)]
}

async fn run<W>(worker: Arc<W>, mut commands: mpsc::UnboundedReceiver<Command>)
where
    W: RefreshWorker,
{
    let (event_sender, mut events) = mpsc::unbounded_channel();
    let current_generation = Arc::new(AtomicU64::new(0));
    let closed = Arc::new(AtomicBool::new(false));
    let publication_state = Arc::new(Mutex::new(PublicationState {
        generation: 0,
        closed: false,
    }));
    let (post_collection_sender, post_collection_receiver) = mpsc::unbounded_channel();
    let mut tasks = JoinSet::new();
    let mut generation = 0_u64;
    let mut failures = 0_usize;
    let mut timer: Option<std::pin::Pin<Box<Sleep>>> = None;
    let mut age_timer = Box::pin(tokio::time::sleep(AGE_TICK));

    tasks.spawn(run_post_collections(
        Arc::clone(&worker),
        event_sender.clone(),
        post_collection_receiver,
        Arc::clone(&publication_state),
    ));

    {
        let mut state = publication_state.lock().await;
        begin_collection(
            Arc::clone(&worker),
            event_sender.clone(),
            &mut tasks,
            &mut generation,
            &current_generation,
        );
        state.generation = generation;
    }

    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(Command::Manual(acknowledged)) => {
                    failures = 0;
                    drop(timer.take());
                    worker.cancel_active();
                    let mut state = publication_state.lock().await;
                    begin_collection(
                            Arc::clone(&worker),
                            event_sender.clone(),
                            &mut tasks,
                            &mut generation,
                            &current_generation,
                        );
                    state.generation = generation;
                    let _ = acknowledged.send(());
                }
                Some(Command::Close(acknowledged)) => {
                    tasks.abort_all();
                    let mut state = publication_state.lock().await;
                    state.closed = true;
                    closed.store(true, Ordering::Release);
                    generation = generation.saturating_add(1);
                    state.generation = generation;
                    current_generation.store(generation, Ordering::Release);
                    worker.cancel_active();
                    let _ = acknowledged.send(());
                    return;
                }
                None => {
                    tasks.abort_all();
                    let mut state = publication_state.lock().await;
                    state.closed = true;
                    closed.store(true, Ordering::Release);
                    return;
                }
            },
            event = events.recv() => if let Some(event) = event {
                match event {
                    Event::Collected { generation: result_generation, result } => {
                        if result_generation != generation || closed.load(Ordering::Acquire) {
                            continue;
                        }
                        match result {
                            Ok(value) => {
                                let attempt = RefreshAttempt {
                                    generation: result_generation,
                                    current_generation: Arc::clone(&current_generation),
                                    closed: Arc::clone(&closed),
                                };
                                worker.on_success(&value, &attempt);
                                let _ = post_collection_sender.send(PostCollection {
                                    generation: result_generation,
                                    attempt,
                                    value,
                                });
                            }
                            Err(error) => settle_failure(
                                &*worker,
                                &mut failures,
                                &mut timer,
                                error,
                            ),
                        }
                    }
                    Event::PostCollectionFinished { generation: result_generation, result } => {
                        if result_generation != generation || closed.load(Ordering::Acquire) {
                            continue;
                        }
                        match result {
                            Ok(()) => {
                                failures = 0;
                                timer = Some(Box::pin(tokio::time::sleep(NORMAL_REFRESH)));
                                worker.on_scheduled(NORMAL_REFRESH, false);
                                worker.on_settled();
                            }
                            Err(error) => settle_failure(
                                &*worker,
                                &mut failures,
                                &mut timer,
                                error,
                            ),
                        }
                    }
                }
            },
            _ = async {
                match timer.as_mut() {
                    Some(timer) => timer.await,
                    None => std::future::pending().await,
                }
            } => {
                timer = None;
                let mut state = publication_state.lock().await;
                begin_collection(
                    Arc::clone(&worker),
                    event_sender.clone(),
                    &mut tasks,
                    &mut generation,
                    &current_generation,
                );
                state.generation = generation;
            },
            _ = &mut age_timer => {
                worker.on_age_tick();
                age_timer.as_mut().reset(Instant::now() + AGE_TICK);
            },
            _ = tasks.join_next(), if !tasks.is_empty() => {},
        }
    }
}

fn settle_failure<W>(
    worker: &W,
    failures: &mut usize,
    timer: &mut Option<std::pin::Pin<Box<Sleep>>>,
    error: W::Error,
) where
    W: RefreshWorker,
{
    *failures = failures.saturating_add(1);
    let delay = backoff(*failures);
    worker.on_failure(&error);
    *timer = Some(Box::pin(tokio::time::sleep(delay)));
    worker.on_scheduled(delay, true);
    worker.on_settled();
}

#[cfg(test)]
mod tests {
    use super::{
        AGE_TICK, FAILURE_BACKOFF, NORMAL_REFRESH, PostCollection, PublicationState,
        RefreshAttempt, RefreshWorker, open, run_post_collections,
    };
    use std::collections::VecDeque;
    use std::future::Future;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::sync::oneshot;

    type Outcome = Result<&'static str, &'static str>;
    type PostOutcome = Result<(), &'static str>;

    struct TestWorker {
        collections: Mutex<VecDeque<oneshot::Receiver<Outcome>>>,
        post_collections: Mutex<VecDeque<oneshot::Receiver<Outcome>>>,
        starts: AtomicUsize,
        cancellations: AtomicUsize,
        successes: Mutex<Vec<&'static str>>,
        post_started: Mutex<Vec<&'static str>>,
        failures: Mutex<Vec<&'static str>>,
        scheduled: Mutex<Vec<(Duration, bool)>>,
        settled: AtomicUsize,
        age_ticks: AtomicUsize,
    }

    impl TestWorker {
        fn new() -> Self {
            Self {
                collections: Mutex::new(VecDeque::new()),
                post_collections: Mutex::new(VecDeque::new()),
                starts: AtomicUsize::new(0),
                cancellations: AtomicUsize::new(0),
                successes: Mutex::new(Vec::new()),
                post_started: Mutex::new(Vec::new()),
                failures: Mutex::new(Vec::new()),
                scheduled: Mutex::new(Vec::new()),
                settled: AtomicUsize::new(0),
                age_ticks: AtomicUsize::new(0),
            }
        }

        fn collect_next(&self) -> oneshot::Sender<Outcome> {
            let (send, receive) = oneshot::channel();
            self.collections.lock().expect("lock").push_back(receive);
            send
        }

        fn post_next(&self) -> oneshot::Sender<Outcome> {
            let (send, receive) = oneshot::channel();
            self.post_collections
                .lock()
                .expect("lock")
                .push_back(receive);
            send
        }
    }

    impl RefreshWorker for std::sync::Arc<TestWorker> {
        type Value = &'static str;
        type Error = &'static str;

        fn collect(&self) -> impl Future<Output = Outcome> + Send {
            let receive = self
                .collections
                .lock()
                .expect("lock")
                .pop_front()
                .expect("a collection outcome must be queued before open");
            async move { receive.await.expect("scheduler must keep the task alive") }
        }

        fn on_start(&self) {
            self.starts.fetch_add(1, Ordering::SeqCst);
        }

        fn on_success(&self, value: &&'static str, attempt: &RefreshAttempt) {
            if attempt.is_current() {
                self.successes.lock().expect("lock").push(*value);
            }
        }

        fn after_success(
            &self,
            value: &'static str,
            _attempt: RefreshAttempt,
        ) -> impl Future<Output = PostOutcome> + Send {
            self.post_started.lock().expect("lock").push(value);
            let receive = self
                .post_collections
                .lock()
                .expect("lock")
                .pop_front()
                .expect("a post-collection outcome must be queued before success");
            async move {
                receive
                    .await
                    .expect("scheduler must keep the task alive")
                    .map(|_| ())
            }
        }

        fn on_failure(&self, error: &&'static str) {
            self.failures.lock().expect("lock").push(*error);
        }

        fn on_scheduled(&self, delay: Duration, after_failure: bool) {
            self.scheduled
                .lock()
                .expect("lock")
                .push((delay, after_failure));
        }

        fn on_settled(&self) {
            self.settled.fetch_add(1, Ordering::SeqCst);
        }

        fn on_age_tick(&self) {
            self.age_ticks.fetch_add(1, Ordering::SeqCst);
        }

        fn cancel_active(&self) {
            self.cancellations.fetch_add(1, Ordering::SeqCst);
        }
    }

    async fn flush() {
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn post_collection_consumer_preserves_enqueue_order() {
        let worker = std::sync::Arc::new(TestWorker::new());
        let first = worker.post_next();
        let second = worker.post_next();
        let current_generation = std::sync::Arc::new(super::AtomicU64::new(1));
        let closed = std::sync::Arc::new(super::AtomicBool::new(false));
        let attempt = RefreshAttempt {
            generation: 1,
            current_generation,
            closed,
        };
        let publication_state = std::sync::Arc::new(tokio::sync::Mutex::new(PublicationState {
            generation: 1,
            closed: false,
        }));
        let (events, mut event_receiver) =
            tokio::sync::mpsc::unbounded_channel::<super::Event<std::sync::Arc<TestWorker>>>();
        let (sender, receiver) =
            tokio::sync::mpsc::unbounded_channel::<PostCollection<std::sync::Arc<TestWorker>>>();
        let consumer = tokio::spawn(run_post_collections(
            std::sync::Arc::new(std::sync::Arc::clone(&worker)),
            events,
            receiver,
            publication_state,
        ));

        sender
            .send(PostCollection {
                generation: 1,
                attempt: attempt.clone(),
                value: "first",
            })
            .expect("consumer");
        sender
            .send(PostCollection {
                generation: 1,
                attempt,
                value: "second",
            })
            .expect("consumer");
        flush().await;
        assert_eq!(
            worker.post_started.lock().expect("lock").as_slice(),
            ["first"]
        );

        first.send(Ok("saved")).expect("receiver");
        event_receiver.recv().await.expect("completion");
        flush().await;
        assert_eq!(
            worker.post_started.lock().expect("lock").as_slice(),
            ["first", "second"]
        );
        second.send(Ok("saved")).expect("receiver");
        drop(sender);
        consumer.await.expect("consumer task");
    }

    #[tokio::test]
    async fn manual_refresh_cannot_advance_generation_during_publication() {
        let worker = std::sync::Arc::new(TestWorker::new());
        let first = worker.collect_next();
        let publication = worker.post_next();
        let second = worker.collect_next();
        let handle = open(std::sync::Arc::clone(&worker));
        flush().await;

        first.send(Ok("first")).expect("receiver");
        flush().await;
        assert_eq!(
            worker.post_started.lock().expect("lock").as_slice(),
            ["first"]
        );

        let manual_handle = handle.clone();
        let manual = tokio::spawn(async move { manual_handle.manual().await });
        flush().await;
        assert_eq!(worker.starts.load(Ordering::SeqCst), 1);

        publication.send(Ok("saved")).expect("receiver");
        manual.await.expect("manual task");
        assert_eq!(worker.starts.load(Ordering::SeqCst), 2);
        handle.close().await;
        drop(second);
    }

    #[tokio::test(start_paused = true)]
    async fn opens_immediately_then_waits_five_minutes_after_post_collection_work() {
        let worker = std::sync::Arc::new(TestWorker::new());
        let first = worker.collect_next();
        let post = worker.post_next();
        let second = worker.collect_next();
        let handle = open(std::sync::Arc::clone(&worker));
        flush().await;
        assert_eq!(worker.starts.load(Ordering::SeqCst), 1);

        first.send(Ok("first")).expect("receiver");
        flush().await;
        assert_eq!(worker.successes.lock().expect("lock").as_slice(), ["first"]);
        tokio::time::advance(NORMAL_REFRESH).await;
        flush().await;
        assert_eq!(worker.starts.load(Ordering::SeqCst), 1);

        post.send(Ok("recorded")).expect("receiver");
        flush().await;
        tokio::time::advance(NORMAL_REFRESH - Duration::from_secs(1)).await;
        flush().await;
        assert_eq!(worker.starts.load(Ordering::SeqCst), 1);
        tokio::time::advance(Duration::from_secs(1)).await;
        flush().await;
        assert_eq!(worker.starts.load(Ordering::SeqCst), 2);
        handle.close().await;
        drop(second);
    }

    #[tokio::test(start_paused = true)]
    async fn automatic_refreshes_do_not_overlap() {
        let worker = std::sync::Arc::new(TestWorker::new());
        let first = worker.collect_next();
        let first_post = worker.post_next();
        let second = worker.collect_next();
        let handle = open(std::sync::Arc::clone(&worker));
        flush().await;

        tokio::time::advance(Duration::from_secs(60 * 60)).await;
        flush().await;
        assert_eq!(worker.starts.load(Ordering::SeqCst), 1);

        first.send(Ok("first")).expect("receiver");
        flush().await;
        first_post.send(Ok("saved")).expect("receiver");
        flush().await;
        tokio::time::advance(NORMAL_REFRESH).await;
        flush().await;
        assert_eq!(worker.starts.load(Ordering::SeqCst), 2);
        handle.close().await;
        drop(second);
    }

    #[tokio::test(start_paused = true)]
    async fn age_ticks_redraw_without_starting_a_collection() {
        let worker = std::sync::Arc::new(TestWorker::new());
        let pending = worker.collect_next();
        let handle = open(std::sync::Arc::clone(&worker));
        flush().await;

        tokio::time::advance(AGE_TICK).await;
        flush().await;

        assert_eq!(worker.age_ticks.load(Ordering::SeqCst), 1);
        assert_eq!(worker.starts.load(Ordering::SeqCst), 1);
        handle.close().await;
        drop(pending);
    }

    #[tokio::test(start_paused = true)]
    async fn failures_back_off_at_ten_twenty_then_thirty_minutes() {
        let worker = std::sync::Arc::new(TestWorker::new());
        let attempts = (0..4).map(|_| worker.collect_next()).collect::<Vec<_>>();
        let handle = open(std::sync::Arc::clone(&worker));
        flush().await;

        for (index, (sender, expected)) in attempts
            .into_iter()
            .zip(FAILURE_BACKOFF.into_iter().chain([FAILURE_BACKOFF[2]]))
            .enumerate()
        {
            sender.send(Err("failed")).expect("receiver");
            flush().await;
            assert_eq!(
                worker.scheduled.lock().expect("lock").last(),
                Some(&(expected, true))
            );
            if index < 3 {
                tokio::time::advance(expected).await;
                flush().await;
            }
        }
        assert_eq!(worker.failures.lock().expect("lock").len(), 4);
        handle.close().await;
    }

    #[tokio::test(start_paused = true)]
    async fn manual_preempts_late_results_and_resets_failure_backoff() {
        let worker = std::sync::Arc::new(TestWorker::new());
        let old = worker.collect_next();
        let failing = worker.collect_next();
        let manual_failure = worker.collect_next();
        let handle = open(std::sync::Arc::clone(&worker));
        flush().await;

        handle.manual().await;
        assert_eq!(worker.cancellations.load(Ordering::SeqCst), 1);
        assert_eq!(worker.starts.load(Ordering::SeqCst), 2);
        old.send(Ok("late")).expect("receiver");
        failing.send(Err("first failure")).expect("receiver");
        flush().await;
        assert!(worker.successes.lock().expect("lock").is_empty());
        assert_eq!(
            worker.scheduled.lock().expect("lock").last(),
            Some(&(FAILURE_BACKOFF[0], true))
        );

        handle.manual().await;
        manual_failure
            .send(Err("manual failure"))
            .expect("receiver");
        flush().await;
        assert_eq!(
            worker.scheduled.lock().expect("lock").last(),
            Some(&(FAILURE_BACKOFF[0], true))
        );
        handle.close().await;
    }

    #[tokio::test(start_paused = true)]
    async fn close_cancels_active_work_and_ignores_late_results() {
        let worker = std::sync::Arc::new(TestWorker::new());
        let late = worker.collect_next();
        let handle = open(std::sync::Arc::clone(&worker));
        flush().await;

        handle.close().await;
        assert_eq!(worker.cancellations.load(Ordering::SeqCst), 1);
        assert_eq!(worker.settled.load(Ordering::SeqCst), 0);
        assert!(late.send(Ok("late")).is_err());
        flush().await;
        assert!(worker.successes.lock().expect("lock").is_empty());
        assert!(worker.scheduled.lock().expect("lock").is_empty());
    }
}
