//! Pure planning for lossless sidebar insertion and split-tree restoration.

use std::collections::HashSet;

use super::SidebarError;

/// One pane rectangle relative to the usable tab area.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PaneRect {
    pub(crate) pane_id: String,
    pub(crate) x: u64,
    pub(crate) y: u64,
    pub(crate) width: u64,
    pub(crate) height: u64,
}

/// The split direction accepted by Herdr.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SplitDirection {
    Right,
    Down,
}

/// A horizontal resize direction accepted by Herdr.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResizeDirection {
    Left,
    Right,
}

impl ResizeDirection {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

impl SplitDirection {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Right => "right",
            Self::Down => "down",
        }
    }
}

/// One operation in a split-tree rebuild.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MoveStep {
    pub(crate) pane: String,
    pub(crate) direction: SplitDirection,
    pub(crate) target: String,
    pub(crate) ratio: f64,
}

/// A stable anchor and the pre-order operations that rebuild its split tree.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RebuildPlan {
    pub(crate) anchor: String,
    pub(crate) steps: Vec<MoveStep>,
}

#[derive(Clone, Copy)]
enum Axis {
    Horizontal,
    Vertical,
}

impl Axis {
    fn direction(self) -> SplitDirection {
        match self {
            Self::Horizontal => SplitDirection::Right,
            Self::Vertical => SplitDirection::Down,
        }
    }

    fn start(self, rect: &PaneRect) -> u64 {
        match self {
            Self::Horizontal => rect.x,
            Self::Vertical => rect.y,
        }
    }

    fn end(self, rect: &PaneRect) -> Option<u64> {
        match self {
            Self::Horizontal => rect.x.checked_add(rect.width),
            Self::Vertical => rect.y.checked_add(rect.height),
        }
    }
}

const AXES: [Axis; 2] = [Axis::Horizontal, Axis::Vertical];

/// Derive a rebuild plan for any rectangular binary split tree.
pub(crate) fn plan_rebuild(rects: &[PaneRect]) -> Result<RebuildPlan, SidebarError> {
    if rects.is_empty()
        || rects.iter().any(|rect| rect.width == 0 || rect.height == 0)
        || rects
            .iter()
            .map(|rect| rect.pane_id.as_str())
            .collect::<HashSet<_>>()
            .len()
            != rects.len()
    {
        return Err(SidebarError::UnsafeLayout);
    }
    partition(rects)
}

fn partition(rects: &[PaneRect]) -> Result<RebuildPlan, SidebarError> {
    if let [rect] = rects {
        return Ok(RebuildPlan {
            anchor: rect.pane_id.clone(),
            steps: Vec::new(),
        });
    }

    for axis in AXES {
        let Some(cut) = clean_cuts(rects, axis)?.into_iter().next() else {
            continue;
        };
        let (start, end) = region_bounds(rects, axis)?;
        let (first, second): (Vec<_>, Vec<_>) = rects
            .iter()
            .cloned()
            .partition(|rect| axis.end(rect).is_some_and(|edge| edge <= cut));
        let left = partition(&first)?;
        let right = partition(&second)?;
        let mut steps = Vec::with_capacity(rects.len().saturating_sub(1));
        steps.push(MoveStep {
            pane: right.anchor.clone(),
            direction: axis.direction(),
            target: left.anchor.clone(),
            ratio: (cut - start) as f64 / (end - start) as f64,
        });
        steps.extend(left.steps);
        steps.extend(right.steps);
        return Ok(RebuildPlan {
            anchor: left.anchor,
            steps,
        });
    }

    Err(SidebarError::UnsafeLayout)
}

fn region_bounds(rects: &[PaneRect], axis: Axis) -> Result<(u64, u64), SidebarError> {
    let start = rects
        .iter()
        .map(|rect| axis.start(rect))
        .min()
        .ok_or(SidebarError::UnsafeLayout)?;
    let end = rects
        .iter()
        .map(|rect| axis.end(rect).ok_or(SidebarError::UnsafeLayout))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .max()
        .ok_or(SidebarError::UnsafeLayout)?;
    if start >= end {
        return Err(SidebarError::UnsafeLayout);
    }
    Ok((start, end))
}

