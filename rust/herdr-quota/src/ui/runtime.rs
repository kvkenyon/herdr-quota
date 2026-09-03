//! Pane-owned live dashboard runtime.

use std::collections::BTreeSet;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Utc};
use crossterm::event::KeyCode;
use ratatui::{Terminal, backend::CrosstermBackend, text::Line, widgets::Paragraph};
use tokio::sync::mpsc;
use unicode_width::UnicodeWidthStr;

use crate::app_state::{self, AppAction, AppState, CollectorFailureKind};
use crate::collector::{Collector, CollectorFailure};
use crate::domain::{
    history_evidence::{self as evidence, HistoryAvailability},
    provider::{MarketedProvider, ProjectionConfidence},
    schema::QuotaReport,
};
use crate::scheduler::{self, RefreshAttempt, RefreshWorker};
use crate::store::sidebar_state::SidebarOwnershipGuard;
use crate::store::{
    history::LocalHistory,
    settings::{
        DashboardSettings, MeterMode as SettingsMeterMode, RemainingThreshold as SettingsThreshold,
        SettingsAvailability, SettingsStore, StartupView, SupportedProvider,
    },
    transitions::{
        self, HistoryDataHealth, HistoryFact, HistoryProjectionConfidence, HistoryProviderSnapshot,
        HistoryRunway, HistoryRunwayState, HistorySnapshot, RemainingThreshold,
        TransitionAvailability, TransitionChannel, TransitionHistory, TransitionKind,
        TransitionSettings, TransitionStore, TransitionView,
    },
};
use crate::ui::{
    bar::MeterMode,
    keys::{DashboardAction, ESCAPE_FRAGMENT_TIMEOUT, TerminalInputParser},
    preferences::{
        self, PreferenceAction, PreferenceCommand, PreferenceFocus, PreferenceNotice,
        PreferencesState,
    },
    readiness::provider_readiness,
    render::{
        DashboardConfig, DashboardStatus, DashboardView, InputAction, clamp_scroll, draw_dashboard,
        handle_key, visible_provider_count,
    },
    terminal::{RawInput, TerminalGuard},
};

struct RuntimeState {
    app: AppState,
    settings: DashboardSettings,
    settings_availability: SettingsAvailability,
    settings_store: Option<SettingsStore>,
    history: LocalHistory,
    history_matches_report: bool,
    transition_store: Option<TransitionStore>,
    transitions: TransitionView,
    preferences: Option<PreferencesState>,
    transition_review: bool,
    view: DashboardView,
    selected_provider: usize,
    scroll: usize,
    terminal_height: u16,
}

fn startup_dashboard_view(startup_view: StartupView) -> DashboardView {
    match startup_view {
        StartupView::Overview => DashboardView::Overview,
        StartupView::Details => DashboardView::Details,
    }
}

impl RuntimeState {
    fn load() -> Self {
        let settings_store = SettingsStore::from_environment().ok();
        let loaded = settings_store
            .as_ref()
            .map(SettingsStore::load)
            .unwrap_or_else(|| crate::store::settings::SettingsLoadResult {
                settings: DashboardSettings::default(),
                availability: SettingsAvailability::Unavailable,
            });
        let mut history = LocalHistory::default();
        let retained = history.retained().cloned();
        let transition_store = TransitionStore::from_environment().ok();
        let transition_settings = transition_settings(&loaded.settings);
        let transitions = match (transition_store.as_ref(), retained.as_ref()) {
            (Some(store), Some(history)) => {
                store.load_view(&transition_history(history), &transition_settings)
            }
            (None, _) => empty_transitions(TransitionAvailability::Unavailable),
            (_, None) => empty_transitions(TransitionAvailability::FirstRun),
        };
        let view = startup_dashboard_view(loaded.settings.startup_view);
        Self {
            app: AppState::default(),
            settings: loaded.settings,
            settings_availability: loaded.availability,
            settings_store,
            history,
            history_matches_report: false,
            transition_store,
            transitions,
            preferences: None,
            transition_review: false,
            view,
            selected_provider: 0,
            scroll: 0,
            terminal_height: 24,
        }
    }
}

fn lock(state: &Arc<Mutex<RuntimeState>>) -> MutexGuard<'_, RuntimeState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Clone)]
struct DashboardWorker {
    collector: Collector,
    state: Arc<Mutex<RuntimeState>>,
    redraw: mpsc::UnboundedSender<()>,
}

impl DashboardWorker {
    fn changed(&self) {
        let _ = self.redraw.send(());
    }
}

impl RefreshWorker for DashboardWorker {
    type Value = QuotaReport;
    type Error = CollectorFailure;

    async fn collect(&self) -> Result<Self::Value, Self::Error> {
        self.collector
            .collect()
            .await
            .publish(|result| result)
            .unwrap_or(Err(CollectorFailure::NetworkProcess))
    }

    fn on_start(&self) {
        app_state::reduce(
            &mut lock(&self.state).app,
            AppAction::RefreshStarted { at: Instant::now() },
        );
        self.changed();
    }

    fn on_success(&self, report: &QuotaReport, attempt: &RefreshAttempt) {
        let state = Arc::clone(&self.state);
        let report = report.clone();
        if attempt
            .publish(|| {
                let mut state = lock(&state);
                app_state::reduce(&mut state.app, AppAction::CollectionSucceeded { report });
                state.history_matches_report = false;
            })
            .is_some()
        {
            self.changed();
        }
    }

