//! Selection of the single quota signal that belongs above provider detail.

use std::cmp::Ordering;

use crate::domain::provider::{
    EffectiveAvailability, EffectiveStatus, PaceStatus, ProjectionConfidence, ProviderQuota,
    ProviderStatus, RunwayStatus, SemanticsStatus,
};
use crate::domain::schema::QuotaReport;
use crate::domain::tiers::{provider_needs_sign_in, provider_tiers};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConstraintKind {
    Exhausted,
    Projected,
}

/// The one top-level decision, or an honest health/data-quality summary.
#[derive(Clone, Debug, PartialEq)]
pub enum Attention {
    Constraint {
        severity: ConstraintSeverity,
        provider: String,
        tier: Option<String>,
        compact_tier: Option<String>,
        constraint: ConstraintKind,
        percent_remaining: Option<f64>,
        projected_exhausted_at: Option<String>,
        projection_confidence: Option<ProjectionConfidence>,
        resets_at: Option<String>,
    },
    Healthy {
        tracked: usize,
    },
    DataHealth {
        reason: DataHealthReason,
        affected: usize,
        tracked: usize,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConstraintSeverity {
    Critical,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataHealthReason {
    Partial,
    Unreadable,
    PaceUnknown,
}

struct RankedConstraint {
    attention: Attention,
    rank: u8,
    time: Option<String>,
    remaining: f64,
    provider_order: usize,
    effective_order: usize,
}

fn provider_name(provider: &ProviderQuota) -> String {
    match provider.provider.to_ascii_lowercase().as_str() {
        "claude" => "Claude".into(),
        // The decision line needs the compact product name, while the detail
        // header may retain the full catalog label.
        "codex" => "Codex".into(),
        "cursor" => "Cursor".into(),
        "kimi" => "Kimi".into(),
        "grok" => "Grok".into(),
        "copilot" => "GitHub Copilot".into(),
        _ => provider.provider.clone(),
    }
}

fn marketed(provider: &ProviderQuota) -> bool {
    matches!(
        provider.provider.to_ascii_lowercase().as_str(),
        "claude" | "codex" | "cursor" | "kimi" | "grok" | "copilot"
    )
}

fn provider_is_current(provider: &ProviderQuota) -> bool {
    provider.state.status == ProviderStatus::Fresh
        && !provider.state.stale
        && !provider_needs_sign_in(provider)
}

fn decision_grade(item: &EffectiveAvailability) -> bool {
    item.effective_percent_remaining.is_some()
        || item
            .runway
            .as_ref()
            .is_some_and(|runway| runway.status != RunwayStatus::Unknown)
        || item
            .pace
            .as_ref()
            .is_some_and(|pace| pace.status != PaceStatus::Unknown)
}

fn on_pace(item: &EffectiveAvailability) -> bool {
    item.runway
        .as_ref()
        .is_some_and(|runway| runway.status == RunwayStatus::ThroughReset)
        || item
            .pace
            .as_ref()
            .is_some_and(|pace| matches!(pace.status, PaceStatus::OnPace | PaceStatus::Behind))
}

fn constraint_for(
    provider: &ProviderQuota,
    effective: &EffectiveAvailability,
    provider_order: usize,
    effective_order: usize,
) -> Option<RankedConstraint> {
    let runway = effective.runway.as_ref()?;
    let (constraint, rank) = match runway.status {
        RunwayStatus::ExhaustedNow => (ConstraintKind::Exhausted, 0),
        RunwayStatus::ProjectedExhaustion
            if runway.projection_confidence == Some(ProjectionConfidence::Established) =>
        {
            (ConstraintKind::Projected, 1)
        }
        _ => return None,
    };
    let limiting_id = runway
        .limiting_window_id
        .as_deref()
        .or(effective
            .pace
            .as_ref()
            .and_then(|pace| pace.worst_reserve_window_id.as_deref()))
        .or(effective.limiting_window_ids.first().map(String::as_str));
    let row = limiting_id.and_then(|id| {
        provider_tiers(provider)
            .into_iter()
            .find(|row| row.id == id)
    });
    let window = limiting_id.and_then(|id| provider.windows.iter().find(|window| window.id == id));
    let attention = Attention::Constraint {
        severity: ConstraintSeverity::Critical,
        provider: provider_name(provider),
        tier: row.as_ref().map(|row| row.label.clone()),
        compact_tier: row.as_ref().map(|row| row.compact_label.clone()),
        constraint,
        percent_remaining: effective.effective_percent_remaining,
        projected_exhausted_at: runway.projected_exhausted_at.clone(),
        projection_confidence: runway.projection_confidence,
        resets_at: window.and_then(|window| window.resets_at.clone()),
    };
    Some(RankedConstraint {
        attention,
        rank,
        time: match constraint {
            ConstraintKind::Projected => runway.projected_exhausted_at.clone(),
            ConstraintKind::Exhausted => window.and_then(|window| window.resets_at.clone()),
        },
        remaining: effective.effective_percent_remaining.unwrap_or(101.0),
        provider_order,
        effective_order,
    })
}

fn decimal(bytes: &[u8]) -> Option<i64> {
    (!bytes.is_empty() && bytes.iter().all(u8::is_ascii_digit)).then(|| {
        bytes
            .iter()
            .fold(0_i64, |value, byte| value * 10 + i64::from(byte - b'0'))
    })
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    // Howard Hinnant's civil-date conversion, with 1970-01-01 as day zero.
    let year = year - i64::from(month <= 2);
    let era = (if year >= 0 { year } else { year - 399 }) / 400;
    let year_of_era = year - era * 400;
    let month_index = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn timestamp_sort_key(value: &str) -> Option<(i64, u32)> {
    let bytes = value.as_bytes();
    if bytes.len() < 20 || !bytes.is_ascii() || !matches!(bytes[10], b'T' | b't') {
        return None;
    }
    let year = decimal(&bytes[0..4])?;
    let month = decimal(&bytes[5..7])?;
    let day = decimal(&bytes[8..10])?;
    let hour = decimal(&bytes[11..13])?;
    let minute = decimal(&bytes[14..16])?;
    let second = decimal(&bytes[17..19])?;
    if !matches!(
        (bytes[4], bytes[7], bytes[13], bytes[16]),
        (b'-', b'-', b':', b':')
    ) || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let mut index = 19;
    let mut nanos = 0_u32;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let fraction_start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        let fraction = &bytes[fraction_start..index];
        if fraction.is_empty() {
            return None;
        }
        for digit in fraction.iter().take(9) {
            nanos = nanos * 10 + u32::from(digit - b'0');
        }
        for _ in fraction.len()..9 {
            nanos *= 10;
        }
    }
    let offset_seconds = match bytes.get(index) {
        Some(b'Z' | b'z') if index + 1 == bytes.len() => 0,
        Some(sign @ (b'+' | b'-')) if index + 6 == bytes.len() && bytes[index + 3] == b':' => {
            let hours = decimal(&bytes[index + 1..index + 3])?;
            let minutes = decimal(&bytes[index + 4..index + 6])?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            let value = hours * 3600 + minutes * 60;
            if *sign == b'+' { value } else { -value }
        }
        _ => return None,
    };
    Some((
        days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second
            - offset_seconds,
        nanos,
    ))
}

fn compare_time(left: &Option<String>, right: &Option<String>) -> Ordering {
    // A missing or malformed time deliberately sorts after a known recovery.
    match (
        left.as_deref().and_then(timestamp_sort_key),
        right.as_deref().and_then(timestamp_sort_key),
    ) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn compare_constraint(left: &RankedConstraint, right: &RankedConstraint) -> Ordering {
    left.rank
        .cmp(&right.rank)
        .then_with(|| {
            if left.rank == 1 {
                compare_time(&left.time, &right.time)
                    .then_with(|| left.remaining.total_cmp(&right.remaining))
            } else {
                left.remaining
                    .total_cmp(&right.remaining)
                    // A longer (or unknown) recovery is the stronger current block.
                    .then_with(|| compare_time(&right.time, &left.time))
            }
        })
        .then_with(|| left.provider_order.cmp(&right.provider_order))
        .then_with(|| left.effective_order.cmp(&right.effective_order))
}

/// Select attention from a trusted subset of providers. The view model uses
/// this seam to remove deliberately non-visible providers before ranking.
pub fn select_attention_from<'a>(
    providers: impl IntoIterator<Item = &'a ProviderQuota>,
) -> Attention {
    let mut constraints = Vec::new();
    let mut tracked_effective = Vec::new();
    let mut tracked = 0;
    let mut unreadable = 0;
    let mut partial = 0;

    for (provider_order, provider) in providers
        .into_iter()
        .filter(|provider| marketed(provider))
        .enumerate()
    {
        if provider.semantics_status == Some(SemanticsStatus::Partial) {
            partial += 1;
        }
        let known = provider
            .effective
            .iter()
            .filter(|item| item.status == EffectiveStatus::Known && decision_grade(item))
            .collect::<Vec<_>>();
        if !provider_is_current(provider)
            || (known.is_empty() && provider.semantics_status != Some(SemanticsStatus::Partial))
        {
            unreadable += 1;
            continue;
        }
        if known.is_empty() {
            continue;
        }
        tracked += 1;
        tracked_effective.extend(known.iter().copied());
        for (effective_order, effective) in known.into_iter().enumerate() {
            if let Some(candidate) =
                constraint_for(provider, effective, provider_order, effective_order)
            {
                constraints.push(candidate);
            }
        }
    }

    if let Some(limiting) = constraints.into_iter().min_by(compare_constraint) {
        return limiting.attention;
    }
    if unreadable > 0 {
        return Attention::DataHealth {
            reason: DataHealthReason::Unreadable,
            affected: unreadable,
            tracked,
        };
    }
    if partial > 0 {
        return Attention::DataHealth {
            reason: DataHealthReason::Partial,
            affected: partial,
            tracked,
        };
    }
    if tracked > 0 && tracked_effective.into_iter().all(on_pace) {
        return Attention::Healthy { tracked };
    }
    Attention::DataHealth {
        reason: DataHealthReason::PaceUnknown,
        affected: 0,
        tracked,
    }
}

/// Select attention from all marketed providers in a schema-v5 report.
pub fn select_attention(report: &QuotaReport) -> Attention {
    select_attention_from(&report.providers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::provider::{EffectivePace, ProviderState, QuotaWindow, Runway};

    fn quota(id: &str, effective: Vec<EffectiveAvailability>) -> ProviderQuota {
        ProviderQuota {
            provider: id.into(),
            label: None,
            source: None,
            plan: None,
            windows: vec![QuotaWindow {
                id: "weekly".into(),
                label: "weekly".into(),
                kind: "limit".into(),
                percent_used: None,
                percent_remaining: Some(0.0),
                starts_at: None,
                resets_at: Some("2026-09-10T00:00:00.000Z".into()),
                reset_text: None,
                window_seconds: None,
                spent_usd: None,
                limit_usd: None,
                pace: None,
            }],
            effective,
            semantics_status: Some(SemanticsStatus::Known),
            credits: None,
            state: ProviderState {
                status: ProviderStatus::Fresh,
                stale: false,
                refreshed_at: None,
                auth_status: Some("usable".into()),
                reason: None,
                remedy_command: None,
                error_code: None,
            },
        }
    }

    fn effective(runway: Runway) -> EffectiveAvailability {
        EffectiveAvailability {
            scope: "all_models".into(),
            status: EffectiveStatus::Known,
            effective_percent_remaining: Some(0.0),
            bounded_by: vec![],
            limiting_window_ids: vec!["weekly".into()],
            pace: None,
            runway: Some(runway),
        }
    }

    #[test]
    fn established_forecast_beats_health_and_earliest_forecast_wins() {
        let later = quota(
            "claude",
            vec![effective(Runway {
                status: RunwayStatus::ProjectedExhaustion,
                usable_runway_seconds: None,
                projected_exhausted_at: Some("2026-09-04T00:00:00.000Z".into()),
                limiting_window_id: Some("weekly".into()),
                projection_confidence: Some(ProjectionConfidence::Established),
                unmeasurable_window_ids: vec![],
            })],
        );
        let earlier = quota(
            "cursor",
            vec![effective(Runway {
                status: RunwayStatus::ProjectedExhaustion,
                usable_runway_seconds: None,
                projected_exhausted_at: Some("2026-09-03T00:00:00.000Z".into()),
                limiting_window_id: Some("weekly".into()),
                projection_confidence: Some(ProjectionConfidence::Established),
                unmeasurable_window_ids: vec![],
            })],
        );
        let selected = select_attention_from([&later, &earlier]);
        assert!(
            matches!(selected, Attention::Constraint { provider, constraint: ConstraintKind::Projected, .. } if provider == "Cursor")
        );
    }

    #[test]
    fn early_forecast_is_not_a_pinned_constraint() {
        let quota = quota(
            "claude",
            vec![effective(Runway {
                status: RunwayStatus::ProjectedExhaustion,
                usable_runway_seconds: None,
                projected_exhausted_at: None,
                limiting_window_id: Some("weekly".into()),
                projection_confidence: Some(ProjectionConfidence::Early),
                unmeasurable_window_ids: vec![],
            })],
        );
        assert!(matches!(
            select_attention_from([&quota]),
            Attention::DataHealth {
                reason: DataHealthReason::PaceUnknown,
                ..
            }
        ));
    }

    #[test]
    fn stale_or_partial_data_never_claims_healthy() {
        let mut stale = quota(
            "claude",
            vec![effective(Runway {
                status: RunwayStatus::ExhaustedNow,
                usable_runway_seconds: None,
                projected_exhausted_at: None,
                limiting_window_id: Some("weekly".into()),
                projection_confidence: None,
                unmeasurable_window_ids: vec![],
            })],
        );
        stale.state.stale = true;
        assert!(matches!(
            select_attention_from([&stale]),
            Attention::DataHealth {
                reason: DataHealthReason::Unreadable,
                ..
            }
        ));

        let mut partial = quota("cursor", vec![]);
        partial.semantics_status = Some(SemanticsStatus::Partial);
        assert!(matches!(
            select_attention_from([&partial]),
            Attention::DataHealth {
                reason: DataHealthReason::Partial,
                affected: 1,
                tracked: 0
            }
        ));
    }

    #[test]
    fn unknown_limiting_ids_do_not_become_attention_text() {
        let quota = quota(
            "cursor",
            vec![EffectiveAvailability {
                scope: "all_models".into(),
                status: EffectiveStatus::Known,
                effective_percent_remaining: Some(0.0),
                bounded_by: vec![],
                limiting_window_ids: vec!["future-window".into()],
                pace: Some(EffectivePace {
                    status: PaceStatus::Unknown,
                    worst_reserve_percent_points: None,
                    worst_reserve_window_id: None,
                    unknown_window_ids: vec![],
                }),
                runway: Some(Runway {
                    status: RunwayStatus::ExhaustedNow,
                    usable_runway_seconds: None,
                    projected_exhausted_at: None,
                    limiting_window_id: Some("future-window".into()),
                    projection_confidence: None,
                    unmeasurable_window_ids: vec![],
                }),
            }],
        );
        assert!(matches!(
            select_attention_from([&quota]),
            Attention::Constraint {
                tier: None,
                compact_tier: None,
                ..
            }
        ));
    }

    #[test]
    fn ranks_times_by_instant_not_timezone_spelling() {
        let first = Some("2026-09-03T04:00:00+02:00".to_owned());
        let second = Some("2026-09-03T01:30:00Z".to_owned());
        assert_eq!(compare_time(&first, &second), std::cmp::Ordering::Greater);
    }
}
