//! Text sanitation for terminal output and safe error messages.

use std::fmt::Display;
use std::sync::LazyLock;

use regex::Regex;

const DISPLAY_LIMIT: usize = 256;
const PROCESS_ERROR_LIMIT: usize = 180;

static BEARER_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)Bearer\s+[A-Za-z0-9._~+/\-]+").expect("the bearer token pattern must be valid")
});
static NAMED_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:sk|api|key|token)[-_][A-Za-z0-9._~+/\-]{8,}")
        .expect("the named token pattern must be valid")
});
static LONG_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[A-Za-z0-9._~+/\-]{24,}\b").expect("the secret pattern must be valid")
});
static UNIX_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:/Users/|/home/)[^\s:'\"]+"#).expect("the Unix path pattern must be valid")
});
static WINDOWS_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)[A-Z]:\\[^\s:'\"]+"#).expect("the Windows path pattern must be valid")
});
static ACCOUNT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[\w.+-]+@[\w.-]+\.[A-Za-z]{2,}").expect("the account pattern must be valid")
});
static LINE_BREAKS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\r\n\t]+").expect("the line break pattern must be valid"));

#[derive(Clone, Copy)]
enum TerminalState {
    Text,
    Escape,
    Csi,
    Osc,
    OscEscape,
    ControlString,
    ControlStringEscape,
}

fn strip_terminal_sequences(value: &str) -> String {
    let mut clean = String::with_capacity(value.len());
    let mut state = TerminalState::Text;

    for character in value.chars() {
        state = match state {
            TerminalState::Text => match character {
                '\u{001b}' => TerminalState::Escape,
                '\u{0080}'..='\u{009f}' => TerminalState::Text,
                character if character.is_control() && !matches!(character, '\t' | '\n' | '\r') => {
                    TerminalState::Text
                }
                _ => {
                    clean.push(character);
                    TerminalState::Text
                }
            },
            TerminalState::Escape => match character {
                '[' => TerminalState::Csi,
                ']' => TerminalState::Osc,
                'P' | 'X' | '^' | '_' => TerminalState::ControlString,
                '\u{0020}'..='\u{002f}' => TerminalState::Escape,
                _ => TerminalState::Text,
            },
            TerminalState::Csi => {
                if ('\u{0040}'..='\u{007e}').contains(&character) {
                    TerminalState::Text
                } else {
                    TerminalState::Csi
                }
            }
            TerminalState::Osc => match character {
                '\u{0007}' => TerminalState::Text,
                '\u{001b}' => TerminalState::OscEscape,
                _ => TerminalState::Osc,
            },
            TerminalState::OscEscape => {
                if character == '\\' {
                    TerminalState::Text
                } else {
                    TerminalState::Osc
                }
            }
            TerminalState::ControlString => {
                if character == '\u{001b}' {
                    TerminalState::ControlStringEscape
                } else {
                    TerminalState::ControlString
                }
            }
            TerminalState::ControlStringEscape => {
                if character == '\\' {
                    TerminalState::Text
                } else {
                    TerminalState::ControlString
                }
            }
        };
    }

    clean
}

/// Clean one display string and limit it to 256 characters.
pub fn sanitize_display_text(value: &str) -> Option<String> {
    let stripped = strip_terminal_sequences(value);
    let clean = stripped
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>();
    let clean = clean.trim();

    if clean.is_empty() {
        None
    } else {
        Some(clean.chars().take(DISPLAY_LIMIT).collect())
    }
}

/// Map a provider error code to safe text.
pub fn friendly_provider_error(code: Option<&str>) -> &'static str {
    let Some(code) = code else {
        return "Quota unavailable";
    };
    let mut normalized = String::with_capacity(code.len());
    let mut has_separator = false;
    for character in code.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            normalized.push(character);
            has_separator = false;
        } else if !has_separator {
            normalized.push('_');
            has_separator = true;
        }
    }

    match normalized.as_str() {
        "credentials_missing" => "Sign-in not found",
        "credentials_invalid" => "Sign-in could not be read",
        "credentials_expired" => "Sign-in expired",
        "keychain_prompt_required" => "Keychain approval required",
        "provider_auth_rejected" => "Provider rejected sign-in",
        "provider_rate_limited" => "Provider temporarily rate limited",
        "request_timeout" => "Provider request timed out",
        "network_unavailable" => "Network unavailable",
        "schema_invalid" => "Provider response changed",
        "quota_unavailable" => "Quota unavailable",
        _ => "Quota unavailable",
    }
}

/// Remove secrets and terminal controls from a process error.
pub fn sanitize_process_error(value: impl Display) -> String {
    let text = strip_terminal_sequences(&value.to_string());
    let text = BEARER_TOKEN.replace_all(&text, "Bearer [redacted]");
    let text = NAMED_TOKEN.replace_all(&text, "[redacted]");
    let text = LONG_SECRET.replace_all(&text, "[redacted]");
    let text = UNIX_PATH.replace_all(&text, "[local path]");
    let text = WINDOWS_PATH.replace_all(&text, "[local path]");
    let text = ACCOUNT.replace_all(&text, "[account]");
    let text = LINE_BREAKS.replace_all(&text, " ");
    let clean = text.trim();

    if clean.is_empty() {
        "Quota refresh failed".to_owned()
    } else {
        clean.chars().take(PROCESS_ERROR_LIMIT).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{friendly_provider_error, sanitize_display_text, sanitize_process_error};

    #[test]
    fn bounds_display_strings() {
        let value = format!("  {}  ", "x".repeat(300));
        let clean = sanitize_display_text(&value).expect("text must remain");

        assert_eq!(clean.chars().count(), 256);
        assert!(clean.chars().all(|character| character == 'x'));
    }

    #[test]
    fn strips_terminal_control_sequences() {
        let value = "before\u{001b}[2J\u{001b}[31mred\u{001b}[0m\u{001b}]0;secret title\u{0007}after\u{0000}\u{009b}31m";

        assert_eq!(sanitize_process_error(value), "beforeredafter31m");
        assert_eq!(
            sanitize_display_text(value).as_deref(),
            Some("beforeredafter31m")
        );
    }

    #[test]
    fn removes_secrets_from_process_errors() {
        let value = "Bearer secret.token.value\n/home/alice/.codex/auth.json alice@example.com api-key-abcdefghijk";
        let clean = sanitize_process_error(value);

        assert!(!clean.contains("secret"));
        assert!(!clean.contains("alice"));
        assert!(!clean.contains("example"));
        assert!(!clean.contains("abcdefghijk"));
        assert!(!clean.contains('\n'));
        assert!(clean.contains("redacted"));
    }

    #[test]
    fn maps_only_known_provider_codes() {
        assert_eq!(
            friendly_provider_error(Some("request_timeout")),
            "Provider request timed out"
        );
        assert_eq!(
            friendly_provider_error(Some("token=secret-account-detail")),
            "Quota unavailable"
        );
    }
}
