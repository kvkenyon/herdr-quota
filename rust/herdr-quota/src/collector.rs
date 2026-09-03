//! Bounded collection from the plugin-local quota-axi executable.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep_until, timeout};

use crate::domain::schema::{QuotaReport, parse_quota_response};

/// The complete wall-clock deadline for one quota-axi invocation.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(12);

const MAX_STDOUT_BYTES: usize = 2 * 1024 * 1024;
const TERMINATION_GRACE: Duration = Duration::from_millis(500);
const QUOTA_AXI_ARGUMENTS: [&str; 4] = [
    "--json",
    "--full",
    "--provider",
    "claude,codex,cursor,kimi,grok,copilot",
];

/// A finite, display-safe whole-collector failure.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CollectorFailure {
    /// The process exceeded the collector deadline.
    #[error("Quota check timed out")]
    Timeout,
    /// The plugin-local quota-axi executable could not be found.
    #[error("quota-axi executable is missing")]
    MissingExecutable,
    /// The process output exceeded the bound or was not schema v5.
    #[error("quota-axi output is incompatible")]
    IncompatibleOutput,
    /// The process could not start, failed, or could not be read.
    #[error("Quota network/process check failed")]
    NetworkProcess,
}

impl CollectorFailure {
    /// Return the stable serialized/display identifier for this failure.
    pub const fn kind(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::MissingExecutable => "missing_executable",
            Self::IncompatibleOutput => "incompatible_output",
            Self::NetworkProcess => "network_process",
        }
    }
}

/// Configuration for one collector instance.
#[derive(Clone)]
struct CollectorConfig {
    executable: PathBuf,
    working_directory: PathBuf,
    deadline: Duration,
}

struct ActiveCollection {
    generation: u64,
    cancel: watch::Sender<bool>,
}

#[derive(Default)]
struct CollectorState {
    generation: u64,
    active: Option<ActiveCollection>,
}

impl Drop for CollectorState {
    fn drop(&mut self) {
        if let Some(active) = self.active.take() {
            let _ = active.cancel.send(true);
        }
    }
}

/// A serial collection owner. Starting or cancelling a collection invalidates
/// every prior generation.
#[derive(Clone)]
pub struct Collector {
    config: Arc<CollectorConfig>,
    state: Arc<Mutex<CollectorState>>,
}

impl Default for Collector {
    fn default() -> Self {
        Self::new()
    }
}

impl Collector {
    /// Collect from this plugin's own quota-axi installation.
    pub fn new() -> Self {
        Self::with_executable(local_quota_axi_executable(), plugin_root(), DEFAULT_TIMEOUT)
    }

    /// Construct a collector around an explicitly selected executable.
    ///
    /// This is intended for bounded fake-executable tests. Production callers
    /// should use [`Collector::new`].
    pub fn with_executable(
        executable: impl Into<PathBuf>,
        working_directory: impl Into<PathBuf>,
        deadline: Duration,
    ) -> Self {
        Self {
            config: Arc::new(CollectorConfig {
                executable: executable.into(),
                working_directory: working_directory.into(),
                deadline,
            }),
            state: Arc::new(Mutex::new(CollectorState::default())),
        }
    }

    /// Start a collection and return a generation-guarded publication.
    ///
    /// A newer call preempts the active child. The returned publication can be
    /// applied only if no newer call or cancellation has invalidated it.
    pub async fn collect(&self) -> Collection {
        let (generation, cancellation) = self.begin();
        let config = Arc::clone(&self.config);
        let state = Arc::downgrade(&self.state);
        let worker_state = state.clone();

        // Keeping the process owner in its own task means dropping a caller's
        // wait cannot orphan the child. Explicit cancellation or final owner
        // drop still reaches the task through the watch channel.
        let worker = tokio::spawn(async move {
            let result = run_collection(&config, cancellation).await;
            finish_generation(&worker_state, generation);
            Collection {
                generation,
                result,
                state: worker_state,
            }
        });

        match worker.await {
            Ok(collection) => collection,
            Err(_) => {
                finish_generation(&state, generation);
                Collection {
                    generation,
                    result: Some(Err(CollectorFailure::NetworkProcess)),
                    state,
                }
            }
        }
    }

