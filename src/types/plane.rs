use crate::error::{Error, Result};

use super::buffer::PlaneBuf;
use super::pixel::DType;
use super::region::PixelRect;

/// A decoded 2-D plane: owned, native-endian samples plus their `[Y, X]` shape. Build it with [`Plane::new`].
/// Multi-sample planes interleave `samples` per pixel; RGB stays R,G,B (CZI's BGR is swapped on decode).
/// `dtype` is the per-sample type, so an RGB48 plane is `(U16, 3)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Plane {
    /// Row-major pixel samples; interleaved within each pixel for multi-sample planes.
    pub data: PlaneBuf,
    /// `[Y, X]` in pixels, independent of `samples`.
    pub shape: [usize; 2],
    /// Samples per pixel (`1` = grayscale, `3` = RGB). Interleaved in `data`.
    pub samples: u32,
}

impl Plane {
    /// A grayscale plane; errors on a shape/length mismatch or an overflowing shape.
    pub fn new(data: PlaneBuf, shape: [usize; 2]) -> Result<Plane> {
        Plane::new_samples(data, shape, 1)
    }

    /// A plane with `samples` interleaved samples per pixel; `samples == 0` is refused.
    pub fn new_samples(data: PlaneBuf, shape: [usize; 2], samples: u32) -> Result<Plane> {
        if samples == 0 {
            return Err(Error::Decode("plane samples per pixel must be >= 1".to_string()));
        }
        let expected = shape[0]
            .checked_mul(shape[1])
            .and_then(|px| px.checked_mul(samples as usize))
            .ok_or_else(|| {
                Error::Decode(format!(
                    "plane shape {}x{}x{} overflows usize",
                    shape[0], shape[1], samples
                ))
            })?;
        if data.len() != expected {
            return Err(Error::Decode(format!(
                "plane data length {} does not match shape {}x{} ({} samples/px, {} elements)",
                data.len(),
                shape[0],
                shape[1],
                samples,
                expected
            )));
        }
        Ok(Plane { data, shape, samples })
    }

    /// The pixel [`DType`] of this plane (the per-sample type for color planes).
    pub fn dtype(&self) -> DType {
        self.data.dtype()
    }

    pub fn samples(&self) -> u32 {
        self.samples
    }

    pub fn height(&self) -> usize {
        self.shape[0]
    }

    pub fn width(&self) -> usize {
        self.shape[1]
    }

    /// A new owned plane `[rect.h, rect.w]`, byte-exact with this plane's window (all interleaved samples).
    /// Errors with [`Error::Decode`] if `rect` exceeds this plane; an overflowing `x + w` or `y + h` is rejected too.
    pub fn crop(&self, rect: PixelRect) -> Result<Plane> {
        let full_w = self.width();
        let full_h = self.height();

        let x_end = rect.x.checked_add(rect.w).ok_or_else(|| {
            Error::Decode(format!(
                "crop rect x+w overflows usize (x={}, w={})",
                rect.x, rect.w
            ))
        })?;
        let y_end = rect.y.checked_add(rect.h).ok_or_else(|| {
            Error::Decode(format!(
                "crop rect y+h overflows usize (y={}, h={})",
                rect.y, rect.h
            ))
        })?;
        if x_end > full_w || y_end > full_h {
            return Err(Error::Decode(format!(
                "crop rect {{x:{}, y:{}, w:{}, h:{}}} out of range for plane {}x{} (w x h)",
                rect.x, rect.y, rect.w, rect.h, full_w, full_h
            )));
        }

        let samples = self.samples as usize;

        // One arm per PlaneBuf variant: `rect.h` contiguous, samples-aware row copies.
        macro_rules! crop_rows {
            ($v:expr) => {{
                let mut out = Vec::with_capacity(rect.h * rect.w * samples);
                for row in 0..rect.h {
                    let src_row = rect.y + row;
                    let row_start = (src_row * full_w + rect.x) * samples;
                    let row_end = row_start + rect.w * samples;
                    out.extend_from_slice(&$v[row_start..row_end]);
                }
                out
            }};
        }

        let data = match &self.data {
            PlaneBuf::U8(v) => PlaneBuf::U8(crop_rows!(v)),
            PlaneBuf::U16(v) => PlaneBuf::U16(crop_rows!(v)),
            PlaneBuf::I16(v) => PlaneBuf::I16(crop_rows!(v)),
            PlaneBuf::F32(v) => PlaneBuf::F32(crop_rows!(v)),
        };

        Plane::new_samples(data, [rect.h, rect.w], self.samples)
    }

    /// Typed `ndarray::Array2`, dispatched on dtype; only grayscale planes map to `[Y, X]`.
    /// A multi-sample plane is refused rather than reshaping interleaved samples.
    #[cfg(feature = "ndarray")]
    pub fn to_ndarray(&self) -> Result<PlaneArray> {
        if self.samples != 1 {
            return Err(Error::Decode(format!(
                "to_ndarray supports only grayscale planes (samples == 1), got {}",
                self.samples
            )));
        }
        let shape = (self.shape[0], self.shape[1]);
        let map_err = |e: ndarray::ShapeError| Error::Decode(format!("ndarray shape error: {e}"));
        Ok(match &self.data {
            PlaneBuf::U8(v) => {
                PlaneArray::U8(ndarray::Array2::from_shape_vec(shape, v.clone()).map_err(map_err)?)
            }
            PlaneBuf::U16(v) => {
                PlaneArray::U16(ndarray::Array2::from_shape_vec(shape, v.clone()).map_err(map_err)?)
            }
            PlaneBuf::I16(v) => {
                PlaneArray::I16(ndarray::Array2::from_shape_vec(shape, v.clone()).map_err(map_err)?)
            }
            PlaneBuf::F32(v) => {
                PlaneArray::F32(ndarray::Array2::from_shape_vec(shape, v.clone()).map_err(map_err)?)
            }
        })
    }
}

/// Present only with the `ndarray` feature, so the dependency stays optional.
#[cfg(feature = "ndarray")]
#[derive(Debug, Clone, PartialEq)]
pub enum PlaneArray {
    U8(ndarray::Array2<u8>),
    U16(ndarray::Array2<u16>),
    I16(ndarray::Array2<i16>),
    F32(ndarray::Array2<f32>),
}
