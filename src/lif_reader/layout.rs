use crate::error::{Error, Result};
use crate::types::{DType, Dims5};

pub(super) struct SceneLayout {
    pub(super) dims: Dims5,
    pub(super) dtype: DType,
    pub(super) block_offset: usize,
    pub(super) block_end: usize,
    pub(super) t_bytes_inc: usize,
    pub(super) z_bytes_inc: usize,
    /// Sorted ascending, as liffile orders its channels; `len == dims.c`.
    pub(super) channel_bytes_inc: Vec<usize>,
    /// Refused images are non-contiguous, so rows are packed at this stride.
    pub(super) row_stride: usize,
}

impl SceneLayout {
    pub(super) fn plane_offset(&self, t: usize, c: usize, z: usize) -> Result<usize> {
        t.checked_mul(self.t_bytes_inc)
            .and_then(|v| v.checked_add(z.checked_mul(self.z_bytes_inc)?))
            .and_then(|v| v.checked_add(self.channel_bytes_inc[c]))
            .ok_or_else(|| Error::Decode("LIF plane offset overflow".to_string()))
    }
}
