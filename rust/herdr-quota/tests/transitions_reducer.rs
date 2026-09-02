use herdr_quota::domain::provider::MarketedProvider;
use herdr_quota::store::transitions::{
    HistoryDataHealth, HistoryFact, HistoryProjectionConfidence, HistoryProviderSnapshot,
    HistoryRunway, HistoryRunwayState, HistorySnapshot, PersistedTransitionKind,
    RemainingThreshold, TRANSITION_CLOCK_SKEW_MILLIS, TransitionChannel, TransitionDocument,
    TransitionHistory, TransitionSettings, append_transition_events, baseline_transitions,
    evaluate_transitions, parse_transition_document, transition_view,
};

const START: i64 = 1_787_227_200_000;
const RESET: &str = "2026-08-27T12:00:00.000Z";

#[derive(Clone, Copy)]
struct PointOptions {
    reset_at: Option<&'static str>,
    runway: HistoryRunwayState,
    confidence: Option<HistoryProjectionConfidence>,
    health: HistoryDataHealth,
    auth_eligible: bool,
    present: bool,
}

impl Default for PointOptions {
    fn default() -> Self {
        Self {
            reset_at: Some(RESET),
            runway: HistoryRunwayState::ThroughReset,
            confidence: None,
            health: HistoryDataHealth::Current,
            auth_eligible: true,
            present: true,
        }
    }
}

