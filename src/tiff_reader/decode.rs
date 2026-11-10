use tiff::decoder::DecodingResult;

use crate::error::{Error, Result};
use crate::types::PlaneBuf;

/// Wraps a decoded page in its [`PlaneBuf`]; decoded dtypes the reader does not model are refused.
pub(super) fn plane_result(decoded: DecodingResult) -> Result<PlaneBuf> {
    Ok(match decoded {
        DecodingResult::U8(v) => PlaneBuf::U8(v),
        DecodingResult::U16(v) => PlaneBuf::U16(v),
        DecodingResult::I16(v) => PlaneBuf::I16(v),
        DecodingResult::F32(v) => PlaneBuf::F32(v),
        other => {
            return Err(Error::Unsupported(format!(
                "decoded TIFF sample type {} not modelled by phasit-imread",
                decoding_result_name(&other)
            )))
        }
    })
}

fn decoding_result_name(d: &DecodingResult) -> &'static str {
    match d {
        DecodingResult::U8(_) => "u8",
        DecodingResult::U16(_) => "u16",
        DecodingResult::U32(_) => "u32",
        DecodingResult::U64(_) => "u64",
        DecodingResult::F16(_) => "f16",
        DecodingResult::F32(_) => "f32",
        DecodingResult::F64(_) => "f64",
        DecodingResult::I8(_) => "i8",
        DecodingResult::I16(_) => "i16",
        DecodingResult::I32(_) => "i32",
        DecodingResult::I64(_) => "i64",
    }
}
