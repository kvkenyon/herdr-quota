//! Fixed-width quota gauges with an honest unknown state.

const FULL: char = '█';
const TRACK: char = '─';
const EIGHTHS: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];

/// The user-selected presentation for a trustworthy remaining percentage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MeterMode {
    Remaining,
    Used,
}

/// Converts a remaining reading to its visible meter value. No reading stays
/// unknown; it is never converted to a zero or 100 percent claim.
pub fn displayed_percent(percent_remaining: Option<f64>, mode: MeterMode) -> Option<f64> {
    let remaining = percent_remaining
        .filter(|value| value.is_finite())?
        .clamp(0.0, 100.0);
    Some(match mode {
        MeterMode::Remaining => remaining,
        MeterMode::Used => 100.0 - remaining,
    })
}

/// Draws a width-exact gauge at one eighth-cell precision. A full bar means
/// exactly 100%, an empty track means exactly 0%, and unknown is spaces.
pub fn remaining_bar(percent: Option<f64>, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let Some(percent) = percent.filter(|value| value.is_finite()) else {
        return " ".repeat(width);
    };
    let capacity = width * 8;
    let percent = percent.clamp(0.0, 100.0);
    let mut eighths = ((percent / 100.0) * capacity as f64).floor() as usize;
    if percent >= 100.0 {
        eighths = capacity;
    } else if percent <= 0.0 {
        eighths = 0;
    } else {
        eighths = eighths.clamp(1, capacity - 1);
    }
    let solid = eighths / 8;
    let partial = eighths % 8;
    let mut result = String::with_capacity(width * 3);
    result.extend(std::iter::repeat_n(FULL, solid));
    if partial > 0 {
        result.push(EIGHTHS[partial]);
    }
    result.extend(std::iter::repeat_n(
        TRACK,
        width - solid - usize::from(partial > 0),
    ));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled_eighths(bar: &str) -> usize {
        bar.chars()
            .map(|cell| match cell {
                '█' => 8,
                '▏' => 1,
                '▎' => 2,
                '▍' => 3,
                '▌' => 4,
                '▋' => 5,
                '▊' => 6,
                '▉' => 7,
                _ => 0,
            })
            .sum()
    }

    #[test]
    fn exact_endpoints_unknown_and_width_hold_at_every_size() {
        for width in 0..64 {
            assert_eq!(remaining_bar(Some(100.0), width).chars().count(), width);
            assert_eq!(remaining_bar(Some(0.0), width).chars().count(), width);
            assert_eq!(remaining_bar(None, width).chars().count(), width);
            assert_eq!(remaining_bar(Some(f64::NAN), width).chars().count(), width);
            if width > 0 {
                assert_eq!(remaining_bar(Some(100.0), width), "█".repeat(width));
                assert_eq!(remaining_bar(Some(0.0), width), "─".repeat(width));
                assert_eq!(remaining_bar(None, width), " ".repeat(width));
            }
        }
    }

    #[test]
    fn non_endpoint_values_keep_a_visible_notch_or_sliver() {
        assert_eq!(remaining_bar(Some(99.9), 4), "███▉");
        assert_eq!(remaining_bar(Some(0.1), 4), "▏───");
        assert_eq!(remaining_bar(Some(69.0), 4), "██▊─");
        assert_eq!(remaining_bar(Some(74.0), 4), "██▉─");
    }

    #[test]
    fn gauge_fill_is_monotonic_for_many_widths_and_inputs() {
        for width in 1..64 {
            let mut prior = 0;
            for step in 0..=10_000 {
                let percent = step as f64 / 100.0;
                let fill = filled_eighths(&remaining_bar(Some(percent), width));
                assert!(fill >= prior, "width {width}, percent {percent}");
                prior = fill;
            }
            assert_eq!(prior, width * 8);
        }
    }

    #[test]
    fn used_mode_is_the_remaining_complement_and_unknown_stays_unknown() {
        assert_eq!(displayed_percent(Some(31.0), MeterMode::Used), Some(69.0));
        assert_eq!(
            displayed_percent(Some(31.0), MeterMode::Remaining),
            Some(31.0)
        );
        assert_eq!(displayed_percent(None, MeterMode::Used), None);
        assert_eq!(
            displayed_percent(Some(f64::INFINITY), MeterMode::Remaining),
            None
        );
    }
}