fn clean_cuts(rects: &[PaneRect], axis: Axis) -> Result<Vec<u64>, SidebarError> {
    let (start, end) = region_bounds(rects, axis)?;
    let mut cuts = rects
        .iter()
        .map(|rect| axis.end(rect).ok_or(SidebarError::UnsafeLayout))
        .collect::<Result<Vec<_>, _>>()?;
    cuts.sort_unstable();
    cuts.dedup();
    cuts.retain(|cut| *cut > start && *cut < end);
    cuts.retain(|cut| {
        let before = rects
            .iter()
            .filter(|rect| axis.end(rect).is_some_and(|edge| edge <= *cut))
            .collect::<Vec<_>>();
        let after = rects
            .iter()
            .filter(|rect| axis.end(rect).is_some_and(|edge| edge > *cut))
            .collect::<Vec<_>>();
        !before.is_empty()
            && !after.is_empty()
            && before
                .iter()
                .filter_map(|rect| axis.end(rect))
                .max()
                .zip(after.iter().map(|rect| axis.start(rect)).min())
                .is_some_and(|(before_end, after_start)| before_end <= after_start)
    });
    Ok(cuts)
}

/// Target 36 cells on ordinary tabs and retain 24 cells for work when possible.
pub(crate) fn target_sidebar_width(total_width: u64) -> u64 {
    let total = total_width.max(1);
    if total >= 60 {
        36
    } else if total >= 44 {
        total - 24
    } else {
        16.max(total / 2)
    }
}

/// Convert a cell-width correction into Herdr's directional ratio operation.
pub(crate) fn resize_for_target(
    total_width: u64,
    current_width: u64,
) -> Option<(ResizeDirection, f64)> {
    let target = target_sidebar_width(total_width);
    let difference = current_width.abs_diff(target);
    if difference == 0 || total_width == 0 {
        return None;
    }
    Some((
        if current_width > target {
            ResizeDirection::Right
        } else {
            ResizeDirection::Left
        },
        difference as f64 / total_width as f64,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &str, x: u64, y: u64, width: u64, height: u64) -> PaneRect {
        PaneRect {
            pane_id: id.to_owned(),
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn rebuilds_deep_mixed_axis_split_trees_independent_of_input_order() {
        let rects = [
            pane("p5", 70, 75, 30, 25),
            pane("p2", 0, 40, 40, 60),
            pane("p4", 70, 0, 30, 75),
            pane("p1", 0, 0, 40, 40),
            pane("p3", 40, 0, 30, 100),
        ];
        let plan = plan_rebuild(&rects).expect("valid split tree");

        assert_eq!(plan.anchor, "p1");
        assert_eq!(
            plan.steps
                .iter()
                .map(|step| (step.pane.as_str(), step.direction, step.target.as_str()))
                .collect::<Vec<_>>(),
            [
                ("p3", SplitDirection::Right, "p1"),
                ("p2", SplitDirection::Down, "p1"),
                ("p4", SplitDirection::Right, "p3"),
                ("p5", SplitDirection::Down, "p4"),
            ]
        );
        assert!((plan.steps[0].ratio - 0.4).abs() < 0.001);
        assert!((plan.steps[1].ratio - 0.4).abs() < 0.001);
        assert!((plan.steps[2].ratio - 0.5).abs() < 0.001);
        assert!((plan.steps[3].ratio - 0.75).abs() < 0.001);
    }

    #[test]
    fn rejects_non_split_layouts_and_duplicate_panes() {
        assert_eq!(
            plan_rebuild(&[pane("p1", 0, 0, 50, 60), pane("p2", 40, 40, 60, 60),]),
            Err(SidebarError::UnsafeLayout)
        );
        assert_eq!(
            plan_rebuild(&[pane("p1", 0, 0, 50, 100), pane("p1", 50, 0, 50, 100)]),
            Err(SidebarError::UnsafeLayout)
        );
    }

    #[test]
    fn width_targets_36_cells_and_degrades_without_starving_the_tab() {
        assert_eq!(target_sidebar_width(160), 36);
        assert_eq!(target_sidebar_width(80), 36);
        assert_eq!(target_sidebar_width(54), 30);
        assert_eq!(target_sidebar_width(48), 24);
        assert_eq!(target_sidebar_width(36), 18);
        assert_eq!(
            resize_for_target(120, 60),
            Some((ResizeDirection::Right, 0.2))
        );
        let (_, amount) = resize_for_target(54, 27).expect("resize required");
        assert!((amount - 3.0 / 54.0).abs() < f64::EPSILON);
    }
}
