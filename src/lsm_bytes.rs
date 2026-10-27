// SPDX-License-Identifier: Apache-2.0
//! Bounds-checked reads of the scalars a Zeiss LSM file is made of.
//!
//! The LSM container is a TIFF whose private structs are read field by field:
//! the CZ-LSM-Info struct, the channel-colours block, the name table. All of
//! them need the same four scalars at a given offset, and every one of those
//! reads must fail cleanly when the file ends first. The checks live here so
//! that the format's readers share one implementation of them.

use byteorder::{BigEndian, ByteOrder, LittleEndian};

use crate::error::{Error, Result};
use crate::types::Endian;

/// Read a `u8` at `off`.
pub fn read_u8(buf: &[u8], off: usize) -> Result<u8> {
    Ok(slice(buf, off, 1)?[0])
}

/// Read a `u16` at `off`.
pub fn read_u16(buf: &[u8], off: usize, e: Endian) -> Result<u16> {    let s = slice(buf, off, 2)?;
    Ok(match e {
        Endian::Little => LittleEndian::read_u16(s),
        Endian::Big => BigEndian::read_u16(s),
    })
}

/// Read a `u32` at `off`.
pub fn read_u32(buf: &[u8], off: usize, e: Endian) -> Result<u32> {
    let s = slice(buf, off, 4)?;
    Ok(match e {
        Endian::Little => LittleEndian::read_u32(s),
        Endian::Big => BigEndian::read_u32(s),
    })
}

/// Read an `i32` at `off`.
pub fn read_i32(buf: &[u8], off: usize, e: Endian) -> Result<i32> {
    let s = slice(buf, off, 4)?;
    Ok(match e {
        Endian::Little => LittleEndian::read_i32(s),
        Endian::Big => BigEndian::read_i32(s),
    })
}

/// Read an `f64` at `off`.
pub fn read_f64(buf: &[u8], off: usize, e: Endian) -> Result<f64> {
    let s = slice(buf, off, 8)?;
    Ok(match e {
        Endian::Little => LittleEndian::read_f64(s),
        Endian::Big => BigEndian::read_f64(s),
    })
}

/// The `need` bytes at `off`, or the error that says why there are none.
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
