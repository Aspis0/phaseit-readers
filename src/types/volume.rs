// SPDX-License-Identifier: Apache-2.0
use crate::error::{Error, Result};

use super::box3::Box3;
use super::buffer::PlaneBuf;
use super::pixel::DType;
use super::plane::Plane;

/// Largest 3-D box a caller may hold at once (8 GiB): [`crate::MAX_DECODED_PLANE_BYTES`] bounds one plane, so this is
/// two planes' worth — past any real box, far below a corrupt header's claim.
pub const MAX_VOLUME_BYTES: usize = 8 * 1024 * 1024 * 1024;

/// A 3-D box of one channel at one time point: native-endian samples, z-major then y then x.
#[derive(Debug, Clone, PartialEq)]
pub struct Volume {
    /// Samples plane by plane, each plane in its own row-major `[Y, X]` layout, as [`Plane::shape`] states.
    pub data: PlaneBuf,
    /// `[x, y, z]` in pixels/planes: the buffer's z axis is the outermost, so a voxel's element index is
    /// `(z_idx * dims[1] + y_idx) * dims[0] + x_idx`.
    pub dims: [usize; 3],
    /// Samples per pixel, interleaved as in the planes this volume was stacked from.
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

    /// Stacks z-major: the planes in the box's z order, each plane as the box's `[h, w]` window.
    /// A plane of another shape, type or sample count would make the flat buffer unreadable, so the box is refused.
    pub(crate) fn stack(bx: Box3, planes: Vec<Plane>) -> Result<Volume> {
        if planes.len() != bx.d {
            return Err(Error::Decode(format!(
                "volume box {}x{}x{} holds {} plane(s), not the stated depth {}",
                bx.w,
                bx.h,
                bx.d,
                planes.len(),
                bx.d
            )));
        }
        let samples = planes[0].samples;
        let dtype = planes[0].dtype();
        let shape = [bx.h, bx.w];
        for plane in &planes {
            if plane.shape != shape || plane.samples != samples || plane.dtype() != dtype {
                return Err(Error::Decode(format!(
                    "volume box {}x{}x{} mixes planes: expected {shape:?} of {dtype:?} with {samples} sample(s)/px",
                    bx.w, bx.h, bx.d
                )));
            }
        }
        let elems = Self::checked_bytes(bx, dtype, samples)? / dtype.size_bytes();

        macro_rules! stack_planes {
            ($variant:ident) => {{
                let mut out = Vec::with_capacity(elems);
                for plane in &planes {
                    match &plane.data {
                        PlaneBuf::$variant(values) => out.extend_from_slice(values),
                        // The pass above refused a mixed box, so no other variant reaches this arm.
                        _ => {
                            return Err(Error::Decode(format!(
                                "volume box {}x{}x{} mixes pixel types",
                                bx.w, bx.h, bx.d
                            )))
                        }
                    }
                }
                PlaneBuf::$variant(out)
            }};
        }

        let data = match dtype {
            DType::U8 => stack_planes!(U8),
            DType::U16 => stack_planes!(U16),
            DType::I16 => stack_planes!(I16),
            DType::F32 => stack_planes!(F32),
        };
        Ok(Volume {
            data,
            dims: [bx.w, bx.h, bx.d],
            samples,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(values: &[u8]) -> Plane {
        Plane::new(PlaneBuf::U8(values.to_vec()), [1, values.len()]).expect("the plane")
    }

    #[test]
    fn a_box_is_stacked_plane_by_plane() {
        let bx = Box3::new(0, 0, 0, 2, 1, 3);
        let planes = vec![plane(&[1, 2]), plane(&[3, 4]), plane(&[5, 6])];
        let volume = Volume::stack(bx, planes).expect("stack the planes");
        assert_eq!(volume.dims, [2, 1, 3]);
        assert_eq!(volume.samples, 1);
        assert_eq!(volume.dtype(), DType::U8);
        assert_eq!(volume.data, PlaneBuf::U8(vec![1, 2, 3, 4, 5, 6]));
    }

    #[test]
    fn a_plane_of_another_shape_refuses_the_box() {
        let bx = Box3::new(0, 0, 0, 2, 1, 2);
        let planes = vec![plane(&[1, 2]), plane(&[1, 2, 3])];
        assert!(matches!(Volume::stack(bx, planes), Err(Error::Decode(_))));
    }

    #[test]
    fn fewer_planes_than_the_box_states_refuses_the_box() {
        let bx = Box3::new(0, 0, 0, 2, 1, 3);
        let planes = vec![plane(&[1, 2]), plane(&[3, 4])];
        assert!(matches!(Volume::stack(bx, planes), Err(Error::Decode(_))));
    }

    #[test]
    fn an_overflowing_box_is_refused_before_any_byte_is_counted() {
        let bx = Box3::new(0, 0, 0, usize::MAX, usize::MAX, usize::MAX);
        match Volume::checked_bytes(bx, DType::F32, 3) {
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
        match Volume::checked_bytes(bx, DType::U8, 1) {
            Err(Error::OversizedAllocation { requested, cap }) => {
                assert!(requested > cap, "{requested} bytes is past the cap");
            }
            other => panic!("expected OversizedAllocation past the cap, got {other:?}"),
        }
    }
}
