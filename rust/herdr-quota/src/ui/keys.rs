//! Bounded terminal input parsing with an explicit fragmented-Escape window.

/// Incomplete Escape/CSI input is held this long before a bare Escape is emitted.
pub const ESCAPE_FRAGMENT_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(100);

/// One finite dashboard action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DashboardAction {
    Quit,
    Escape,
    Refresh,
    Preferences,
    Acknowledge,
    ScrollDown,
    ScrollUp,
    PageDown,
    PageUp,
    Previous,
    Next,
    Toggle,
    Activate,
    MoveUp,
    MoveDown,
    Save,
    Cancel,
    Reset,
    Confirm,
    Decline,
}

/// Stateful byte parser for a raw terminal stream.
#[derive(Debug, Default)]
pub struct TerminalInputParser {
    pending: Vec<u8>,
}

impl TerminalInputParser {
    /// Parse a new input fragment, retaining only an incomplete Escape sequence.
    pub fn push(&mut self, input: &[u8]) -> Vec<DashboardAction> {
        let mut value = std::mem::take(&mut self.pending);
        value.extend_from_slice(input);
        let (actions, pending) = parse_input(&value, true);
        self.pending = pending;
        actions
    }

    /// Resolve retained input after [`ESCAPE_FRAGMENT_TIMEOUT`].
    pub fn flush(&mut self) -> Vec<DashboardAction> {
        let value = std::mem::take(&mut self.pending);
        parse_input(&value, false).0
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

fn sequence_action(sequence: &[u8]) -> Option<DashboardAction> {
    match sequence {
        b"\x1b[A" => Some(DashboardAction::ScrollUp),
        b"\x1b[B" => Some(DashboardAction::ScrollDown),
        b"\x1b[D" => Some(DashboardAction::Previous),
        b"\x1b[C" => Some(DashboardAction::Next),
        b"\x1b[5~" => Some(DashboardAction::PageUp),
        b"\x1b[6~" => Some(DashboardAction::PageDown),
        _ => None,
    }
}

fn csi_length(value: &[u8]) -> Option<usize> {
    if !value.starts_with(b"\x1b[") {
        return None;
    }
    value
        .iter()
        .enumerate()
        .skip(2)
        .find_map(|(index, byte)| (0x40..=0x7e).contains(byte).then_some(index + 1))
}

fn key_action(byte: u8) -> Option<DashboardAction> {
    match byte {
        b'q' | b'Q' | 0x03 => Some(DashboardAction::Quit),
        b'r' | b'R' => Some(DashboardAction::Refresh),
        b'p' | b'P' => Some(DashboardAction::Preferences),
        b'a' | b'A' => Some(DashboardAction::Acknowledge),
        b'j' | b'J' => Some(DashboardAction::ScrollDown),
        b'k' | b'K' => Some(DashboardAction::ScrollUp),
        b'u' | b'U' => Some(DashboardAction::MoveUp),
        b'd' | b'D' => Some(DashboardAction::MoveDown),
        b's' | b'S' => Some(DashboardAction::Save),
        b'c' | b'C' => Some(DashboardAction::Cancel),
        b'x' | b'X' => Some(DashboardAction::Reset),
        b'y' | b'Y' => Some(DashboardAction::Confirm),
        b'n' | b'N' => Some(DashboardAction::Decline),
        b' ' => Some(DashboardAction::Toggle),
        b'\r' | b'\n' => Some(DashboardAction::Activate),
        _ => None,
    }
}

fn parse_input(value: &[u8], hold_incomplete: bool) -> (Vec<DashboardAction>, Vec<u8>) {
    let mut actions = Vec::new();
    let mut index = 0;
    while index < value.len() {
        if value[index] == 0x1b {
            let rest = &value[index..];
            if rest.starts_with(b"\x1b[") {
                if let Some(length) = csi_length(rest) {
                    if let Some(action) = sequence_action(&rest[..length]) {
                        actions.push(action);
                    }
                    index += length;
                    continue;
                }
                if hold_incomplete {
                    return (actions, rest.to_vec());
                }
            } else if hold_incomplete && rest.len() == 1 {
                return (actions, rest.to_vec());
            }
            actions.push(DashboardAction::Escape);
        } else if let Some(action) = key_action(value[index]) {
            actions.push(action);
        }
        index += 1;
    }
    (actions, Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_dashboard_modal_and_navigation_keys() {
        assert_eq!(
            TerminalInputParser::default().push(b"qrpa jkudscxyn\r"),
            vec![
                DashboardAction::Quit,
                DashboardAction::Refresh,
                DashboardAction::Preferences,
                DashboardAction::Acknowledge,
                DashboardAction::Toggle,
                DashboardAction::ScrollDown,
                DashboardAction::ScrollUp,
                DashboardAction::MoveUp,
                DashboardAction::MoveDown,
                DashboardAction::Save,
                DashboardAction::Cancel,
                DashboardAction::Reset,
                DashboardAction::Confirm,
                DashboardAction::Decline,
                DashboardAction::Activate,
            ]
        );
    }

    #[test]
    fn fragmented_escape_sequences_are_not_bare_escape() {
        let mut parser = TerminalInputParser::default();
        assert!(parser.push(b"\x1b").is_empty());
        assert!(parser.has_pending());
        assert!(parser.push(b"[").is_empty());
        assert_eq!(parser.push(b"A"), vec![DashboardAction::ScrollUp]);
        assert!(parser.push(b"\x1b[").is_empty());
        assert!(parser.push(b"6").is_empty());
        assert_eq!(parser.push(b"~"), vec![DashboardAction::PageDown]);
        assert!(parser.push(b"\x1b").is_empty());
        assert_eq!(parser.flush(), vec![DashboardAction::Escape]);
    }

    #[test]
    fn coalesced_input_keeps_repeated_essential_actions() {
        let mut parser = TerminalInputParser::default();
        assert_eq!(
            parser.push(b"rrqjz\x1b[2~\x1b[6~"),
            vec![
                DashboardAction::Refresh,
                DashboardAction::Refresh,
                DashboardAction::Quit,
                DashboardAction::ScrollDown,
                DashboardAction::PageDown,
            ]
        );
    }
}
