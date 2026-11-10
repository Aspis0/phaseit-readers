//! Why a level of the file's pyramid is not offered: the refusal and the tiles behind it.

use std::collections::BTreeMap;

use crate::types::RefusedLevel;

use super::coarse::Skipped;

/// Why a level is not offered: the tiles behind it and the reason.
pub(super) struct Refusal {
    pub skipped: usize,
    pub reason: String,
}

impl Refusal {
    /// A refusal with no tile behind it.
    pub(super) fn new(reason: impl Into<String>) -> Refusal {
        Refusal {
            skipped: 0,
            reason: reason.into(),
        }
    }
}

/// The refusal of a level that never materialized: its tiles' sizes name no level.
pub(super) fn orphan_refusal(size: [usize; 2], skips: &[&Skipped]) -> RefusedLevel {
    RefusedLevel {
        factor: 0,
        skipped: skips.len(),
        reason: format!(
            "{} tile(s) of the level of {}x{}-pixel tiles could not be read: {}",
            skips.len(),
            size[0],
            size[1],
            skips
                .iter()
                .map(|skip| skip.reason.reason())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// The skipped tiles whose stored size names no minification and whose size is no level's cell,
/// by that size: the levels they belong to never materialized.
pub(super) fn unplaced_skips<'a>(
    skipped: &'a [Skipped],
    cells: &BTreeMap<usize, [usize; 2]>,
) -> BTreeMap<[usize; 2], Vec<&'a Skipped>> {
    let mut out: BTreeMap<[usize; 2], Vec<&Skipped>> = BTreeMap::new();
    for skip in skipped {
        if skip.factor.is_none() && !cells.values().any(|cell| *cell == skip.size) {
            out.entry(skip.size).or_default().push(skip);
        }
    }
    out
}

/// The reasons behind the refusal of the level with this minification.
///
/// WHY the cell: a tile whose stored size names no minification belongs to the level whose cell
/// is its size — the level its grid position was made for.
pub(super) fn skips_of(
    factor: usize,
    skipped: &[Skipped],
    cells: &BTreeMap<usize, [usize; 2]>,
) -> Vec<String> {
    skipped
        .iter()
        .filter(|skip| match skip.factor {
            Some(placed) => placed == factor,
            None => cells.get(&factor).is_some_and(|cell| *cell == skip.size),
        })
        .map(|skip| skip.reason.reason())
        .collect()
}
