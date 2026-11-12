// SPDX-License-Identifier: Apache-2.0
use crate::error::{Error, Result};

use super::box3::Box3;
use super::buffer::PlaneBuf;
use super::pixel::DType;
use super::plane::Plane;

/// Largest 3-D box a caller may hold at once (8 GiB): the box is materialised in ONE buffer, reserved once, so this is
/// the peak `volume()` needs beside the single plane it is copying.
pub const MAX_VOLUME_BYTES: usize = 8 * 1024 * 1024 * 1024;

/// A 3-D box of one channel at one time point: native-endian samples, z-major then y then x.
#[derive(Debug, Clone, PartialEq)]
pub struct Volume {
    /// Samples plane by plane, each plane in its own row-major `[Y, X]` layout, as [`Plane::shape`] states.
    pub data: PlaneBuf,
    /// `[x, y, z]` in pixels/planes: the buffer's z axis is the outermost, so a voxel's element index is
    /// `(z_idx * dims[1] + y_idx) * dims[0] + x_idx`.
    pub dims: [usize; 3],
    /// Samples per pixel, interleaved as in the planes this volume was filled from.
    pub samples: u32,
}

impl Volume {
    /// The pixel [`DType`] of this volume's planes (the per-sample type for color volumes).
    pub fn dtype(&self) -> DType {
        self.data.dtype()
    }

    /// Byte size of `bx`, refused on overflow or above [`MAX_VOLUME_BYTES`]; call it BEFORE reserving memory.
    pub(crate) fn checked_bytes(bx: Box3, dtype: DType, samples: u32) -> Result<usize> {
        // Overflow is an oversize refusal, not a panic.
        let bytes =
            bx.w.checked_mul(bx.h)
                .and_then(|px| px.checked_mul(bx.d))
                .and_then(|px| px.checked_mul(samples as usize))
                .and_then(|px| px.checked_mul(dtype.size_bytes()));
        match bytes {
            Some(b) if b <= MAX_VOLUME_BYTES => Ok(b),
            Some(b) => Err(Error::OversizedAllocation {
                requested: b,
                cap: MAX_VOLUME_BYTES,
            }),
            None => Err(Error::OversizedAllocation {
                requested: usize::MAX,
                cap: MAX_VOLUME_BYTES,
            }),
        }
    }

    /// A volume of `bx`'s shape, its buffer reserved ONCE: the planes are appended into it, so the peak is one buffer
    /// beside one plane, not a plane per depth plus a second buffer of the volume's size.
    pub(crate) fn new(bx: Box3, dtype: DType, samples: u32) -> Result<Volume> {
        if bx.is_empty() {
            return Err(Error::Decode(format!(
                "volume box {}x{}x{} is empty",
                bx.w, bx.h, bx.d
            )));
        }
        let elems = Self::checked_bytes(bx, dtype, samples)? / dtype.size_bytes();
        let data = match dtype {
            DType::U8 => PlaneBuf::U8(Vec::with_capacity(elems)),
            DType::U16 => PlaneBuf::U16(Vec::with_capacity(elems)),
            DType::I16 => PlaneBuf::I16(Vec::with_capacity(elems)),
            DType::F32 => PlaneBuf::F32(Vec::with_capacity(elems)),
        };
        Ok(Volume {
            data,
            dims: [bx.w, bx.h, bx.d],
            samples,
        })
    }

