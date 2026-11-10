use crate::error::{Error, Result};

use super::experiment::{Axis, Loop};

pub(super) fn scene_count(loops: &[Loop]) -> usize {
    loops
        .iter()
        .find(|l| l.axis == Axis::P)
        .map(|l| l.count)
        .unwrap_or(1)
}

pub(super) fn axis_count(loops: &[Loop], axis: Axis) -> usize {
    loops.iter().find(|l| l.axis == axis).map(|l| l.count).unwrap_or(1)
}

/// C is not a loop axis: components live within each frame.
pub(super) fn seq_index(loops: &[Loop], scene: usize, t: usize, z: usize) -> Result<usize> {
    let mut seq = 0usize;
    for l in loops {
        let idx = match l.axis {
            Axis::P => scene,
            Axis::T => t,
            Axis::Z => z,
        };
        seq = seq
            .checked_mul(l.count)
            .and_then(|s| s.checked_add(idx))
            .ok_or_else(|| Error::Parse("ND2 sequence index overflow".to_string()))?;
    }
    Ok(seq)
}
