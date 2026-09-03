use herdr_quota::store::settings::{DashboardSettings, MeterMode, SupportedProvider};
use herdr_quota::ui::{
    keys::{DashboardAction, TerminalInputParser},
    preferences::{self, PreferenceAction, PreferenceCommand, PreferenceFocus},
};
use herdr_quota::{Command, parse_command};

#[test]
fn live_entrypoint_and_fragmented_modal_journey_use_one_finite_action_stream() {
    assert_eq!(parse_command(["dashboard"]), Ok(Command::Dashboard));

    let mut parser = TerminalInputParser::default();
    assert_eq!(parser.push(b"p"), [DashboardAction::Preferences]);
    assert!(parser.push(b"\x1b").is_empty());
    assert!(parser.push(b"[").is_empty());
    assert_eq!(parser.push(b"B"), [DashboardAction::ScrollDown]);
    assert_eq!(
        parser.push(b"sxya"),
        [
            DashboardAction::Save,
            DashboardAction::Reset,
            DashboardAction::Confirm,
            DashboardAction::Acknowledge,
        ]
    );
}

#[test]
fn preferences_journey_separates_draft_cancel_save_reset_and_transition_clear() {
    let active = DashboardSettings {
        meter_mode: MeterMode::Remaining,
        ..DashboardSettings::default()
    };
    let mut draft = preferences::open(&active);
    draft.focus = PreferenceFocus::Meter;
    draft = preferences::reduce(draft, PreferenceAction::Toggle, 4).state;
    assert_eq!(draft.draft.meter_mode, MeterMode::Used);
    assert_eq!(
        preferences::reduce(draft.clone(), PreferenceAction::Cancel, 4).command,
        PreferenceCommand::Cancel
    );
    assert_eq!(active.meter_mode, MeterMode::Remaining);
    assert_eq!(
        preferences::reduce(draft.clone(), PreferenceAction::Save, 4).command,
        PreferenceCommand::Save
    );

    draft.focus = PreferenceFocus::Provider(SupportedProvider::Claude);
    draft = preferences::reduce(draft, PreferenceAction::Toggle, 4).state;
    assert_eq!(draft.draft.hidden_providers, [SupportedProvider::Claude]);
    draft = preferences::reduce(draft, PreferenceAction::Reset, 4).state;
    assert!(draft.confirm_reset);
    draft = preferences::reduce(draft, PreferenceAction::Confirm, 4).state;
    assert_eq!(draft.draft, DashboardSettings::default());

    draft.focus = PreferenceFocus::ClearTransitions;
    draft = preferences::reduce(draft, PreferenceAction::Activate, 4).state;
    assert!(draft.confirm_transition_clear);
    assert_eq!(
        preferences::reduce(draft, PreferenceAction::Confirm, 4).command,
        PreferenceCommand::ClearTransitions
    );
}
