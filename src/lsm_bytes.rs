// SPDX-License-Identifier: Apache-2.0
//! Bounds-checked reads of the scalars a Zeiss LSM file is made of: every read fails cleanly when the file ends first.
//! Shared by the LSM structs and the channel-colours block, so the checks have one implementation.

use byteorder::{BigEndian, ByteOrder, LittleEndian};

use crate::error::{Error, Result};
use crate::types::Endian;

pub fn read_u8(buf: &[u8], off: usize) -> Result<u8> {
    Ok(slice(buf, off, 1)?[0])
}

pub fn read_u16(buf: &[u8], off: usize, e: Endian) -> Result<u16> {    let s = slice(buf, off, 2)?;
    Ok(match e {
        Endian::Little => LittleEndian::read_u16(s),
        Endian::Big => BigEndian::read_u16(s),
    })
}

pub fn read_u32(buf: &[u8], off: usize, e: Endian) -> Result<u32> {
    let s = slice(buf, off, 4)?;
    Ok(match e {
        Endian::Little => LittleEndian::read_u32(s),
        Endian::Big => BigEndian::read_u32(s),
    })
}

pub fn read_i32(buf: &[u8], off: usize, e: Endian) -> Result<i32> {
    let s = slice(buf, off, 4)?;
    Ok(match e {
        Endian::Little => LittleEndian::read_i32(s),
        Endian::Big => BigEndian::read_i32(s),
    })
}

pub fn read_f64(buf: &[u8], off: usize, e: Endian) -> Result<f64> {
    let s = slice(buf, off, 8)?;
    Ok(match e {
        Endian::Little => LittleEndian::read_f64(s),
        Endian::Big => BigEndian::read_f64(s),
    })
}

fn slice(buf: &[u8], off: usize, need: usize) -> Result<&[u8]> {
    let end = off
        .checked_add(need)
        .ok_or_else(|| Error::Parse(format!("LSM read out of bounds: need {need} bytes at offset {off}")))?;
    buf.get(off..end).ok_or_else(|| {
        Error::Parse(format!(
            "LSM read out of bounds: need {need} bytes at offset {off}, file is {} bytes",
            buf.len()
        ))
    })
}