    /// Cancel the active child and invalidate any result it might produce.
    pub fn cancel(&self) {
        let active = {
            let mut state = lock_state(&self.state);
            state.generation = next_generation(state.generation);
            state.active.take()
        };
        if let Some(active) = active {
            let _ = active.cancel.send(true);
        }
    }

    fn begin(&self) -> (u64, watch::Receiver<bool>) {
        let (cancel, cancellation) = watch::channel(false);
        let previous = {
            let mut state = lock_state(&self.state);
            state.generation = next_generation(state.generation);
            let generation = state.generation;
            let previous = state
                .active
                .replace(ActiveCollection { generation, cancel });
            (generation, previous)
        };
        if let Some(active) = previous.1 {
            let _ = active.cancel.send(true);
        }
        (previous.0, cancellation)
    }
}

/// A completed result that can publish only while its generation is current.
pub struct Collection {
    generation: u64,
    result: Option<Result<QuotaReport, CollectorFailure>>,
    state: Weak<Mutex<CollectorState>>,
}

impl Collection {
    /// Invoke `publish` only if this collection is still current.
    ///
    /// The callback runs while generation advancement is excluded, so a
    /// manual refresh cannot slip between the currency check and publication.
    /// It must not call back into this collector.
    pub fn publish<R>(
        mut self,
        publish: impl FnOnce(Result<QuotaReport, CollectorFailure>) -> R,
    ) -> Option<R> {
        let state = self.state.upgrade()?;
        let state = lock_state(&state);
        if state.generation != self.generation {
            return None;
        }
        self.result.take().map(publish)
    }
}

/// Return the exact plugin-local quota-axi executable path.
pub fn local_quota_axi_executable() -> PathBuf {
    plugin_root()
        .join("node_modules")
        .join(".bin")
        .join("quota-axi")
}

fn plugin_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the Rust package is nested beneath the plugin root")
        .to_path_buf()
}

fn lock_state(state: &Mutex<CollectorState>) -> MutexGuard<'_, CollectorState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn next_generation(generation: u64) -> u64 {
    generation.wrapping_add(1).max(1)
}

fn finish_generation(state: &Weak<Mutex<CollectorState>>, generation: u64) {
    let Some(state) = state.upgrade() else {
        return;
    };
    let mut state = lock_state(&state);
    if state
        .active
        .as_ref()
        .is_some_and(|active| active.generation == generation)
    {
        state.active = None;
    }
}

