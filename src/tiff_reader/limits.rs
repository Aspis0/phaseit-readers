use crate::error::{Error, Result};
use crate::types::DType;

/// Caps one IFD value; the `tiff` crate gates reads on `decoding_buffer_size`, so both limits are raised (tiff 0.11.3).
const MAX_IFD_VALUE_BYTES: usize = 64 * 1024 * 1024;

/// Caps the bytes the `tiff` crate materialises for one page: an inflated SamplesPerPixel is refused before the decoder
/// is built.
pub(super) fn raw_page_bytes(y: usize, x: usize, samples: usize, dtype: DType) -> Result<usize> {
    let bytes = y
        .checked_mul(x)
        .and_then(|v| v.checked_mul(samples))
        .and_then(|v| v.checked_mul(dtype.size_bytes()));
    match bytes {
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

pub(super) fn tiff_limits(raw_bytes: usize) -> tiff::decoder::Limits {
    let buf = raw_bytes.clamp(MAX_IFD_VALUE_BYTES, crate::guard::MAX_DECODED_PLANE_BYTES);
    let mut limits = tiff::decoder::Limits::default();
    limits.decoding_buffer_size = buf;
    limits.intermediate_buffer_size = buf;
    limits.ifd_value_size = MAX_IFD_VALUE_BYTES.min(crate::guard::MAX_DECODED_PLANE_BYTES);
    limits
}
