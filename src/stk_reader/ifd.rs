// SPDX-License-Identifier: Apache-2.0
//! The first IFD of a classic TIFF, reduced to where one private tag keeps its values.
//! The `tiff` crate materialises a whole tag value, and UIC2's COUNT is the plane count rather than the numbers its
//! declared type describes, so the raw entry is read here instead.

use crate::error::{Error, Result};
use crate::types::Endian;

/// UIC1 (33628): the `(id, value)` pairs, among them the calibration entries.
pub(super) const UIC1: u16 = 33628;

/// UIC2 (33629): the plane table; its COUNT is the number of planes.
pub(super) const UIC2: u16 = 33629;

const LITTLE_ENDIAN: [u8; 4] = [b'I', b'I', 0x2A, 0x00];
const BIG_ENDIAN: [u8; 4] = [b'M', b'M', 0x00, 0x2A];
const IFD_OFFSET: usize = 4;
const ENTRY_BYTES: usize = 12;

const SHORT_FILE: &str = "MetaMorph STK is a classic TIFF: the file is too short for a TIFF header";
const NOT_CLASSIC: &str =
    "MetaMorph STK is a classic TIFF: the byte-order mark is neither II* nor MM*";
const OVERFLOW: &str = "MetaMorph STK: an offset in the first IFD overflows usize";

/// One tag's values: `count` of them, and the byte `at` where they sit when they do not fit in the entry's own four
/// bytes.
pub(super) struct Entry {
    pub count: usize,
    pub at: usize,
}

pub(super) fn byte_order(buf: &[u8]) -> Result<Endian> {
    let magic: [u8; 4] = buf
        .get(..4)
        .and_then(|head| head.try_into().ok())
        .ok_or_else(|| Error::Unsupported(SHORT_FILE.to_string()))?;
    match magic {
        LITTLE_ENDIAN => Ok(Endian::Little),
        BIG_ENDIAN => Ok(Endian::Big),
        _ => Err(Error::Unsupported(NOT_CLASSIC.to_string())),
    }
}

/// The entry of `tag` in the first IFD, or `None` when the file carries no such tag.
pub(super) fn entry(buf: &[u8], endian: Endian, tag: u16) -> Result<Option<Entry>> {
    let base = u32_at(buf, IFD_OFFSET, endian)? as usize;
    let entries = u16_at(buf, base, endian)? as usize;
    for index in 0..entries {
        let at = base
            .checked_add(2 + index * ENTRY_BYTES)
            .ok_or_else(|| Error::Parse(OVERFLOW.to_string()))?;
        let raw = bytes_at(buf, at, ENTRY_BYTES)?;
        if u16_at(raw, 0, endian)? == tag {
            return Ok(Some(Entry {
                count: u32_at(raw, 4, endian)? as usize,
                at: u32_at(raw, 8, endian)? as usize,
            }));
        }
    }
    Ok(None)
}

/// The u32 at byte `at` in the file's own byte order.
pub(super) fn u32_at(buf: &[u8], at: usize, endian: Endian) -> Result<u32> {
    let raw = bytes_at(buf, at, 4)?;
    let bytes = [raw[0], raw[1], raw[2], raw[3]];
    Ok(match endian {
        Endian::Little => u32::from_le_bytes(bytes),
        Endian::Big => u32::from_be_bytes(bytes),
    })
}

fn u16_at(buf: &[u8], at: usize, endian: Endian) -> Result<u16> {
    let raw = bytes_at(buf, at, 2)?;
    Ok(match endian {
        Endian::Little => u16::from_le_bytes([raw[0], raw[1]]),
        Endian::Big => u16::from_be_bytes([raw[0], raw[1]]),
    })
}

fn bytes_at(buf: &[u8], at: usize, len: usize) -> Result<&[u8]> {
    let end = at
        .checked_add(len)
        .ok_or_else(|| Error::Parse(OVERFLOW.to_string()))?;
    buf.get(at..end).ok_or_else(|| {
        Error::Parse(format!(
            "MetaMorph STK: the file is {} bytes, too short for the {len} bytes at byte {at}",
            buf.len()
        ))
    })
}
