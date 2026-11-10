use std::collections::HashMap;

use crate::types::{DType, Dims5};

#[derive(Clone)]
pub(super) struct CziScene {
    pub(super) dims: Dims5,
    pub(super) dtype: DType,
    pub(super) samples: u32,
    /// Sorted distinct values: canonical index `i` maps to position `i`; bases need not be zero or contiguous.
    pub(super) c_values: Vec<i32>,
    pub(super) z_values: Vec<i32>,
    pub(super) t_values: Vec<i32>,
    pub(super) origin_x: i32,
    pub(super) origin_y: i32,
    pub(super) tiles: HashMap<(i32, i32, i32), Vec<usize>>,
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct SceneSkips {
    pub(super) unmodelled_pixel_type: usize,
    pub(super) bad_span: usize,
}

impl SceneSkips {
    pub(super) fn total(&self) -> usize {
        self.unmodelled_pixel_type + self.bad_span
    }
}

/// The reason a scene is refused because it lost tiles: how many of its full-resolution tiles were skipped, and why.
/// Names the scene by its file rank and its `S` value, so the reason points at the file's scene, not its published
/// rank.
pub(super) fn lost_tiles_reason(rank: usize, s_val: i32, total: usize, skips: &SceneSkips) -> String {
    let mut causes = Vec::new();
    if skips.unmodelled_pixel_type > 0 {
        causes.push(format!(
            "{} tile(s) of a pixel type the reader does not model",
            skips.unmodelled_pixel_type
        ));
    }
    if skips.bad_span > 0 {
        causes.push(format!(
            "{} tile(s) whose pixel span is out of bounds",
            skips.bad_span
        ));
    }
    let cause = causes.join("; ");
    let skipped = skips.total();
    if skipped >= total {
        format!(
            "scene {rank} of the file (S={s_val}): all {total} of its full-resolution tiles \
             were skipped ({cause})"
        )
    } else {
        format!(
            "scene {rank} of the file (S={s_val}): {skipped} of its {total} full-resolution \
             tiles were skipped ({cause})"
        )
    }
}
