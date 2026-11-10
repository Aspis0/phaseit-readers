use crate::error::{Error, Result};

use super::segment::SEGMENT_HEADER_LEN;

pub(super) fn segment_id_is(buf: &[u8], off: usize, id: &[u8]) -> bool {
    match buf.get(off..off + 16) {
        Some(sid) => sid.iter().zip(id).all(|(a, b)| a == b) && sid[id.len()..].iter().all(|&b| b == 0),
        None => false,
    }
}

pub(super) fn position_in_file(pos: i64, len: usize, what: &str) -> Result<usize> {
    if pos < 0 {
        return Err(Error::Parse(format!("CZI {what} {pos} is negative")));
    }
    let pos = usize::try_from(pos)
        .map_err(|_| Error::Parse(format!("CZI {what} overflows usize")))?;
    if pos.saturating_add(SEGMENT_HEADER_LEN) > len {
        return Err(Error::Parse(format!(
            "CZI {what} {pos} points past EOF ({len} bytes)"
        )));
    }
    Ok(pos)
}

pub(super) fn nonneg_usize(v: i32, what: &str) -> Result<usize> {
    if v < 0 {
        return Err(Error::Parse(format!("CZI {what} {v} is negative")));
    }
    Ok(v as usize)
}

pub(super) fn rd_i32(buf: &[u8], off: usize) -> Result<i32> {
    let s = buf.get(off..off + 4).ok_or_else(|| bounds(off, 4, buf.len()))?;
    Ok(i32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

pub(super) fn rd_i64(buf: &[u8], off: usize) -> Result<i64> {
    let s = buf.get(off..off + 8).ok_or_else(|| bounds(off, 8, buf.len()))?;
    Ok(i64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
}

pub(super) fn bounds(off: usize, need: usize, len: usize) -> Error {
    Error::Parse(format!(
        "CZI read out of bounds: need {need} bytes at offset {off}, file is {len} bytes"
    ))
}