async fn run_collection(
    config: &CollectorConfig,
    mut cancellation: watch::Receiver<bool>,
) -> Option<Result<QuotaReport, CollectorFailure>> {
    let mut command = Command::new(&config.executable);
    command
        .args(QUOTA_AXI_ARGUMENTS)
        .current_dir(&config.working_directory)
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Some(Err(if error.kind() == io::ErrorKind::NotFound {
                CollectorFailure::MissingExecutable
            } else {
                CollectorFailure::NetworkProcess
            }));
        }
    };
    let Some(process_id) = child.id() else {
        return Some(Err(CollectorFailure::NetworkProcess));
    };
    let Some(mut stdout) = child.stdout.take() else {
        return Some(Err(CollectorFailure::NetworkProcess));
    };
    let Some(stderr) = child.stderr.take() else {
        return Some(Err(CollectorFailure::NetworkProcess));
    };

    let stderr_drain = tokio::spawn(async move {
        let mut stderr = stderr;
        let mut sink = tokio::io::sink();
        let _ = tokio::io::copy(&mut stderr, &mut sink).await;
        let _ = sink.shutdown().await;
    });
    let mut wait = tokio::spawn(async move { child.wait().await });
    let deadline = sleep_until(Instant::now() + config.deadline);
    tokio::pin!(deadline);
    let mut output = Vec::new();
    let mut stdout_done = false;
    let mut exit_status = None;
    let mut chunk = [0_u8; 16 * 1024];

    loop {
        if stdout_done && exit_status.is_some() {
            break;
        }
        tokio::select! {
            _ = &mut deadline => {
                terminate_and_reap(process_id, &mut wait).await;
                stop_drain(stderr_drain).await;
                return Some(Err(CollectorFailure::Timeout));
            }
            _ = cancellation_requested(&mut cancellation) => {
                terminate_and_reap(process_id, &mut wait).await;
                stop_drain(stderr_drain).await;
                return None;
            }
            read = stdout.read(&mut chunk), if !stdout_done => {
                match read {
                    Ok(0) => stdout_done = true,
                    Ok(count) if output.len().saturating_add(count) <= MAX_STDOUT_BYTES => {
                        output.extend_from_slice(&chunk[..count]);
                    }
                    Ok(_) => {
                        terminate_and_reap(process_id, &mut wait).await;
                        stop_drain(stderr_drain).await;
                        return Some(Err(CollectorFailure::IncompatibleOutput));
                    }
                    Err(_) => {
                        terminate_and_reap(process_id, &mut wait).await;
                        stop_drain(stderr_drain).await;
                        return Some(Err(CollectorFailure::NetworkProcess));
                    }
                }
            }
            status = &mut wait, if exit_status.is_none() => {
                exit_status = Some(joined_exit_status(status));
            }
        }
    }

    stop_drain(stderr_drain).await;
    let status = match exit_status.expect("exit status is set before loop completion") {
        Ok(status) => status,
        Err(failure) => return Some(Err(failure)),
    };
    if !status.success() {
        return Some(Err(CollectorFailure::NetworkProcess));
    }
    let report = std::str::from_utf8(&output)
        .ok()
        .and_then(|output| parse_quota_response(output).ok())
        .ok_or(CollectorFailure::IncompatibleOutput);
    Some(report)
}

async fn cancellation_requested(cancellation: &mut watch::Receiver<bool>) {
    if *cancellation.borrow() {
        return;
    }
    let _ = cancellation.changed().await;
}

fn joined_exit_status(
    status: Result<io::Result<ExitStatus>, tokio::task::JoinError>,
) -> Result<ExitStatus, CollectorFailure> {
    status
        .map_err(|_| CollectorFailure::NetworkProcess)?
        .map_err(|_| CollectorFailure::NetworkProcess)
}

async fn terminate_and_reap(process_id: u32, wait: &mut JoinHandle<io::Result<ExitStatus>>) {
    if wait.is_finished() {
        let _ = wait.await;
        return;
    }
    let _ = crate::unix_signal::terminate(process_id);
    if timeout(TERMINATION_GRACE, &mut *wait).await.is_err() {
        let _ = crate::unix_signal::kill(process_id);
        let _ = wait.await;
    }
}

async fn stop_drain(drain: JoinHandle<()>) {
    if !drain.is_finished() {
        drain.abort();
    }
    let _ = drain.await;
}

#[cfg(test)]
mod tests {
    use super::{CollectorFailure, DEFAULT_TIMEOUT, QUOTA_AXI_ARGUMENTS};
    use std::time::Duration;

    #[test]
    fn constants_preserve_the_collector_contract() {
        assert_eq!(DEFAULT_TIMEOUT, Duration::from_secs(12));
        assert_eq!(
            QUOTA_AXI_ARGUMENTS,
            [
                "--json",
                "--full",
                "--provider",
                "claude,codex,cursor,kimi,grok,copilot",
            ]
        );
    }

    #[test]
    fn failures_are_a_closed_sanitized_set() {
        let failures = [
            CollectorFailure::Timeout,
            CollectorFailure::MissingExecutable,
            CollectorFailure::IncompatibleOutput,
            CollectorFailure::NetworkProcess,
        ];
        assert_eq!(
            failures.map(CollectorFailure::kind),
            [
                "timeout",
                "missing_executable",
                "incompatible_output",
                "network_process",
            ]
        );
        for failure in failures {
            let message = failure.to_string();
            assert!(!message.contains('\u{1b}'));
            assert!(!message.contains("/Users/"));
            assert!(!message.contains("/home/"));
        }
    }
}
