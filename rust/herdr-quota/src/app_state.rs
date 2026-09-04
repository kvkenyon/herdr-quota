//! Product-owned dashboard state and its side-effect-free reducer.
//!
//! The scheduler and future terminal renderer communicate through these
//! actions.  Collection and persistence stay outside this module so they can
//! be wired independently without giving a collector ownership of UI state.

use std::time::{Duration, Instant};

use crate::domain::schema::QuotaReport;

/// The finite, safe categories for a whole-collector failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CollectorFailureKind {
    Timeout,
    MissingExecutable,
    IncompatibleOutput,
    NetworkProcess,
}

/// Display state for a whole-collector failure.
///
/// It deliberately has no raw process error, path, endpoint, or provider
/// payload. Those details remain behind the collector boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectorFailure {
    pub kind: CollectorFailureKind,
    pub retry_at: Option<Instant>,
}

impl CollectorFailure {
    /// Create a failure before its retry has been scheduled.
    pub const fn new(kind: CollectorFailureKind) -> Self {
        Self {
            kind,
            retry_at: None,
        }
    }
}

/// The subset of dashboard state owned before the renderer and persistence
/// ports land. It retains the last useful report across a collection failure.
#[derive(Clone, Debug, PartialEq)]
pub struct AppState {
    pub report: Option<QuotaReport>,
    pub loading: bool,
    pub failure: Option<CollectorFailure>,
    pub last_attempt_at: Option<Instant>,
    /// Advances only for a 30-second age-only redraw. It never starts work or
    /// changes quota data.
    pub age_tick: u64,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            report: None,
            loading: true,
            failure: None,
            last_attempt_at: None,
            age_tick: 0,
        }
    }
}

/// A state transition requested by the scheduler or terminal loop.
#[derive(Clone, Debug, PartialEq)]
pub enum AppAction {
    RefreshStarted {
        at: Instant,
    },
    CollectionSucceeded {
        report: QuotaReport,
    },
    CollectionFailed {
        kind: CollectorFailureKind,
    },
    RefreshScheduled {
        at: Instant,
        delay: Duration,
        after_failure: bool,
    },
    RefreshSettled,
    AgeTick,
}

/// Apply a dashboard action and return whether the terminal should redraw.
///
/// All current actions affect visible state, including age-only ticks; callers
/// may coalesce redraws, but must not turn an age tick into a collection.
pub fn reduce(state: &mut AppState, action: AppAction) -> bool {
    match action {
        AppAction::RefreshStarted { at } => {
            state.loading = true;
            state.failure = None;
            state.last_attempt_at = Some(at);
        }
        AppAction::CollectionSucceeded { report } => {
            state.report = Some(report);
            state.failure = None;
        }
        AppAction::CollectionFailed { kind } => {
            state.failure = Some(CollectorFailure::new(kind));
        }
        AppAction::RefreshScheduled {
            at,
            delay,
            after_failure,
        } => {
            if after_failure && let Some(failure) = state.failure.as_mut() {
                failure.retry_at = Some(at + delay);
            }
        }
        AppAction::RefreshSettled => state.loading = false,
        AppAction::AgeTick => state.age_tick = state.age_tick.saturating_add(1),
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{AppAction, AppState, CollectorFailureKind, reduce};
    use crate::domain::schema::QuotaReport;
    use std::time::{Duration, Instant};

    fn report(name: &str) -> QuotaReport {
        QuotaReport {
            generated_at: name.to_owned(),
            schema_version: 5,
            providers: Vec::new(),
            adaptation_warnings: Vec::new(),
        }
    }

    #[test]
    fn collector_failure_keeps_the_last_report_and_exposes_its_retry() {
        let now = Instant::now();
        let previous = report("previous");
        let mut state = AppState {
            report: Some(previous.clone()),
            ..AppState::default()
        };

        reduce(
            &mut state,
            AppAction::CollectionFailed {
                kind: CollectorFailureKind::Timeout,
            },
        );
        reduce(
            &mut state,
            AppAction::RefreshScheduled {
                at: now,
                delay: Duration::from_secs(10 * 60),
                after_failure: true,
            },
        );
        reduce(&mut state, AppAction::RefreshSettled);

        assert_eq!(state.report, Some(previous));
        assert!(!state.loading);
        assert_eq!(
            state.failure.expect("failure is retained").retry_at,
            Some(now + Duration::from_secs(10 * 60))
        );
    }

    #[test]
    fn refresh_start_clears_a_previous_failure_and_records_the_attempt() {
        let now = Instant::now();
        let mut state = AppState::default();
        reduce(
            &mut state,
            AppAction::CollectionFailed {
                kind: CollectorFailureKind::NetworkProcess,
            },
        );

        reduce(&mut state, AppAction::RefreshStarted { at: now });

        assert!(state.loading);
        assert_eq!(state.failure, None);
        assert_eq!(state.last_attempt_at, Some(now));
    }

    #[test]
    fn age_ticks_only_mark_a_redraw_and_preserve_collection_state() {
        let mut state = AppState {
            report: Some(report("fresh")),
            loading: false,
            failure: Some(super::CollectorFailure::new(
                CollectorFailureKind::IncompatibleOutput,
            )),
            ..AppState::default()
        };
        let report = state.report.clone();
        let failure = state.failure.clone();

        assert!(reduce(&mut state, AppAction::AgeTick));

        assert_eq!(state.age_tick, 1);
        assert_eq!(state.report, report);
        assert_eq!(state.failure, failure);
        assert!(!state.loading);
    }
}
