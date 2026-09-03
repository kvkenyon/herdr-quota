//! Pane-owned live dashboard runtime.

use std::collections::BTreeSet;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Utc};
use ratatui::{Terminal, backend::CrosstermBackend, text::Line, widgets::Paragraph};
use tokio::sync::mpsc;

use crate::app_state::{self, AppAction, AppState, CollectorFailureKind};
use crate::collector::{Collector, CollectorFailure};
use crate::domain::{
    history_evidence as evidence,
    provider::{MarketedProvider, ProjectionConfidence},
    schema::QuotaReport,
};
use crate::scheduler::{self, RefreshAttempt, RefreshWorker};
use crate::store::{
    history::LocalHistory,
    settings::{
        DashboardSettings, MeterMode as SettingsMeterMode, RemainingThreshold as SettingsThreshold,
        SettingsAvailability, SettingsStore, SupportedProvider,
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
    render::{DashboardConfig, DashboardStatus, clamp_scroll, draw_dashboard},
    terminal::{RawInput, TerminalGuard},
};

struct RuntimeState {
    app: AppState,
    settings: DashboardSettings,
    settings_availability: SettingsAvailability,
    settings_store: Option<SettingsStore>,
    history: LocalHistory,
    transition_store: Option<TransitionStore>,
    transitions: TransitionView,
    preferences: Option<PreferencesState>,
    transition_review: bool,
    scroll: usize,
    terminal_height: u16,
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
        Self {
            app: AppState::default(),
            settings: loaded.settings,
            settings_availability: loaded.availability,
            settings_store,
            history,
            transition_store,
            transitions,
            preferences: None,
            transition_review: false,
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
                app_state::reduce(
                    &mut lock(&state).app,
                    AppAction::CollectionSucceeded { report },
                );
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
                state.history.record(&report);
                let history = state.history.current().cloned();
                let settings = transition_settings(&state.settings);
                if let (Some(store), Some(history)) = (state.transition_store.as_ref(), history) {
                    state.transitions = store.evaluate(&transition_history(&history), &settings);
                }
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
            DashboardAction::Escape => return Ok(true),
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
            DashboardAction::ScrollDown => {
                let mut state = lock(shared);
                state.scroll = state.scroll.saturating_add(1);
            }
            DashboardAction::ScrollUp => {
                let mut state = lock(shared);
                state.scroll = state.scroll.saturating_sub(1);
            }
            DashboardAction::PageDown => {
                let mut state = lock(shared);
                state.scroll = state.scroll.saturating_add(8);
            }
            DashboardAction::PageUp => {
                let mut state = lock(shared);
                state.scroll = state.scroll.saturating_sub(8);
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
    if state.preferences.is_none()
        && !state.transition_review
        && let Some(report) = state.app.report.as_ref()
    {
        let config = dashboard_config(&state);
        state.scroll = clamp_scroll(report, area.width, area.height, &config);
    }
    terminal.draw(|frame| {
        if let Some(preferences) = state.preferences.as_ref() {
            frame.render_widget(
                Paragraph::new(preference_lines(preferences, frame.area().height)),
                frame.area(),
            );
        } else if state.transition_review {
            frame.render_widget(
                Paragraph::new(transition_lines(&state.transitions)),
                frame.area(),
            );
        } else if let Some(report) = state.app.report.as_ref() {
            let config = dashboard_config(&state);
            draw_dashboard(frame, report, &config);
        } else {
            let status = match dashboard_status(&state.app) {
                Some(DashboardStatus::Refreshing) => "~ Refreshing quota data",
                Some(DashboardStatus::Timeout) => "? Quota check timed out",
                Some(DashboardStatus::MissingExecutable) => "? quota-axi executable is missing",
                Some(DashboardStatus::IncompatibleOutput) => "? quota-axi output is incompatible",
                Some(DashboardStatus::NetworkProcess) => "? Quota network/process check failed",
                None => "? Quota data unavailable",
            };
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from("Herdr Quota"),
                    Line::from(status),
                    Line::from(""),
                    Line::from("r refresh · p Preferences · q quit"),
                ]),
                frame.area(),
            );
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
        transition_count: state.transitions.events.len(),
        status: dashboard_status(&state.app),
    }
}

fn preference_lines(state: &PreferencesState, height: u16) -> Vec<Line<'static>> {
    let dirty = preferences::settings_changed(state)
        .then_some(" *")
        .unwrap_or("");
    if state.confirm_transition_clear {
        return vec![
            Line::from("Clear transition history?"),
            Line::from("Quota history and settings stay."),
            Line::from("y clear · n/esc back"),
        ];
    }
    if state.confirm_reset {
        return vec![
            Line::from("Reset Preferences draft?"),
            Line::from("Nothing changes until save."),
            Line::from("y reset · n/esc back"),
        ];
    }
    let order = preferences::focus_order(&state.draft);
    let mut rows = order
        .iter()
        .map(|focus| {
            let marker = if *focus == state.focus { ">" } else { " " };
            format!("{marker} {}", preference_label(*focus, &state.draft))
        })
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
        Some(PreferenceNotice::TransitionHistoryCleared) => "= transition history cleared",
        None if state.saving => "~ saving",
        None => "j/k navigate · space change",
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

fn preference_label(focus: PreferenceFocus, settings: &DashboardSettings) -> String {
    match focus {
        PreferenceFocus::Provider(provider) => format!(
            "[{}] {}",
            if settings.hidden_providers.contains(&provider) {
                " "
            } else {
                "x"
            },
            marketed(provider).label()
        ),
        PreferenceFocus::Meter => format!(
            "Meter: {}",
            match settings.meter_mode {
                SettingsMeterMode::Remaining => "remaining",
                SettingsMeterMode::Used => "used",
            }
        ),
        PreferenceFocus::Threshold => format!(
            "Remaining cue: {}",
            match settings.remaining_threshold {
                SettingsThreshold::Off => "off",
                SettingsThreshold::Percent25 => "25%",
                SettingsThreshold::Percent10 => "10%",
                SettingsThreshold::Percent5 => "5%",
            }
        ),
        PreferenceFocus::Forecast => format!(
            "Forecast cue: {}",
            if settings.forecast_before_reset {
                "on"
            } else {
                "off"
            }
        ),
        PreferenceFocus::Save => "Save (s)".into(),
        PreferenceFocus::Cancel => "Cancel (c)".into(),
        PreferenceFocus::Reset => "Reset draft (x)".into(),
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
        let text = preference_lines(&state, 8)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("> Clear transition history"));
    }
}
