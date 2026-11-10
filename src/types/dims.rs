use crate::error::{Error, Result};

use super::pixel::DType;

/// Image dimensions in canonical **TCZYX** order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dims5 {
    pub t: usize,
    pub c: usize,
    pub z: usize,
    pub y: usize,
    pub x: usize,
}

impl Dims5 {
    /// Total planes `t * c * z`; saturates on overflow, so never size an allocation with it.
    pub fn num_planes(&self) -> usize {
        self.t.saturating_mul(self.c).saturating_mul(self.z)
    }

    pub fn plane_elems(&self) -> usize {
        self.y.saturating_mul(self.x)
    }

    /// Byte size of one plane, refused on overflow or above [`MAX_DECODED_PLANE_BYTES`]; call before reserving memory.
    pub fn checked_plane_bytes(&self, dtype: DType) -> Result<usize> {
        self.checked_plane_bytes_samples(dtype, 1)
    }

    /// `dtype` is the per-sample type: an RGB24 plane is `(U8, 3)` and an RGB48 plane is `(U16, 3)`.
    pub fn checked_plane_bytes_samples(&self, dtype: DType, samples: u32) -> Result<usize> {
        match self.plane_bytes(dtype, samples) {
            // Overflow is an oversize refusal, not a panic.
            None => Err(Error::OversizedAllocation {
                requested: usize::MAX,
                cap: crate::guard::MAX_DECODED_PLANE_BYTES,
            }),
            Some(b) if b > crate::guard::MAX_DECODED_PLANE_BYTES => {
                Err(Error::OversizedAllocation {
                    requested: b,
                    cap: crate::guard::MAX_DECODED_PLANE_BYTES,
                })
            }
            Some(b) => Ok(b),
        }
    }

    /// Refused only on overflow: the ceiling applies where a plane is materialised; over-ceiling scenes serve windows.
    pub fn plane_bytes_no_ceiling(&self, dtype: DType) -> Result<usize> {
        match self.plane_bytes(dtype, 1) {
            None => Err(Error::OversizedAllocation {
                requested: usize::MAX,
                cap: crate::guard::MAX_DECODED_PLANE_BYTES,
            }),
            Some(b) => Ok(b),
        }
    }

    fn plane_bytes(&self, dtype: DType, samples: u32) -> Option<usize> {
        self.x
            .checked_mul(self.y)
            .and_then(|elems| elems.checked_mul(samples as usize))
            .and_then(|elems| elems.checked_mul(dtype.size_bytes()))
    }

    /// Flat index `((t * C + c) * Z + z)`: Z fastest, then C, then T; saturates on overflow, like `num_planes`.
    pub fn plane_index(&self, t: usize, c: usize, z: usize) -> usize {
        t.saturating_mul(self.c)
            .saturating_add(c)
            .saturating_mul(self.z)
            .saturating_add(z)
    }

    pub fn iter_planes(&self) -> impl Iterator<Item = (usize, usize, usize)> {
        let (c, z) = (self.c, self.z);
        (0..self.t).flat_map(move |ti| {
            (0..c).flat_map(move |ci| (0..z).map(move |zi| (ti, ci, zi)))
        })
    }
}
