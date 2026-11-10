use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::types::DType;

use super::chunks::chunk_data;
use super::clx_parse::parse_clx_variant;

/// The only `eCompression` the reader decodes; both real samples state it.
pub(super) const COMPRESSION_NONE: i64 = 2;

const MAX_COMPONENTS: usize = 1_024;

pub(super) struct Nd2Attributes {
    pub(super) width: usize,
    pub(super) width_bytes: usize,
    pub(super) height: usize,
    pub(super) comp: usize,
    pub(super) dtype: DType,
}

/// `SLxImageAttributes`, read as `nd2`'s `load_attributes` reads them.
pub(super) fn parse_attributes(buf: &[u8], map: &HashMap<Vec<u8>, (u64, u64)>) -> Result<Nd2Attributes> {
    let (off, _) = map
        .get(b"ImageAttributesLV!".as_slice())
        .ok_or_else(|| Error::Parse("ND2 missing ImageAttributesLV! chunk".to_string()))?;
    let data = chunk_data(buf, *off, None)?;
    let root = parse_clx_variant(data)?;
    let a = root
        .get_level("SLxImageAttributes")
        .ok_or_else(|| Error::Parse("ND2 missing SLxImageAttributes".to_string()))?;

    let width = a
        .get_usize("uiWidth")
        .ok_or_else(|| Error::Parse("ND2 attributes missing uiWidth".to_string()))?;
    let width_bytes = a
        .get_usize("uiWidthBytes")
        .ok_or_else(|| Error::Parse("ND2 attributes missing uiWidthBytes".to_string()))?;
    let height = a
        .get_usize("uiHeight")
        .ok_or_else(|| Error::Parse("ND2 attributes missing uiHeight".to_string()))?;
    let comp = a
        .get_usize("uiComp")
        .ok_or_else(|| Error::Parse("ND2 attributes missing uiComp".to_string()))?;
    let bpc = a
        .get_u32("uiBpcInMemory")
        .ok_or_else(|| Error::Parse("ND2 attributes missing uiBpcInMemory".to_string()))?;

    // Only uncompressed frames are decoded: `2` is what Nikon means by no compression, as both real samples state.
    // Every other value is refused BY VALUE, so the reason names what the file said; an absent value is refused too.
    match a.get_i64("eCompression") {
        Some(e) if e != COMPRESSION_NONE => {
            return Err(Error::Unsupported(format!(
                "ND2 eCompression {e} is not uncompressed ({COMPRESSION_NONE}); compressed ND2 \
                 not yet supported"
            )));
        }
        None => {
            return Err(Error::Unsupported(
                "ND2 states no eCompression; only uncompressed frames are supported".to_string(),
            ));
        }
        Some(_) => {}
    }

    let dtype = match bpc {
        8 => DType::U8,
        16 => DType::U16,
        32 => DType::F32,
        other => {
            return Err(Error::Unsupported(format!(
                "ND2 uiBpcInMemory {other} not supported (expected 8/16/32)"
            )))
        }
    };

    if width == 0 || height == 0 || comp == 0 {
        return Err(Error::Parse("ND2 attributes have a zero dimension".to_string()));
    }
    if comp > MAX_COMPONENTS {
        return Err(Error::Unsupported(format!(
            "ND2 component count {comp} exceeds cap {MAX_COMPONENTS}"
        )));
    }
    let min_stride = width
        .checked_mul(comp)
        .and_then(|v| v.checked_mul(dtype.size_bytes()))
        .ok_or_else(|| Error::Parse("ND2 row size overflow".to_string()))?;
    if width_bytes < min_stride {
        return Err(Error::Parse(format!(
            "ND2 uiWidthBytes {width_bytes} is smaller than width*comp*itemsize {min_stride}"
        )));
    }

    Ok(Nd2Attributes { width, width_bytes, height, comp, dtype })
}