    async fn after_success(
        &self,
        report: QuotaReport,
        attempt: RefreshAttempt,
    ) -> Result<(), Self::Error> {
        let state = Arc::clone(&self.state);
        if attempt
            .publish(|| {
                let mut state = lock(&state);
                let history_view = state.history.record(&report);
                state.history_matches_report = state.history.represents(&report);
                let history = state.history.current().cloned();
                let settings = transition_settings(&state.settings);
                state.transitions = match (
                    history_view.availability,
                    state.transition_store.as_ref(),
                    history,
                ) {
                    (HistoryAvailability::Incompatible, _, _) => {
                        empty_transitions(TransitionAvailability::Incompatible)
                    }
                    (HistoryAvailability::Unavailable, _, _) => {
                        empty_transitions(TransitionAvailability::Unavailable)
                    }
                    (HistoryAvailability::ClockSkew, Some(store), Some(history)) => store.baseline(
                        &transition_history(&history),
                        &settings,
                        &[TransitionChannel::Threshold, TransitionChannel::Forecast],
                        None,
                    ),
                    (_, Some(store), Some(history)) => {
                        store.evaluate(&transition_history(&history), &settings)
                    }
                    (_, None, _) => empty_transitions(TransitionAvailability::Unavailable),
                    (_, _, None) => empty_transitions(TransitionAvailability::FirstRun),
                };
            })
            .is_some()
        {
            self.changed();
        }
        Ok(())
    }

    fn on_failure(&self, error: &Self::Error) {
        app_state::reduce(
            &mut lock(&self.state).app,
            AppAction::CollectionFailed {
                kind: collector_failure(*error),
            },
        );
        self.changed();
    }

    fn on_scheduled(&self, delay: Duration, after_failure: bool) {
        app_state::reduce(
            &mut lock(&self.state).app,
            AppAction::RefreshScheduled {
                at: Instant::now(),
                delay,
                after_failure,
            },
        );
        self.changed();
    }

    fn on_settled(&self) {
        app_state::reduce(&mut lock(&self.state).app, AppAction::RefreshSettled);
        self.changed();
    }

    fn on_age_tick(&self) {
        app_state::reduce(&mut lock(&self.state).app, AppAction::AgeTick);
        self.changed();
    }

    fn cancel_active(&self) {
        self.collector.cancel();
    }
}

fn collector_failure(error: CollectorFailure) -> CollectorFailureKind {
    match error {
        CollectorFailure::Timeout => CollectorFailureKind::Timeout,
        CollectorFailure::MissingExecutable => CollectorFailureKind::MissingExecutable,
        CollectorFailure::IncompatibleOutput => CollectorFailureKind::IncompatibleOutput,
        CollectorFailure::NetworkProcess => CollectorFailureKind::NetworkProcess,
    }
}

fn dashboard_status(app: &AppState) -> Option<DashboardStatus> {
    if app.loading {
        return Some(DashboardStatus::Refreshing);
    }
    app.failure.as_ref().map(|failure| match failure.kind {
        CollectorFailureKind::Timeout => DashboardStatus::Timeout,
        CollectorFailureKind::MissingExecutable => DashboardStatus::MissingExecutable,
        CollectorFailureKind::IncompatibleOutput => DashboardStatus::IncompatibleOutput,
        CollectorFailureKind::NetworkProcess => DashboardStatus::NetworkProcess,
    })
}

fn retry_minutes(app: &AppState) -> Option<u64> {
    app.failure.as_ref()?.retry_at.map(|retry_at| {
        retry_at
            .saturating_duration_since(Instant::now())
            .as_secs()
            .div_ceil(60)
    })
}

fn transition_settings(settings: &DashboardSettings) -> TransitionSettings {
    TransitionSettings {
        hidden_providers: settings
            .hidden_providers
            .iter()
            .filter_map(|provider| MarketedProvider::from_id(provider.id()))
            .collect(),
        remaining_threshold: match settings.remaining_threshold {
            SettingsThreshold::Off => RemainingThreshold::Off,
            SettingsThreshold::Percent25 => RemainingThreshold::Percent25,
            SettingsThreshold::Percent10 => RemainingThreshold::Percent10,
            SettingsThreshold::Percent5 => RemainingThreshold::Percent5,
        },
        forecast_before_reset: settings.forecast_before_reset,
    }
}