fn timestamp(minute: i64) -> String {
    chrono::DateTime::from_timestamp_millis(START + minute * 60_000)
        .expect("test timestamp")
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn provider_point(
    provider: MarketedProvider,
    _minute: i64,
    remaining: f64,
    options: PointOptions,
) -> HistoryProviderSnapshot {
    let facts =
        if options.present && options.health == HistoryDataHealth::Current && options.auth_eligible
        {
            vec![HistoryFact {
                scope: "All models".to_owned(),
                limit: Some("Week".to_owned()),
                remaining,
                reset_at: options.reset_at.map(str::to_owned),
                runway: Some(HistoryRunway {
                    state: options.runway,
                    confidence: options.confidence,
                }),
            }]
        } else {
            Vec::new()
        };
    HistoryProviderSnapshot {
        provider,
        data_health: options.health,
        auth_eligible: options.auth_eligible,
        facts,
    }
}

fn point(minute: i64, remaining: f64, options: PointOptions) -> HistorySnapshot {
    HistorySnapshot {
        captured_at: timestamp(minute),
        providers: if options.present {
            vec![provider_point(
                MarketedProvider::Codex,
                minute,
                remaining,
                options,
            )]
        } else {
            Vec::new()
        },
    }
}

fn history(snapshots: Vec<HistorySnapshot>) -> TransitionHistory {
    TransitionHistory { snapshots }
}

fn threshold_settings() -> TransitionSettings {
    TransitionSettings {
        hidden_providers: Vec::new(),
        remaining_threshold: RemainingThreshold::Percent25,
        forecast_before_reset: false,
    }
}

fn actionable_kinds(
    evaluation: &herdr_quota::store::transitions::TransitionEvaluation,
) -> Vec<PersistedTransitionKind> {
    evaluation
        .generated
        .iter()
        .filter(|event| {
            !matches!(
                event.kind,
                PersistedTransitionKind::ThresholdBaseline
                    | PersistedTransitionKind::ForecastBaseline
            )
        })
        .map(|event| event.kind)
        .collect()
}

#[test]
fn threshold_reducer_baselines_crosses_dedupes_recovers_and_recrosses() {
    let settings = threshold_settings();
    let mut samples = vec![point(0, 40.0, PointOptions::default())];
    let first = evaluate_transitions(
        &TransitionDocument::default(),
        &history(samples.clone()),
        &settings,
    );
    assert_eq!(
        first.generated[0].kind,
        PersistedTransitionKind::ThresholdBaseline
    );
    let mut document = first.document;

    samples.push(point(5, 23.0, PointOptions::default()));
    let entered = evaluate_transitions(&document, &history(samples.clone()), &settings);
    assert_eq!(
        actionable_kinds(&entered),
        [PersistedTransitionKind::ThresholdEnter]
    );
    document = entered.document;
    assert!(
        evaluate_transitions(&document, &history(samples.clone()), &settings)
            .generated
            .is_empty()
    );

    samples.push(point(10, 31.0, PointOptions::default()));
    let recovered = evaluate_transitions(&document, &history(samples.clone()), &settings);
    assert_eq!(
        actionable_kinds(&recovered),
        [PersistedTransitionKind::ThresholdRecovery]
    );
    document = recovered.document;

    samples.push(point(15, 20.0, PointOptions::default()));
    assert_eq!(
        actionable_kinds(&evaluate_transitions(
            &document,
            &history(samples),
            &settings
        )),
        [PersistedTransitionKind::ThresholdEnter]
    );
}

#[test]
fn forecast_uses_only_established_or_exhausted_authoritative_state() {
    let settings = TransitionSettings {
        hidden_providers: Vec::new(),
        remaining_threshold: RemainingThreshold::Off,
        forecast_before_reset: true,
    };
    let mut samples = vec![point(0, 60.0, PointOptions::default())];
    let mut document = evaluate_transitions(
        &TransitionDocument::default(),
        &history(samples.clone()),
        &settings,
    )
    .document;

    samples.push(point(
        5,
        55.0,
        PointOptions {
            runway: HistoryRunwayState::ProjectedExhaustion,
            confidence: Some(HistoryProjectionConfidence::Early),
            ..PointOptions::default()
        },
    ));
    assert!(
        evaluate_transitions(&document, &history(samples.clone()), &settings)
            .generated
            .is_empty()
    );

    samples.push(point(
        10,
        50.0,
        PointOptions {
            runway: HistoryRunwayState::ProjectedExhaustion,
            confidence: Some(HistoryProjectionConfidence::Established),
            ..PointOptions::default()
        },
    ));
    let entered = evaluate_transitions(&document, &history(samples.clone()), &settings);
    assert_eq!(
        actionable_kinds(&entered),
        [PersistedTransitionKind::ForecastEnter]
    );
    document = entered.document;

    samples.push(point(15, 48.0, PointOptions::default()));
    assert_eq!(
        actionable_kinds(&evaluate_transitions(
            &document,
            &history(samples),
            &settings
        )),
        [PersistedTransitionKind::ForecastRecovery]
    );
}

#[test]
fn policy_visibility_and_reset_changes_establish_scoped_baselines() {
    let settings = threshold_settings();
    let first = HistorySnapshot {
        captured_at: timestamp(0),
        providers: vec![
            provider_point(MarketedProvider::Codex, 0, 30.0, PointOptions::default()),
            provider_point(MarketedProvider::Claude, 0, 30.0, PointOptions::default()),
        ],
    };
    let current = HistorySnapshot {
        captured_at: timestamp(5),
        providers: vec![
            provider_point(MarketedProvider::Codex, 5, 8.0, PointOptions::default()),
            provider_point(MarketedProvider::Claude, 5, 8.0, PointOptions::default()),
        ],
    };
    let samples = history(vec![first.clone(), current.clone()]);
    let document = evaluate_transitions(
        &TransitionDocument::default(),
        &history(vec![first]),
        &settings,
    )
    .document;
    let document = evaluate_transitions(&document, &samples, &settings).document;

    let tightened = TransitionSettings {
        remaining_threshold: RemainingThreshold::Percent10,
        ..settings.clone()
    };
    let tightened_baseline = baseline_transitions(
        &document,
        &samples,
        &tightened,
        &[TransitionChannel::Threshold],
        None,
    );
    assert!(actionable_kinds(&tightened_baseline).is_empty());
    assert!(
        evaluate_transitions(&tightened_baseline.document, &samples, &tightened)
            .generated
            .is_empty()
    );

    let hidden = TransitionSettings {
        hidden_providers: vec![MarketedProvider::Claude],
        ..settings.clone()
    };
    let hidden_baseline = baseline_transitions(
        &tightened_baseline.document,
        &samples,
        &hidden,
        &[TransitionChannel::Threshold],
        Some(&[MarketedProvider::Claude]),
    );
    assert!(hidden_baseline.generated.is_empty());

    let shown = baseline_transitions(
        &tightened_baseline.document,
        &samples,
        &settings,
        &[TransitionChannel::Threshold],
        Some(&[MarketedProvider::Claude]),
    );
    assert!(
        shown
            .generated
            .iter()
            .all(|event| event.provider == MarketedProvider::Claude
                && event.kind == PersistedTransitionKind::ThresholdBaseline)
    );
    assert!(
        evaluate_transitions(&shown.document, &samples, &settings)
            .generated
            .is_empty()
    );

    let next_reset = "2026-09-03T12:00:00.000Z";
    let reset = history(vec![
        current,
        point(
            10,
            100.0,
            PointOptions {
                reset_at: Some(next_reset),
                ..PointOptions::default()
            },
        ),
    ]);
    let reset_update = evaluate_transitions(&shown.document, &reset, &settings);
    assert!(actionable_kinds(&reset_update).is_empty());
    assert_eq!(
        reset_update.generated.last().map(|event| event.kind),
        Some(PersistedTransitionKind::ThresholdBaseline)
    );
}

#[test]
fn gaps_bridge_same_cycle_but_never_fabricate_or_cross_reset_cycles() {
    for health in [
        HistoryDataHealth::Stale,
        HistoryDataHealth::Unavailable,
        HistoryDataHealth::Error,
        HistoryDataHealth::Unknown,
    ] {
        let settings = threshold_settings();
        let initial = history(vec![point(0, 40.0, PointOptions::default())]);
        let document =
            evaluate_transitions(&TransitionDocument::default(), &initial, &settings).document;
        let gap = point(
            5,
            0.0,
            PointOptions {
                health,
                ..PointOptions::default()
            },
        );
        let during = evaluate_transitions(
            &document,
            &history(vec![point(0, 40.0, PointOptions::default()), gap.clone()]),
            &settings,
        );
        assert!(during.generated.is_empty(), "{health:?}");

        let caught_up = evaluate_transitions(
            &document,
            &history(vec![
                point(0, 40.0, PointOptions::default()),
                gap,
                point(10, 20.0, PointOptions::default()),
            ]),
            &settings,
        );
        assert_eq!(
            actionable_kinds(&caught_up),
            [PersistedTransitionKind::ThresholdEnter],
            "{health:?}"
        );
    }

    let settings = threshold_settings();
    let initial = history(vec![point(0, 40.0, PointOptions::default())]);
    let document =
        evaluate_transitions(&TransitionDocument::default(), &initial, &settings).document;
    let entered = evaluate_transitions(
        &document,
        &history(vec![
            point(0, 40.0, PointOptions::default()),
            point(5, 20.0, PointOptions::default()),
        ]),
        &settings,
    )
    .document;
    let after_reset = evaluate_transitions(
        &entered,
        &history(vec![
            point(0, 40.0, PointOptions::default()),
            point(5, 20.0, PointOptions::default()),
            point(
                10,
                0.0,
                PointOptions {
                    health: HistoryDataHealth::Error,
                    ..PointOptions::default()
                },
            ),
            point(
                15,
                100.0,
                PointOptions {
                    reset_at: Some("2026-09-03T12:00:00.000Z"),
                    ..PointOptions::default()
                },
            ),
        ]),
        &settings,
    );
    assert!(actionable_kinds(&after_reset).is_empty());
    assert_eq!(
        after_reset.generated.last().map(|event| event.kind),
        Some(PersistedTransitionKind::ThresholdBaseline)
    );
}

#[test]
fn pane_reopen_dedupes_retained_history_catch_up_and_new_baseline_archives_review() {
    let settings = threshold_settings();
    let first = history(vec![point(0, 40.0, PointOptions::default())]);
    let baseline = evaluate_transitions(&TransitionDocument::default(), &first, &settings).document;
    let serialized = serde_json::to_value(&baseline).expect("serialize baseline");
    let reopened = parse_transition_document(&serialized).expect("parse reopened state");
    let retained = history(vec![
        point(0, 40.0, PointOptions::default()),
        point(30, 20.0, PointOptions::default()),
    ]);
    let caught_up = evaluate_transitions(&reopened, &retained, &settings);
    assert_eq!(
        actionable_kinds(&caught_up),
        [PersistedTransitionKind::ThresholdEnter]
    );
    let reopened_again = parse_transition_document(
        &serde_json::to_value(&caught_up.document).expect("serialize catch-up"),
    )
    .expect("parse catch-up state");
    assert!(
        evaluate_transitions(&reopened_again, &retained, &settings)
            .generated
            .is_empty()
    );
    let view = transition_view(
        &reopened_again,
        &retained,
        &settings,
        herdr_quota::store::transitions::TransitionAvailability::Ready,
    );
    assert_eq!(view.events.len(), 1);
    assert_eq!(view.events[0].remaining, Some(20.0));

    let archived = baseline_transitions(
        &reopened_again,
        &retained,
        &settings,
        &[TransitionChannel::Threshold],
        None,
    );
    assert!(
        transition_view(
            &archived.document,
            &retained,
            &settings,
            herdr_quota::store::transitions::TransitionAvailability::Ready,
        )
        .events
        .is_empty()
    );
}

#[test]
fn reset_jitter_and_large_clock_rollback_have_bounded_deterministic_identity() {
    let settings = threshold_settings();
    let first_reset = "2026-08-27T12:00:00.145Z";
    let jittered_reset = "2026-08-27T12:00:00.147Z";
    let baseline = evaluate_transitions(
        &TransitionDocument::default(),
        &history(vec![point(
            0,
            40.0,
            PointOptions {
                reset_at: Some(first_reset),
                ..PointOptions::default()
            },
        )]),
        &settings,
    )
    .document;
    let crossed = evaluate_transitions(
        &baseline,
        &history(vec![
            point(
                0,
                40.0,
                PointOptions {
                    reset_at: Some(first_reset),
                    ..PointOptions::default()
                },
            ),
            point(
                5,
                20.0,
                PointOptions {
                    reset_at: Some(jittered_reset),
                    ..PointOptions::default()
                },
            ),
        ]),
        &settings,
    );
    assert_eq!(
        crossed.document.events.last().expect("crossing").cycle,
        "2026-08-27T12:00:00.000Z"
    );

    let mut earlier = crossed.document.events[0].clone();
    earlier.occurred_at =
        chrono::DateTime::from_timestamp_millis(START - TRANSITION_CLOCK_SKEW_MILLIS - 1)
            .expect("rollback timestamp")
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    earlier.cycle = "unbounded".to_owned();
    let segmented = append_transition_events(&crossed.document, vec![earlier.clone()]);
    assert!(segmented.clock_skew);
    assert_eq!(segmented.document.events, [earlier]);
}
