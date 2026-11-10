use crate::error::{Error, Result};
use crate::lsm_bytes::{read_u16, read_u32, read_u8};
use crate::types::Endian;

/// Caps a strip table: a crafted count could grow a Vec far past the file; real files have one strip per channel.
const MAX_STRIP_COUNT: usize = 100_000;

pub(super) fn tiff_type_size(ty: u16) -> u64 {
    match ty {
        1 | 2 | 6 | 7 => 1, // BYTE, ASCII, SBYTE, UNDEFINED
        3 | 8 => 2,         // SHORT, SSHORT
        4 | 9 | 11 => 4,    // LONG, SLONG, FLOAT
        5 | 10 | 12 => 8,   // RATIONAL, SRATIONAL, DOUBLE
        _ => 0,
    }
}

/// The first element of an IFD entry as a `u64`, read through the offset when it is not inline.
pub(super) fn read_scalar(buf: &[u8], ty: u16, count: u64, val_off: usize, e: Endian) -> Result<u64> {
    let tsz = tiff_type_size(ty);
    if tsz == 0 || count == 0 {
        return Err(Error::Decode(format!(
            "LSM entry has unsupported type {ty} / count {count}"
        )));
    }
    let total = tsz.checked_mul(count).ok_or_else(overflow)?;
    let data_off = if total <= 4 {
        val_off
    } else {
        read_u32(buf, val_off, e)? as usize
    };
    read_int_at(buf, ty, data_off, e)
}

pub(super) fn read_int_array(buf: &[u8], ty: u16, count: u64, val_off: usize, e: Endian) -> Result<Vec<u64>> {
    let tsz = tiff_type_size(ty);
    if tsz == 0 {
        return Err(Error::Decode(format!("LSM strip tag has unsupported type {ty}")));
    }
    let total = tsz.checked_mul(count).ok_or_else(overflow)?;
    let data_off = if total <= 4 {
        val_off
    } else {
        read_u32(buf, val_off, e)? as usize
    };
    let n = usize::try_from(count).map_err(|_| overflow())?;
    // The cap is checked before allocating: an oversized Vec aborts the process, uncatchably.
    if n > MAX_STRIP_COUNT {
        return Err(Error::Decode(format!(
            "LSM strip/tag array has {n} elements, exceeds sane cap {MAX_STRIP_COUNT} (corrupt?)"
        )));
    }
    let mut out = Vec::with_capacity(n.min(1 << 20));
    for i in 0..n {
        let off = i
            .checked_mul(tsz as usize)
            .and_then(|delta| data_off.checked_add(delta))
            .ok_or_else(overflow)?;
        out.push(read_int_at(buf, ty, off, e)?);
    }
    Ok(out)
}

fn read_int_at(buf: &[u8], ty: u16, off: usize, e: Endian) -> Result<u64> {
    Ok(match ty {
        3 | 8 => read_u16(buf, off, e)? as u64,
        4 | 9 | 11 => read_u32(buf, off, e)? as u64,
        1 | 6 | 7 => read_u8(buf, off)? as u64,
        other => {
            return Err(Error::Decode(format!(
                "LSM: cannot read type {other} as an integer"
            )))
        }
    })
}

pub(super) fn overflow() -> Error {
    Error::Decode("LSM: arithmetic overflow while parsing IFD".to_string())
}
