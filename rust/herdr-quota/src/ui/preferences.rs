//! Pure reducer for the finite Preferences surface.

use crate::store::settings::{
    DashboardSettings, MeterMode, RemainingThreshold, StartupView, SupportedProvider,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreferenceFocus {
    Provider(SupportedProvider),
    Meter,
    StartupView,
    Threshold,
    Forecast,
    Save,
    Cancel,
    Reset,
    ClearTransitions,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreferencesState {
    pub original: DashboardSettings,
    pub draft: DashboardSettings,
    pub focus: PreferenceFocus,
    pub confirm_reset: bool,
    pub confirm_transition_clear: bool,
    pub saving: bool,
    pub notice: Option<PreferenceNotice>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreferenceNotice {
    SaveFailed,
    TransitionClearFailed,
    TransitionHistoryCleared,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreferenceAction {
    FocusDown,
    FocusUp,
    PageDown,
    PageUp,
    Toggle,
    MoveUp,
    MoveDown,
    Previous,
    Next,
    Activate,
    Save,
    Cancel,
    Reset,
    Confirm,
    Decline,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreferenceCommand {
    None,
    Save,
    Cancel,
    ClearTransitions,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreferenceUpdate {
    pub state: PreferencesState,
    pub command: PreferenceCommand,
}

pub fn focus_order(settings: &DashboardSettings) -> Vec<PreferenceFocus> {
    settings
        .provider_order
        .iter()
        .copied()
        .map(PreferenceFocus::Provider)
        .chain([
            PreferenceFocus::Meter,
            PreferenceFocus::StartupView,
            PreferenceFocus::Threshold,
            PreferenceFocus::Forecast,
            PreferenceFocus::Save,
            PreferenceFocus::Cancel,
            PreferenceFocus::Reset,
            PreferenceFocus::ClearTransitions,
        ])
        .collect()
}

pub fn open(settings: &DashboardSettings) -> PreferencesState {
    PreferencesState {
        original: settings.clone(),
        draft: settings.clone(),
        focus: settings
            .provider_order
            .first()
            .copied()
            .map(PreferenceFocus::Provider)
            .unwrap_or(PreferenceFocus::Meter),
        confirm_reset: false,
        confirm_transition_clear: false,
        saving: false,
        notice: None,
    }
}

fn move_focus(mut state: PreferencesState, amount: isize) -> PreferencesState {
    let order = focus_order(&state.draft);
    let current = order
        .iter()
        .position(|focus| *focus == state.focus)
        .unwrap_or(0);
    let next = current
        .saturating_add_signed(amount)
        .min(order.len().saturating_sub(1));
    state.focus = order[next];
    state.notice = None;
    state
}

fn toggle_focused(mut state: PreferencesState, direction: isize) -> PreferencesState {
    match state.focus {
        PreferenceFocus::Provider(provider) => {
            if let Some(index) = state
                .draft
                .hidden_providers
                .iter()
                .position(|item| *item == provider)
            {
                state.draft.hidden_providers.remove(index);
            } else {
                state.draft.hidden_providers.push(provider);
            }
        }
        PreferenceFocus::Meter => {
            state.draft.meter_mode = match state.draft.meter_mode {
                MeterMode::Remaining => MeterMode::Used,
                MeterMode::Used => MeterMode::Remaining,
            };
        }
        PreferenceFocus::StartupView => {
            state.draft.startup_view = match state.draft.startup_view {
                StartupView::Overview => StartupView::Details,
                StartupView::Details => StartupView::Overview,
            };
        }
        PreferenceFocus::Threshold => {
            const VALUES: [RemainingThreshold; 4] = [
                RemainingThreshold::Off,
                RemainingThreshold::Percent25,
                RemainingThreshold::Percent10,
                RemainingThreshold::Percent5,
            ];
            let current = VALUES
                .iter()
                .position(|value| *value == state.draft.remaining_threshold)
                .unwrap_or(0);
            state.draft.remaining_threshold =
                VALUES[(current as isize + direction).rem_euclid(VALUES.len() as isize) as usize];
        }
        PreferenceFocus::Forecast => {
            state.draft.forecast_before_reset = !state.draft.forecast_before_reset;
        }
        _ => return state,
    }
    state.notice = None;
    state
}

fn move_visible_provider(mut state: PreferencesState, direction: isize) -> PreferencesState {
    let PreferenceFocus::Provider(provider) = state.focus else {
        return state;
    };
    if state.draft.hidden_providers.contains(&provider) {
        return state;
    }
    let visible = state
        .draft
        .provider_order
        .iter()
        .copied()
        .filter(|item| !state.draft.hidden_providers.contains(item))
        .collect::<Vec<_>>();
    let Some(visible_index) = visible.iter().position(|item| *item == provider) else {
        return state;
    };
    let neighbor_index = visible_index as isize + direction;
    if !(0..visible.len() as isize).contains(&neighbor_index) {
        return state;
    }
    let neighbor = visible[neighbor_index as usize];
    let current = state
        .draft
        .provider_order
        .iter()
        .position(|item| *item == provider)
        .expect("focused provider belongs to order");
    let other = state
        .draft
        .provider_order
        .iter()
        .position(|item| *item == neighbor)
        .expect("visible neighbor belongs to order");
    state.draft.provider_order.swap(current, other);
    state.notice = None;
    state
}

fn reset_draft(mut state: PreferencesState) -> PreferencesState {
    state.draft = DashboardSettings::default();
    state.focus = PreferenceFocus::Reset;
    state.confirm_reset = false;
    state.confirm_transition_clear = false;
    state.notice = None;
    state
}

pub fn reduce(
    current: PreferencesState,
    action: PreferenceAction,
    page_size: usize,
) -> PreferenceUpdate {
    if current.saving {
        return PreferenceUpdate {
            state: current,
            command: PreferenceCommand::None,
        };
    }
    if current.confirm_transition_clear {
        return match action {
            PreferenceAction::Confirm => PreferenceUpdate {
                state: PreferencesState {
                    confirm_transition_clear: false,
                    ..current
                },
                command: PreferenceCommand::ClearTransitions,
            },
            PreferenceAction::Decline | PreferenceAction::Cancel => PreferenceUpdate {
                state: PreferencesState {
                    confirm_transition_clear: false,
                    ..current
                },
                command: PreferenceCommand::None,
            },
            _ => PreferenceUpdate {
                state: current,
                command: PreferenceCommand::None,
            },
        };
    }
    if current.confirm_reset {
        return match action {
            PreferenceAction::Confirm => PreferenceUpdate {
                state: reset_draft(current),
                command: PreferenceCommand::None,
            },
            PreferenceAction::Decline | PreferenceAction::Cancel => PreferenceUpdate {
                state: PreferencesState {
                    confirm_reset: false,
                    ..current
                },
                command: PreferenceCommand::None,
            },
            _ => PreferenceUpdate {
                state: current,
                command: PreferenceCommand::None,
            },
        };
    }
    let (state, command) = match action {
        PreferenceAction::FocusDown => (move_focus(current, 1), PreferenceCommand::None),
        PreferenceAction::FocusUp => (move_focus(current, -1), PreferenceCommand::None),
        PreferenceAction::PageDown => (
            move_focus(current, page_size.max(1) as isize),
            PreferenceCommand::None,
        ),
        PreferenceAction::PageUp => (
            move_focus(current, -(page_size.max(1) as isize)),
            PreferenceCommand::None,
        ),
        PreferenceAction::Toggle => (toggle_focused(current, 1), PreferenceCommand::None),
        PreferenceAction::Previous => (toggle_focused(current, -1), PreferenceCommand::None),
        PreferenceAction::Next => (toggle_focused(current, 1), PreferenceCommand::None),
        PreferenceAction::MoveUp => (move_visible_provider(current, -1), PreferenceCommand::None),
        PreferenceAction::MoveDown => (move_visible_provider(current, 1), PreferenceCommand::None),
        PreferenceAction::Save => (current, PreferenceCommand::Save),
        PreferenceAction::Cancel => (current, PreferenceCommand::Cancel),
        PreferenceAction::Reset => (
            PreferencesState {
                focus: PreferenceFocus::Reset,
                confirm_reset: true,
                ..current
            },
            PreferenceCommand::None,
        ),
        PreferenceAction::Activate => match current.focus {
            PreferenceFocus::Save => (current, PreferenceCommand::Save),
            PreferenceFocus::Cancel => (current, PreferenceCommand::Cancel),
            PreferenceFocus::Reset => (
                PreferencesState {
                    confirm_reset: true,
                    ..current
                },
                PreferenceCommand::None,
            ),
            PreferenceFocus::ClearTransitions => (
                PreferencesState {
                    confirm_transition_clear: true,
                    ..current
                },
                PreferenceCommand::None,
            ),
            _ => (toggle_focused(current, 1), PreferenceCommand::None),
        },
        PreferenceAction::Confirm | PreferenceAction::Decline => (current, PreferenceCommand::None),
    };
    PreferenceUpdate { state, command }
}

pub fn settings_changed(state: &PreferencesState) -> bool {
    state.draft != state.original
}

#[cfg(test)]
mod tests {
    use super::*;

    fn focused(mut state: PreferencesState, focus: PreferenceFocus) -> PreferencesState {
        state.focus = focus;
        state
    }

    #[test]
    fn toggles_every_finite_setting_and_moves_only_visible_providers() {
        let mut state = focused(
            open(&DashboardSettings::default()),
            PreferenceFocus::Provider(SupportedProvider::Claude),
        );
        state = reduce(state, PreferenceAction::Toggle, 4).state;
        assert_eq!(state.draft.hidden_providers, [SupportedProvider::Claude]);
        state = focused(state, PreferenceFocus::Provider(SupportedProvider::Cursor));
        state = reduce(state, PreferenceAction::MoveUp, 4).state;
        assert_eq!(
            state.draft.provider_order[..3],
            [
                SupportedProvider::Claude,
                SupportedProvider::Cursor,
                SupportedProvider::Codex
            ]
        );
        state = focused(state, PreferenceFocus::Meter);
        state = reduce(state, PreferenceAction::Next, 4).state;
        assert_eq!(state.draft.meter_mode, MeterMode::Used);
        state = focused(state, PreferenceFocus::StartupView);
        state = reduce(state, PreferenceAction::Activate, 4).state;
        assert_eq!(state.draft.startup_view, StartupView::Details);
        state = focused(state, PreferenceFocus::Threshold);
        state = reduce(state, PreferenceAction::Previous, 4).state;
        assert_eq!(
            state.draft.remaining_threshold,
            RemainingThreshold::Percent5
        );
        state = focused(state, PreferenceFocus::Forecast);
        state = reduce(state, PreferenceAction::Activate, 4).state;
        assert!(state.draft.forecast_before_reset);
    }

    #[test]
    fn confirmations_are_independent_and_cancel_is_non_destructive() {
        let active = DashboardSettings {
            meter_mode: MeterMode::Used,
            ..DashboardSettings::default()
        };
        let state = reduce(open(&active), PreferenceAction::Reset, 4).state;
        assert!(state.confirm_reset);
        let state = reduce(state, PreferenceAction::Decline, 4).state;
        assert_eq!(state.draft, active);
        let state = focused(state, PreferenceFocus::ClearTransitions);
        let state = reduce(state, PreferenceAction::Activate, 4).state;
        assert!(state.confirm_transition_clear);
        assert_eq!(
            reduce(state, PreferenceAction::Confirm, 4).command,
            PreferenceCommand::ClearTransitions
        );
    }

    #[test]
    fn page_navigation_reaches_all_actions() {
        let state = reduce(
            open(&DashboardSettings::default()),
            PreferenceAction::FocusUp,
            4,
        )
        .state;
        assert_eq!(
            state.focus,
            PreferenceFocus::Provider(SupportedProvider::Claude)
        );
        let state = reduce(state, PreferenceAction::PageDown, 6).state;
        assert_eq!(state.focus, PreferenceFocus::Meter);
        let state = reduce(state, PreferenceAction::PageDown, 5).state;
        assert_eq!(state.focus, PreferenceFocus::Cancel);
        let state = reduce(state, PreferenceAction::FocusDown, 4).state;
        assert_eq!(state.focus, PreferenceFocus::Reset);
    }

    #[test]
    fn explicit_save_cancel_reset_and_clear_actions_emit_only_their_commands() {
        let initial = open(&DashboardSettings::default());
        assert_eq!(
            reduce(initial.clone(), PreferenceAction::Save, 4).command,
            PreferenceCommand::Save
        );
        assert_eq!(
            reduce(initial.clone(), PreferenceAction::Cancel, 4).command,
            PreferenceCommand::Cancel
        );
        let reset = reduce(initial.clone(), PreferenceAction::Reset, 4);
        assert_eq!(reset.command, PreferenceCommand::None);
        assert!(reset.state.confirm_reset);
        let clear = focused(initial, PreferenceFocus::ClearTransitions);
        let clear = reduce(clear, PreferenceAction::Activate, 4).state;
        assert_eq!(
            reduce(clear, PreferenceAction::Confirm, 4).command,
            PreferenceCommand::ClearTransitions
        );
    }
}