fn transition_history(document: &evidence::HistoryDocument) -> TransitionHistory {
    TransitionHistory {
        snapshots: document
            .snapshots
            .iter()
            .map(|snapshot| HistorySnapshot {
                captured_at: snapshot.captured_at.clone(),
                providers: snapshot
                    .providers
                    .iter()
                    .map(|provider| HistoryProviderSnapshot {
                        provider: provider.provider.marketed(),
                        data_health: match provider.data_health {
                            evidence::HistoryDataHealth::Current => HistoryDataHealth::Current,
                            evidence::HistoryDataHealth::Stale => HistoryDataHealth::Stale,
                            evidence::HistoryDataHealth::Unavailable => {
                                HistoryDataHealth::Unavailable
                            }
                            evidence::HistoryDataHealth::Error => HistoryDataHealth::Error,
                            evidence::HistoryDataHealth::Unknown => HistoryDataHealth::Unknown,
                        },
                        auth_eligible: provider.auth_eligible,
                        facts: provider
                            .facts
                            .iter()
                            .map(|fact| HistoryFact {
                                scope: fact.scope.clone(),
                                limit: fact.limit.clone(),
                                remaining: fact.remaining,
                                reset_at: fact.reset_at.clone(),
                                runway: fact.runway.as_ref().map(|runway| HistoryRunway {
                                    state: match runway.state {
                                        evidence::HistoryRunwayState::ExhaustedNow => {
                                            HistoryRunwayState::ExhaustedNow
                                        }
                                        evidence::HistoryRunwayState::ProjectedExhaustion => {
                                            HistoryRunwayState::ProjectedExhaustion
                                        }
                                        evidence::HistoryRunwayState::ThroughReset => {
                                            HistoryRunwayState::ThroughReset
                                        }
                                    },
                                    confidence: runway.confidence.map(
                                        |confidence| match confidence {
                                            ProjectionConfidence::Early => {
                                                HistoryProjectionConfidence::Early
                                            }
                                            ProjectionConfidence::Established => {
                                                HistoryProjectionConfidence::Established
                                            }
                                        },
                                    ),
                                }),
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn empty_transitions(availability: TransitionAvailability) -> TransitionView {
    TransitionView {
        availability,
        events: Vec::new(),
    }
}

/// Open the live collector-backed dashboard until the pane quits or is signalled.
pub async fn dashboard() -> io::Result<()> {
    dashboard_with_collector(Collector::new()).await
}

/// Test seam for exercising the complete terminal loop with a bounded fake collector.
#[doc(hidden)]
pub async fn dashboard_with_collector(collector: Collector) -> io::Result<()> {
    let _sidebar_ownership = SidebarOwnershipGuard::from_environment();
    if !crossterm::tty::IsTty::is_tty(&io::stdin()) || !crossterm::tty::IsTty::is_tty(&io::stdout())
    {
        return Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "dashboard requires an interactive terminal",
        ));
    }
    let state = Arc::new(Mutex::new(RuntimeState::load()));
    let mut signals = Signals::new()?;
    let (redraw, mut redraws) = mpsc::unbounded_channel();
    let worker = DashboardWorker {
        collector,
        state: Arc::clone(&state),
        redraw,
    };
    let refresh = scheduler::open(worker);
    let mut guard = TerminalGuard::enter_stdout()?;
    let (input_guard, mut input) = RawInput::open();
    let result = {
        let backend = CrosstermBackend::new(guard.writer_mut());
        let mut terminal = Terminal::new(backend)?;
        run_loop(
            &mut terminal,
            Arc::clone(&state),
            &refresh,
            &mut input,
            &mut redraws,
            &mut signals,
        )
        .await
    };
    refresh.close().await;
    drop(input_guard);
    let cleanup = guard.restore();
    result.and(cleanup)
}

async fn run_loop<W: io::Write>(
    terminal: &mut Terminal<CrosstermBackend<&mut W>>,
    state: Arc<Mutex<RuntimeState>>,
    refresh: &scheduler::RefreshHandle,
    input: &mut mpsc::UnboundedReceiver<io::Result<Vec<u8>>>,
    redraws: &mut mpsc::UnboundedReceiver<()>,
    signals: &mut Signals,
) -> io::Result<()> {
    let mut parser = TerminalInputParser::default();
    let mut escape_deadline = None;
    draw(terminal, &state)?;
    loop {
        tokio::select! {
            chunk = input.recv() => {
                let chunk = chunk.ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "terminal input stopped"))??;
                let actions = parser.push(&chunk);
                escape_deadline = parser.has_pending().then(|| tokio::time::Instant::now() + ESCAPE_FRAGMENT_TIMEOUT);
                if handle_actions(actions, &state, refresh).await? { return Ok(()); }
                draw(terminal, &state)?;
            }
            _ = async {
                match escape_deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            } => {
                escape_deadline = None;
                if handle_actions(parser.flush(), &state, refresh).await? { return Ok(()); }
                draw(terminal, &state)?;
            }
            Some(()) = redraws.recv() => draw(terminal, &state)?,
            signal = signals.next() => match signal {
                SignalEvent::Resize => draw(terminal, &state)?,
                SignalEvent::Shutdown => return Ok(()),
            }
        }
    }
}

#[cfg(unix)]
enum SignalEvent {
    Resize,
    Shutdown,
}

#[cfg(unix)]
struct Signals {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
    hangup: tokio::signal::unix::Signal,
    resize: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl Signals {
    fn new() -> io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
            hangup: signal(SignalKind::hangup())?,
            resize: signal(SignalKind::window_change())?,
        })
    }

    async fn next(&mut self) -> SignalEvent {
        tokio::select! {
            _ = self.interrupt.recv() => SignalEvent::Shutdown,
            _ = self.terminate.recv() => SignalEvent::Shutdown,
            _ = self.hangup.recv() => SignalEvent::Shutdown,
            _ = self.resize.recv() => SignalEvent::Resize,
        }
    }
}

#[cfg(not(unix))]
struct Signals;

#[cfg(not(unix))]
impl Signals {
    fn new() -> io::Result<Self> {
        Ok(Self)
    }

    async fn next(&mut self) -> SignalEvent {
        std::future::pending().await
    }
}

#[cfg(not(unix))]
enum SignalEvent {
    Resize,
    Shutdown,
}

async fn handle_actions(
    actions: Vec<DashboardAction>,
    shared: &Arc<Mutex<RuntimeState>>,
    refresh: &scheduler::RefreshHandle,
) -> io::Result<bool> {
    for action in actions {
        if action == DashboardAction::Quit {
            return Ok(true);
        }
        let mode = {
            let state = lock(shared);
            (state.preferences.is_some(), state.transition_review)
        };
        if mode.0 {
            handle_preference(action, shared);
            continue;
        }
        if mode.1 {
            match action {
                DashboardAction::Escape => lock(shared).transition_review = false,
                DashboardAction::Acknowledge | DashboardAction::Activate => acknowledge(shared),
                _ => {}
            }
            continue;
        }
        match action {
            DashboardAction::Refresh => refresh.manual().await,
            DashboardAction::Preferences => {
                let mut state = lock(shared);
                state.preferences = Some(preferences::open(&state.settings));
            }
            DashboardAction::Acknowledge => {
                let mut state = lock(shared);
                if !state.transitions.events.is_empty() {
                    state.transition_review = true;
                }
            }
            DashboardAction::Escape
            | DashboardAction::ScrollDown
            | DashboardAction::ScrollUp
            | DashboardAction::PageDown
            | DashboardAction::PageUp
            | DashboardAction::Activate
            | DashboardAction::SelectProvider(_) => {
                let mut state = lock(shared);
                let mut config = dashboard_config(&state);
                let Some(provider_count) = state
                    .app
                    .report
                    .as_ref()
                    .map(|report| visible_provider_count(report, &config))
                else {
                    if action == DashboardAction::Escape {
                        return Ok(true);
                    }
                    continue;
                };
                let key = match action {
                    DashboardAction::Escape => KeyCode::Esc,
                    DashboardAction::ScrollDown => KeyCode::Down,
                    DashboardAction::ScrollUp => KeyCode::Up,
                    DashboardAction::PageDown => KeyCode::PageDown,
                    DashboardAction::PageUp => KeyCode::PageUp,
                    DashboardAction::Activate => KeyCode::Enter,
                    DashboardAction::SelectProvider(index) => {
                        KeyCode::Char(char::from_digit((index + 1) as u32, 10).unwrap())
                    }
                    _ => unreachable!(),
                };
                if handle_key(&mut config, key, provider_count) == InputAction::Quit {
                    return Ok(true);
                }
                state.view = config.view;
                state.selected_provider = config.selected_provider;
                state.scroll = config.scroll;
            }
            _ => {}
        }
    }
    Ok(false)
}

fn preference_action(
    action: DashboardAction,
    state: &PreferencesState,
) -> Option<PreferenceAction> {
    match action {
        DashboardAction::Escape => Some(if state.confirm_reset || state.confirm_transition_clear {
            PreferenceAction::Decline
        } else {
            PreferenceAction::Cancel
        }),
        DashboardAction::ScrollDown => Some(PreferenceAction::FocusDown),
        DashboardAction::ScrollUp => Some(PreferenceAction::FocusUp),
        DashboardAction::PageDown => Some(PreferenceAction::PageDown),
        DashboardAction::PageUp => Some(PreferenceAction::PageUp),
        DashboardAction::Previous => Some(PreferenceAction::Previous),
        DashboardAction::Next => Some(PreferenceAction::Next),
        DashboardAction::Toggle => Some(PreferenceAction::Toggle),
        DashboardAction::Activate => Some(PreferenceAction::Activate),
        DashboardAction::MoveUp => Some(PreferenceAction::MoveUp),
        DashboardAction::MoveDown => Some(PreferenceAction::MoveDown),
        DashboardAction::Save => Some(PreferenceAction::Save),
        DashboardAction::Cancel => Some(PreferenceAction::Cancel),
        DashboardAction::Reset => Some(PreferenceAction::Reset),
        DashboardAction::Confirm => Some(PreferenceAction::Confirm),
        DashboardAction::Decline => Some(PreferenceAction::Decline),
        _ => None,
    }
}

fn handle_preference(action: DashboardAction, shared: &Arc<Mutex<RuntimeState>>) {
    let mut state = lock(shared);
    let Some(current) = state.preferences.take() else {
        return;
    };
    let Some(action) = preference_action(action, &current) else {
        state.preferences = Some(current);
        return;
    };
    let page_size = usize::from(state.terminal_height.saturating_sub(2).max(1));
    let update = preferences::reduce(current, action, page_size);
    state.preferences = Some(update.state);
    match update.command {
        PreferenceCommand::None => {}
        PreferenceCommand::Cancel => state.preferences = None,
        PreferenceCommand::Save => save_preferences(&mut state),
        PreferenceCommand::ClearTransitions => clear_transitions(&mut state),
    }
}

fn save_preferences(state: &mut RuntimeState) {
    let Some(mut preferences) = state.preferences.take() else {
        return;
    };
    preferences.saving = true;
    preferences.notice = None;
    let previous = state.settings.clone();
    let saved = state
        .settings_store
        .as_ref()
        .is_some_and(|store| store.save(&preferences.draft).is_ok());
    if !saved {
        preferences.saving = false;
        preferences.notice = Some(PreferenceNotice::SaveFailed);
        state.preferences = Some(preferences);
        return;
    }
    state.settings = preferences.draft;
    state.settings_availability = SettingsAvailability::Ready;
    let settings = transition_settings(&state.settings);
    let history = state.history.current().cloned();
    let (channels, providers) = baseline_scope(&previous, &state.settings);
    state.transitions = if !transitions::transition_policy_enabled(&settings) {
        empty_transitions(TransitionAvailability::Ready)
    } else if let (Some(store), Some(history)) = (state.transition_store.as_ref(), history.as_ref())
    {
        if channels.is_empty() {
            store.load_view(&transition_history(history), &settings)
        } else {
            store.baseline(
                &transition_history(history),
                &settings,
                &channels,
                providers.as_deref(),
            )
        }
    } else {
        empty_transitions(TransitionAvailability::Unavailable)
    };
    state.transition_review = false;
    state.preferences = None;
    state.scroll = 0;
}

fn baseline_scope(
    previous: &DashboardSettings,
    next: &DashboardSettings,
) -> (Vec<TransitionChannel>, Option<Vec<MarketedProvider>>) {
    let visible = previous
        .hidden_providers
        .iter()
        .filter(|provider| !next.hidden_providers.contains(provider))
        .filter_map(|provider| MarketedProvider::from_id(provider.id()))
        .collect::<Vec<_>>();
    let threshold_changed = previous.remaining_threshold != next.remaining_threshold;
    let forecast_changed = previous.forecast_before_reset != next.forecast_before_reset;
    let mut channels = Vec::new();
    if !visible.is_empty() || threshold_changed {
        channels.push(TransitionChannel::Threshold);
    }
    if !visible.is_empty() || forecast_changed {
        channels.push(TransitionChannel::Forecast);
    }
    let providers =
        (!visible.is_empty() && !threshold_changed && !forecast_changed).then_some(visible);
    (channels, providers)
}

fn acknowledge(state: &Arc<Mutex<RuntimeState>>) {
    let mut state = lock(state);
    let history = state.history.current().cloned();
    let settings = transition_settings(&state.settings);
    if let (Some(store), Some(history)) = (state.transition_store.as_ref(), history) {
        let now = DateTime::<Utc>::from(SystemTime::now());
        state.transitions = store.acknowledge(&transition_history(&history), &settings, now);
        if state.transitions.events.is_empty() {
            state.transition_review = false;
        }
    }
}

fn clear_transitions(state: &mut RuntimeState) {
    let Some(mut preferences) = state.preferences.take() else {
        return;
    };
    preferences.saving = true;
    preferences.notice = None;
    let mut view = state
        .transition_store
        .as_ref()
        .map(TransitionStore::clear)
        .unwrap_or_else(|| empty_transitions(TransitionAvailability::Unavailable));
    let history = state.history.current().cloned();
    let settings = transition_settings(&state.settings);
    if !matches!(
        view.availability,
        TransitionAvailability::Unavailable | TransitionAvailability::Incompatible
    ) && transitions::transition_policy_enabled(&settings)
        && let (Some(store), Some(history)) = (state.transition_store.as_ref(), history)
    {
        view = store.baseline(
            &transition_history(&history),
            &settings,
            &[TransitionChannel::Threshold, TransitionChannel::Forecast],
            None,
        );
    }
    let failed = matches!(
        view.availability,
        TransitionAvailability::Unavailable | TransitionAvailability::Incompatible
    );
    state.transitions = view;
    state.transition_review = false;
    preferences.confirm_transition_clear = false;
    preferences.saving = false;
    preferences.notice = Some(if failed {
        PreferenceNotice::TransitionClearFailed
    } else {
        PreferenceNotice::TransitionHistoryCleared
    });
    state.preferences = Some(preferences);
}

fn draw<W: io::Write>(
    terminal: &mut Terminal<CrosstermBackend<&mut W>>,
    shared: &Arc<Mutex<RuntimeState>>,
) -> io::Result<()> {
    let area = terminal.size()?;
    let mut state = lock(shared);
    state.terminal_height = area.height;
    if state.preferences.is_none() && !state.transition_review {
        let config = dashboard_config(&state);
        if let Some((provider_count, scroll)) = state.app.report.as_ref().map(|report| {
            (
                visible_provider_count(report, &config),
                clamp_scroll(report, area.width, area.height, &config),
            )
        }) {
            state.selected_provider = state
                .selected_provider
                .min(provider_count.saturating_sub(1));
            state.scroll = scroll;
        }
    }
    terminal.draw(|frame| {
        if let Some(preferences) = state.preferences.as_ref() {
            frame.render_widget(
                Paragraph::new(preference_lines(
                    preferences,
                    state.app.report.as_ref(),
                    frame.area().width,
                    frame.area().height,
                )),
                frame.area(),
            );
        } else if state.transition_review {
            frame.render_widget(
                Paragraph::new(transition_lines(&state.transitions)),
                frame.area(),
            );
        } else {
            let config = dashboard_config(&state);
            draw_dashboard(frame, state.app.report.as_ref(), &config);
        }
    })?;
    Ok(())
}

fn dashboard_config(state: &RuntimeState) -> DashboardConfig {
    DashboardConfig {
        user_hidden: state
            .settings
            .hidden_providers
            .iter()
            .map(|provider| provider.id().to_owned())
            .collect::<BTreeSet<_>>(),
        scroll: state.scroll,
        color: std::env::var_os("NO_COLOR").is_none(),
        provider_order: state
            .settings
            .provider_order
            .iter()
            .map(|provider| provider.id().to_owned())
            .collect(),
        meter_mode: match state.settings.meter_mode {
            SettingsMeterMode::Remaining => MeterMode::Remaining,
            SettingsMeterMode::Used => MeterMode::Used,
        },
        interactive: true,
        first_run: state.settings_availability == SettingsAvailability::FirstRun,
        transition_count: state.transitions.events.len(),
        status: dashboard_status(&state.app),
        retry_minutes: retry_minutes(&state.app),
        history: display_history(&state.history, state.history_matches_report),
        view: state.view,
        selected_provider: state.selected_provider,
        startup_view: state.settings.startup_view,
        ..DashboardConfig::default()
    }
}

fn display_history(
    history: &LocalHistory,
    matches_report: bool,
) -> Option<evidence::HistoryDocument> {
    matches_report.then(|| history.current().cloned()).flatten()
}

fn preference_lines(
    state: &PreferencesState,
    report: Option<&QuotaReport>,
    width: u16,
    height: u16,
) -> Vec<Line<'static>> {
    let dirty = if preferences::settings_changed(state) {
        " *"
    } else {
        ""
    };
    if state.confirm_transition_clear {
        return if width >= 32 {
            vec![
                Line::from("Clear transition history?"),
                Line::from("Quota history and settings stay."),
                Line::from("y clear · n/esc back"),
            ]
        } else {
            vec![
                Line::from("Clear history?"),
                Line::from("Quota stays"),
                Line::from("Prefs stay"),
                Line::from("y clear n/esc"),
            ]
        };
    }
    if state.confirm_reset {
        return if width >= 27 {
            vec![
                Line::from("Reset Preferences draft?"),
                Line::from("Nothing changes until save."),
                Line::from("y reset · n/esc back"),
            ]
        } else {
            vec![
                Line::from("Reset draft?"),
                Line::from("Save applies it"),
                Line::from("y reset n/esc"),
            ]
        };
    }
    let order = preferences::focus_order(&state.draft);
    let mut rows = order
        .iter()
        .map(|focus| preference_row(*focus, state, report, width))
        .collect::<Vec<_>>();
    let body = usize::from(height).saturating_sub(3).max(1);
    let focus = order
        .iter()
        .position(|focus| *focus == state.focus)
        .unwrap_or(0);
    let start = focus
        .saturating_sub(body.saturating_sub(1))
        .min(rows.len().saturating_sub(body));
    rows = rows.into_iter().skip(start).take(body).collect();
    let notice = match state.notice {
        Some(PreferenceNotice::SaveFailed) => "? save failed",
        Some(PreferenceNotice::TransitionClearFailed) => "? clear failed",
        Some(PreferenceNotice::TransitionHistoryCleared) if width >= 28 => {
            "= transition history cleared"
        }
        Some(PreferenceNotice::TransitionHistoryCleared) if width >= 17 => "= history cleared",
        Some(PreferenceNotice::TransitionHistoryCleared) => "= cleared",
        None if state.saving => "~ saving",
        None if width >= 27 => "j/k navigate · space change",
        None if width >= 18 => "j/k · space change",
        None => "j/k space",
    };
    std::iter::once(Line::from(format!("Preferences{dirty}")))
        .chain(std::iter::once(Line::from(format!(
            "Item {} of {}",
            focus + 1,
            order.len()
        ))))
        .chain(rows.into_iter().map(Line::from))
        .chain(std::iter::once(Line::from(notice)))
        .collect()
}

fn first_fitting(candidates: impl IntoIterator<Item = String>, width: u16) -> String {
    candidates
        .into_iter()
        .find(|candidate| UnicodeWidthStr::width(candidate.as_str()) <= usize::from(width))
        .unwrap_or_default()
}

fn preference_row(
    focus: PreferenceFocus,
    state: &PreferencesState,
    report: Option<&QuotaReport>,
    width: u16,
) -> String {
    let selected = focus == state.focus;
    let marker = if selected { '>' } else { ' ' };
    let PreferenceFocus::Provider(provider) = focus else {
        return format!(
            "{marker} {}",
            preference_label(focus, &state.draft, width.saturating_sub(2))
        );
    };
    let marketed = marketed(provider);
    let readiness = report
        .map(|report| provider_readiness(report, marketed).text())
        .unwrap_or_else(|| "unsupported".into());
    let visible = !state.draft.hidden_providers.contains(&provider);
    let check = if visible { 'x' } else { ' ' };
    let full = marketed.label();
    let compact = match provider {
        SupportedProvider::Codex => "Codex",
        SupportedProvider::Copilot => "GitHub",
        _ => full,
    };
    let boundary = match provider {
        SupportedProvider::Claude => "C",
        SupportedProvider::Codex => "O",
        SupportedProvider::Cursor => "U",
        SupportedProvider::Kimi => "K",
        SupportedProvider::Grok => "G",
        SupportedProvider::Copilot => "H",
    };
    let single_marker = if selected {
        '>'
    } else if visible {
        'x'
    } else {
        '-'
    };
    first_fitting(
        [
            format!("{marker} [{check}] {full} · {readiness}"),
            format!("{marker} [{check}] {compact} {readiness}"),
            format!("{single_marker}{compact} {readiness}"),
            format!("{single_marker}{boundary} {readiness}"),
            format!("{single_marker}{boundary}"),
        ],
        width,
    )
}

fn preference_label(focus: PreferenceFocus, settings: &DashboardSettings, width: u16) -> String {
    match focus {
        PreferenceFocus::Provider(_) => unreachable!("provider rows include readiness"),
        PreferenceFocus::Meter => format!(
            "Meter: {}",
            match settings.meter_mode {
                SettingsMeterMode::Remaining if width < 18 => "left",
                SettingsMeterMode::Remaining => "remaining",
                SettingsMeterMode::Used => "used",
            }
        ),
        PreferenceFocus::StartupView => format!(
            "{}: {}",
            if width >= 24 {
                "Startup view"
            } else if width >= 19 {
                "Startup"
            } else {
                "View"
            },
            match settings.startup_view {
                StartupView::Overview => "overview",
                StartupView::Details => "details",
            }
        ),
        PreferenceFocus::Threshold => format!(
            "{}: {}",
            if width >= 20 { "Remaining cue" } else { "Cue" },
            match settings.remaining_threshold {
                SettingsThreshold::Off => "off",
                SettingsThreshold::Percent25 => "25%",
                SettingsThreshold::Percent10 => "10%",
                SettingsThreshold::Percent5 => "5%",
            }
        ),
        PreferenceFocus::Forecast => format!(
            "{}: {}",
            if width >= 19 {
                "Forecast cue"
            } else {
                "Forecast"
            },
            if settings.forecast_before_reset {
                "on"
            } else {
                "off"
            }
        ),
        PreferenceFocus::Save => "Save (s)".into(),
        PreferenceFocus::Cancel => "Cancel (c)".into(),
        PreferenceFocus::Reset if width < 17 => "Reset (x)".into(),
        PreferenceFocus::Reset => "Reset draft (x)".into(),
        PreferenceFocus::ClearTransitions if width < 26 => "Clear history".into(),
        PreferenceFocus::ClearTransitions => "Clear transition history".into(),
    }
}

fn marketed(provider: SupportedProvider) -> MarketedProvider {
    MarketedProvider::from_id(provider.id()).expect("settings providers belong to marketed catalog")
}

fn transition_lines(view: &TransitionView) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from("Quota changes"),
        Line::from("Enter/a acknowledge · esc back"),
    ];
    for event in &view.events {
        let kind = match event.kind {
            TransitionKind::ThresholdEnter => "below cue",
            TransitionKind::ThresholdRecovery => "recovered",
            TransitionKind::ForecastEnter => "forecast risk",
            TransitionKind::ForecastRecovery => "forecast recovered",
        };
        let remaining = event
            .remaining
            .map(|value| format!(" · {}%", value.round()))
            .unwrap_or_default();
        lines.push(Line::from(format!(
            "! {} · {kind}{remaining}",
            event.provider.label()
        )));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newly_published_report_suppresses_previous_history_until_recorded() {
        let directory = tempfile::tempdir().expect("temporary history directory");
        let mut history = LocalHistory::new(directory.path().join("history.json"));
        let report = crate::domain::schema::parse_quota_response(include_str!(
            "../../../../test/fixtures/complete.json"
        ))
        .expect("complete fixture");
        history.record_at(&report, SystemTime::UNIX_EPOCH + Duration::from_secs(1_000));

        assert!(display_history(&history, false).is_none());
        assert_eq!(display_history(&history, true), history.current().cloned());
    }

    #[test]
    fn settings_projection_cannot_add_provider_or_policy_fields() {
        let settings = DashboardSettings {
            hidden_providers: vec![SupportedProvider::Claude],
            remaining_threshold: SettingsThreshold::Percent10,
            forecast_before_reset: true,
            ..DashboardSettings::default()
        };
        let projected = transition_settings(&settings);
        assert_eq!(projected.hidden_providers, [MarketedProvider::Claude]);
        assert_eq!(projected.remaining_threshold, RemainingThreshold::Percent10);
        assert!(projected.forecast_before_reset);
    }

    #[test]
    fn escape_cancels_preferences_but_quit_remains_global() {
        let state = preferences::open(&DashboardSettings::default());
        assert_eq!(
            preference_action(DashboardAction::Escape, &state),
            Some(PreferenceAction::Cancel)
        );
        assert_eq!(preference_action(DashboardAction::Quit, &state), None);
        let state = PreferencesState {
            confirm_reset: true,
            ..state
        };
        assert_eq!(
            preference_action(DashboardAction::Escape, &state),
            Some(PreferenceAction::Decline)
        );
    }

    #[test]
    fn preference_view_keeps_focused_action_reachable_at_narrow_height() {
        let mut state = preferences::open(&DashboardSettings::default());
        state.focus = PreferenceFocus::ClearTransitions;
        let text = preference_lines(&state, None, 36, 8)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("> Clear transition history"));
    }

    #[test]
    fn startup_view_projects_into_the_live_dashboard_and_preferences() {
        assert_eq!(
            startup_dashboard_view(StartupView::Overview),
            DashboardView::Overview
        );
        assert_eq!(
            startup_dashboard_view(StartupView::Details),
            DashboardView::Details
        );
        let mut preferences = preferences::open(&DashboardSettings::default());
        preferences.focus = PreferenceFocus::StartupView;
        let text = preference_lines(&preferences, None, 36, 12)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("> Startup view: overview"));

        for width in 16..=36 {
            for focus in preferences::focus_order(&preferences.draft) {
                preferences.focus = focus;
                let lines = preference_lines(&preferences, None, width, 12);
                assert!(
                    lines.iter().all(|line| {
                        UnicodeWidthStr::width(line.to_string().as_str()) <= usize::from(width)
                    }),
                    "{width} columns at {focus:?}: {lines:?}"
                );
            }
            for variant in [
                PreferencesState {
                    confirm_reset: true,
                    ..preferences.clone()
                },
                PreferencesState {
                    confirm_transition_clear: true,
                    ..preferences.clone()
                },
                PreferencesState {
                    notice: Some(PreferenceNotice::TransitionHistoryCleared),
                    ..preferences.clone()
                },
            ] {
                let lines = preference_lines(&variant, None, width, 12);
                assert!(
                    lines.iter().all(|line| {
                        UnicodeWidthStr::width(line.to_string().as_str()) <= usize::from(width)
                    }),
                    "{width} columns: {lines:?}"
                );
            }
            preferences.focus = PreferenceFocus::StartupView;
            let lines = preference_lines(&preferences, None, width, 12);
            let text = lines
                .into_iter()
                .map(|line| line.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            assert!(text.contains("overview"));
            assert!(!text.contains(" over\n"));
        }
    }

    #[test]
    fn preferences_lists_every_marketed_provider_with_the_same_finite_readiness_cues() {
        use crate::domain::provider::{ProviderStatus, SemanticsStatus};

        let mut report = crate::domain::schema::parse_quota_response(include_str!(
            "../../../../test/fixtures/launch.json"
        ))
        .expect("sanitized launch fixture");
        let codex = report
            .providers
            .iter_mut()
            .find(|provider| provider.provider == "codex")
            .expect("Codex");
        codex.state.status = ProviderStatus::AuthRequired;
        codex.state.auth_status = Some("unusable".into());
        let cursor = report
            .providers
            .iter_mut()
            .find(|provider| provider.provider == "cursor")
            .expect("Cursor");
        cursor.state.status = ProviderStatus::Stale;
        cursor.state.stale = true;
        cursor.state.refreshed_at = Some("2026-09-03T10:00:00Z".into());
        let kimi = report
            .providers
            .iter_mut()
            .find(|provider| provider.provider == "kimi")
            .expect("Kimi");
        kimi.semantics_status = Some(SemanticsStatus::Partial);
        let grok = report
            .providers
            .iter_mut()
            .find(|provider| provider.provider == "grok")
            .expect("Grok");
        grok.state.status = ProviderStatus::Fresh;
        grok.state.auth_status = Some("usable".into());
        report
            .providers
            .retain(|provider| provider.provider != "copilot");
        let mut future = report.providers[0].clone();
        future.provider = "future-lab".into();
        report.providers.push(future);

        let mut preferences = preferences::open(&DashboardSettings::default());
        let cases = [
            (SupportedProvider::Claude, "Claude", "live"),
            (SupportedProvider::Codex, "Codex", "auth"),
            (SupportedProvider::Cursor, "Cursor", "stale 2h"),
            (SupportedProvider::Kimi, "Kimi", "partial"),
            (SupportedProvider::Grok, "G", "quota unavailable"),
            (SupportedProvider::Copilot, "GitHub", "unsupported"),
        ];
        for width in [36, 20] {
            for (provider, name, readiness) in cases {
                preferences.focus = PreferenceFocus::Provider(provider);
                let text = preference_lines(&preferences, Some(&report), width, 12)
                    .into_iter()
                    .map(|line| line.to_string())
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(text.contains(name), "{width}: {text}");
                assert!(text.contains(readiness), "{width}: {text}");
                assert!(!text.contains("Users"));
                assert!(!text.contains("account"));
                assert!(!text.contains("auth.json"));
                assert!(!text.contains("future-lab"));
            }
        }
    }

    #[test]
    fn runtime_projects_failure_kind_and_retry_without_raw_collector_data() {
        let app = AppState {
            loading: false,
            failure: Some(crate::app_state::CollectorFailure {
                kind: CollectorFailureKind::Timeout,
                retry_at: Some(Instant::now() + Duration::from_secs(10 * 60)),
            }),
            ..AppState::default()
        };
        assert_eq!(dashboard_status(&app), Some(DashboardStatus::Timeout));
        assert_eq!(retry_minutes(&app), Some(10));
    }
}
