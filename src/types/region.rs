use crate::error::{Error, Result};

use super::buffer::PlaneBuf;
use super::dims::Dims5;
use super::pixel::DType;
use super::plane::Plane;

/// A rectangle of a scene's plane, in pixels; `(x, y)` is the top-left corner in the full `dims.x` x `dims.y` plane.
/// `x`/`w` address the width axis and `y`/`h` the height axis, as in [`Plane::shape`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl PixelRect {
    pub fn new(x: usize, y: usize, w: usize, h: usize) -> PixelRect {
        PixelRect { x, y, w, h }
    }

    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }
}

/// Validates a region's coordinate, then its rectangle, in the order every reader uses.
/// One helper, because a call wrong in both ways must report the same error everywhere.
pub(crate) fn checked_region(
    label: &str,
    dims: &Dims5,
    scene: usize,
    t: usize,
    c: usize,
    z: usize,
    rect: PixelRect,
) -> Result<()> {
    if t >= dims.t || c >= dims.c || z >= dims.z {
        return Err(Error::OutOfRange { scene, t, c, z });
    }

    let x_end = rect.x.checked_add(rect.w).ok_or_else(|| {
        Error::Decode(format!(
            "{label} rect x+w overflows usize (x={}, w={})",
            rect.x, rect.w
        ))
    })?;
    let y_end = rect.y.checked_add(rect.h).ok_or_else(|| {
        Error::Decode(format!(
            "{label} rect y+h overflows usize (y={}, h={})",
            rect.y, rect.h
        ))
    })?;
    if x_end > dims.x || y_end > dims.y {
        return Err(Error::Decode(format!(
            "{label} rect {{x:{}, y:{}, w:{}, h:{}}} out of range for scene {} dims (x:{}, y:{})",
            rect.x, rect.y, rect.w, rect.h, scene, dims.x, dims.y
        )));
    }

    Ok(())
}

/// The plane of an EMPTY rectangle: the same plane the default `region()` returns, shape `[rect.h, rect.w]`.
/// Settled once here, because a reader counting the rows of an empty rect can underflow on `rect.h - 1`.
pub(crate) fn empty_region_plane(dtype: DType, samples: u32, rect: PixelRect) -> Result<Plane> {
    let data = match dtype {
        DType::U8 => PlaneBuf::U8(Vec::new()),
        DType::U16 => PlaneBuf::U16(Vec::new()),
        DType::I16 => PlaneBuf::I16(Vec::new()),
        DType::F32 => PlaneBuf::F32(Vec::new()),
    };
    Plane::new_samples(data, [rect.h, rect.w], samples)
}
