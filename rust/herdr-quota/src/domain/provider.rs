//! Provider data that the product can use.

use serde::Serialize;

/// A provider that Herdr Quota markets.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MarketedProvider {
    Claude,
    Codex,
    Cursor,
    Kimi,
    Grok,
    Copilot,
}

impl MarketedProvider {
    /// All providers in product order.
    pub const ALL: [Self; 6] = [
        Self::Claude,
        Self::Codex,
        Self::Cursor,
        Self::Kimi,
        Self::Grok,
        Self::Copilot,
    ];

    /// Return a provider for a schema ID.
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "cursor" => Some(Self::Cursor),
            "kimi" => Some(Self::Kimi),
            "grok" => Some(Self::Grok),
            "copilot" => Some(Self::Copilot),
            _ => None,
        }
    }

    /// Return the schema ID.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Cursor => "cursor",
            Self::Kimi => "kimi",
            Self::Grok => "grok",
            Self::Copilot => "copilot",
        }
    }

    /// Return the product label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "OpenAI Codex",
            Self::Cursor => "Cursor",
            Self::Kimi => "Kimi",
            Self::Grok => "Grok",
            Self::Copilot => "GitHub Copilot",
        }
    }

    /// Return the safe sign-in instruction.
    pub const fn recovery_instruction(self) -> &'static str {
        match self {
            Self::Claude => "claude, then /login",
            Self::Codex => "codex login",
            Self::Cursor => "cursor-agent login",
            Self::Kimi => "kimi login",
            Self::Grok => "grok",
            Self::Copilot => "github-copilot-cli auth login",
        }
    }
}

/// A provider collection status.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStatus {
    Fresh,
    Stale,
    Unavailable,
    AuthRequired,
    RateLimited,
    Error,
}

/// A quota semantics status.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticsStatus {
    Known,
    Partial,
    Unknown,
}

/// A pace status from quota-axi.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PaceStatus {
    Ahead,
    OnPace,
    Behind,
    Mixed,
    Unknown,
}

/// A runway status from quota-axi.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunwayStatus {
    ExhaustedNow,
    ProjectedExhaustion,
    ThroughReset,
    Unknown,
}

/// The confidence of a quota projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionConfidence {
    Early,
    Established,
}

/// The status of effective quota data.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveStatus {
    Known,
    Unknown,
}

/// The unit for provider credits.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CreditUnit {
    Usd,
    Credits,
}

/// Pace data for one quota window.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowPace {
    pub status: PaceStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reserve_percent_points: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub burn_multiple: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projected_exhausted_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projection_confidence: Option<ProjectionConfidence>,
}

/// One quota window from an allowed provider.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    pub id: String,
    pub label: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent_used: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent_remaining: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starts_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spent_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pace: Option<WindowPace>,
}

/// Pace data for effective quota.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectivePace {
    pub status: PaceStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worst_reserve_percent_points: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worst_reserve_window_id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unknown_window_ids: Vec<String>,
}

/// Runway data for effective quota.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Runway {
    pub status: RunwayStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usable_runway_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projected_exhausted_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limiting_window_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projection_confidence: Option<ProjectionConfidence>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unmeasurable_window_ids: Vec<String>,
}

/// Effective quota for one scope.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveAvailability {
    pub scope: String,
    pub status: EffectiveStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_percent_remaining: Option<f64>,
    pub bounded_by: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub limiting_window_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pace: Option<EffectivePace>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runway: Option<Runway>,
}

/// Optional provider credits.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Credits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unlimited: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<CreditUnit>,
}

/// Safe provider state.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderState {
    pub status: ProviderStatus,
    pub stale: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refreshed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remedy_command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

/// Safe quota data for one provider.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderQuota {
    pub provider: String,
    /// A display-safe account label produced at the schema boundary. Raw
    /// account identifiers and credential metadata never enter product data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_label: Option<String>,
    pub account_reported: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    pub windows: Vec<QuotaWindow>,
    pub effective: Vec<EffectiveAvailability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantics_status: Option<SemanticsStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credits: Option<Credits>,
    pub state: ProviderState,
}

#[cfg(test)]
mod tests {
    use super::MarketedProvider;

    #[test]
    fn lists_all_six_marketed_providers() {
        let ids = MarketedProvider::ALL.map(MarketedProvider::id);
        let labels = MarketedProvider::ALL.map(MarketedProvider::label);

        assert_eq!(
            ids,
            ["claude", "codex", "cursor", "kimi", "grok", "copilot"]
        );
        assert_eq!(
            labels,
            [
                "Claude",
                "OpenAI Codex",
                "Cursor",
                "Kimi",
                "Grok",
                "GitHub Copilot"
            ]
        );
        assert!(
            MarketedProvider::ALL
                .iter()
                .all(|provider| !provider.recovery_instruction().is_empty())
        );
    }
}