    /// Appends one plane's samples, in the box's z order; the plane is dropped by the caller right after this returns,
    /// so a whole box costs one buffer plus one plane, never one buffer plus a plane per depth.
    pub(crate) fn push_plane(&mut self, plane: Plane) -> Result<()> {
        let shape = [self.dims[1], self.dims[0]];
        if plane.shape != shape || plane.samples != self.samples || plane.dtype() != self.dtype() {
            return Err(Error::Decode(format!(
                "volume box {}x{}x{} takes {shape:?} planes of {:?} with {} sample(s)/px",
                self.dims[0],
                self.dims[1],
                self.dims[2],
                self.dtype(),
                self.samples
            )));
        }
        let total = self.dims[0] * self.dims[1] * self.dims[2] * self.samples as usize;
        let dtype = self.dtype();

        macro_rules! push {
            ($variant:ident) => {{
                match (&mut self.data, plane.data) {
                    (PlaneBuf::$variant(out), PlaneBuf::$variant(values)) => {
                        // `new` reserved the box's element count; past it the box states more planes than its depth.
                        if out.len() + values.len() > total {
                            return Err(Error::Decode(format!(
                                "volume box {}x{}x{} holds more planes than its depth {}",
                                self.dims[0], self.dims[1], self.dims[2], self.dims[2]
                            )));
                        }
                        out.extend_from_slice(&values);
                    }
                    _ => {
                        return Err(Error::Decode(format!(
                            "volume box {}x{}x{} mixes pixel types",
                            self.dims[0], self.dims[1], self.dims[2]
                        )))
                    }
                }
            }};
        }

        match dtype {
            DType::U8 => push!(U8),
            DType::U16 => push!(U16),
            DType::I16 => push!(I16),
            DType::F32 => push!(F32),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(values: &[u8]) -> Plane {
        Plane::new(PlaneBuf::U8(values.to_vec()), [1, values.len()]).expect("the plane")
    }

    #[test]
    fn a_box_is_filled_plane_by_plane() {
        let bx = Box3::new(0, 0, 0, 2, 1, 3);
        let mut volume = Volume::new(bx, DType::U8, 1).expect("the volume");
        for values in [&[1u8, 2][..], &[3, 4], &[5, 6]] {
            volume.push_plane(plane(values)).expect("push the plane");
        }
        assert_eq!(volume.dims, [2, 1, 3]);
        assert_eq!(volume.samples, 1);
        assert_eq!(volume.dtype(), DType::U8);
        assert_eq!(volume.data, PlaneBuf::U8(vec![1, 2, 3, 4, 5, 6]));
    }

    #[test]
    fn a_plane_of_another_shape_refuses_the_push() {
        let bx = Box3::new(0, 0, 0, 2, 1, 2);
        let mut volume = Volume::new(bx, DType::U8, 1).expect("the volume");
        assert!(matches!(
            volume.push_plane(plane(&[1, 2, 3])),
            Err(Error::Decode(_))
        ));
    }

    #[test]
    fn more_planes_than_the_box_states_refuse_the_push() {
        let bx = Box3::new(0, 0, 0, 2, 1, 2);
        let mut volume = Volume::new(bx, DType::U8, 1).expect("the volume");
        volume.push_plane(plane(&[1, 2])).expect("the first plane");
        volume.push_plane(plane(&[3, 4])).expect("the second plane");
        assert!(matches!(
            volume.push_plane(plane(&[5, 6])),
            Err(Error::Decode(_))
        ));
    }

    #[test]
    fn an_empty_box_is_refused_before_any_byte_is_reserved() {
        for bx in [
            Box3::new(0, 0, 0, 0, 1, 1),
            Box3::new(0, 0, 0, 1, 0, 1),
            Box3::new(0, 0, 0, 1, 1, 0),
        ] {
            match Volume::new(bx, DType::U8, 1) {
                Err(Error::Decode(_)) => {}
                other => panic!("an empty box is refused, got {other:?}"),
            }
        }
    }

    #[test]
    fn an_overflowing_box_is_refused_before_any_byte_is_counted() {
        let bx = Box3::new(0, 0, 0, usize::MAX, usize::MAX, usize::MAX);
        match Volume::new(bx, DType::F32, 3) {
            Err(Error::OversizedAllocation { requested, cap }) => {
                assert_eq!(requested, usize::MAX);
                assert_eq!(cap, MAX_VOLUME_BYTES);
            }
            other => panic!("expected OversizedAllocation on overflow, got {other:?}"),
        }
    }

    #[test]
    fn a_box_past_the_cap_is_refused() {
        let bx = Box3::new(0, 0, 0, 100_000, 100_000, 100);
        match Volume::new(bx, DType::U8, 1) {
            Err(Error::OversizedAllocation { requested, cap }) => {
                assert!(requested > cap, "{requested} bytes is past the cap");
            }
            other => panic!("expected OversizedAllocation past the cap, got {other:?}"),
        }
    }
}
