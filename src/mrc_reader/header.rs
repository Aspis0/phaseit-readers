// SPDX-License-Identifier: Apache-2.0
//! The 1024-byte MRC2014 main header: 256 four-byte words in the file's own byte order.
//! `MAP ` and MACHST are read as raw bytes, because they are what tells the byte order.

use crate::error::{Error, Result};
use crate::types::{DType, Endian};

/// Size of the main header; the extended header (NSYMBT bytes) and the data follow it.
pub(super) const HEADER_BYTES: usize = 1024;

const MAP_OFFSET: usize = 208;
const MACHST_OFFSET: usize = 212;

/// The header words the reader needs; the remaining 256-word fields are not used.
pub(super) struct MrcHeader {
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    pub mx: i32,
    pub my: i32,
    pub mz: i32,
    /// Cell size in ångström along x, y, z.
    pub cella: [f32; 3],
    pub ispg: i32,
    /// Bytes of extended header between the main header and the data.
    pub nsymbt: usize,
    pub endian: Endian,
    pub dtype: DType,
}

pub(super) fn parse(buf: &[u8]) -> Result<MrcHeader> {
    if buf.len() < HEADER_BYTES {
        return Err(Error::Parse(format!(
            "MRC file is {} bytes, too short for the {HEADER_BYTES}-byte header",
            buf.len()
        )));
    }
    if buf.get(MAP_OFFSET..MAP_OFFSET + 4) != Some(&b"MAP "[..]) {
        return Err(Error::Unsupported(
            "not an MRC file: the `MAP ` signature is missing at offset 208".to_string(),
        ));
    }
    let endian = byte_order(buf)?;
    let mapc = word(buf, 16, endian)?;
    let mapr = word(buf, 17, endian)?;
    let maps = word(buf, 18, endian)?;
    axis_order(mapc, mapr, maps)?;

    Ok(MrcHeader {
        nx: extent("NX", word(buf, 0, endian)?)?,
        ny: extent("NY", word(buf, 1, endian)?)?,
        nz: extent("NZ", word(buf, 2, endian)?)?,
        mx: word(buf, 7, endian)?,
        my: word(buf, 8, endian)?,
        mz: word(buf, 9, endian)?,
        cella: [
            real(buf, 10, endian)?,
            real(buf, 11, endian)?,
            real(buf, 12, endian)?,
        ],
        ispg: word(buf, 22, endian)?,
        nsymbt: extended_bytes(word(buf, 23, endian)?)?,
        endian,
        dtype: dtype_of(word(buf, 3, endian)?)?,
    })
}

/// `0x44 0x44` and `0x44 0x41` are little-endian stamps, `0x11 0x11` is big-endian; anything else is a corrupt header.
fn byte_order(buf: &[u8]) -> Result<Endian> {
    let stamp = buf
        .get(MACHST_OFFSET..MACHST_OFFSET + 2)
        .ok_or_else(|| Error::Parse("MRC header has no MACHST stamp".to_string()))?;
    match stamp {
        [0x44, 0x44] | [0x44, 0x41] => Ok(Endian::Little),
        [0x11, 0x11] => Ok(Endian::Big),
        other => Err(Error::Unsupported(format!(
            "MRC MACHST byte-order stamp {other:02x?} is neither little- nor big-endian"
        ))),
    }
}

/// MODE is the sample type: the crate represents only I16, U16 and F32 exactly.
fn dtype_of(mode: i32) -> Result<DType> {
    match mode {
        1 => Ok(DType::I16),
        2 => Ok(DType::F32),
        6 => Ok(DType::U16),
        0 => Err(Error::Unsupported(
            "MRC MODE 0 stores signed 8-bit samples, which this crate cannot represent exactly; convert the file \
             to MODE 1 (16-bit signed) or MODE 6 (16-bit unsigned)"
                .to_string(),
        )),
        other => Err(Error::Unsupported(format!(
            "MRC MODE {other} stores a sample type this reader does not decode (it reads MODE 1, 2 and 6)"
        ))),
    }
}

/// A plane is read x fastest; any other axis order is not the layout this reader decodes.
fn axis_order(mapc: i32, mapr: i32, maps: i32) -> Result<()> {
    if [mapc, mapr, maps]
        .iter()
        .any(|axis| !(1..=3).contains(axis))
    {
        return Err(Error::Unsupported(format!(
            "MRC axis order MAPC/MAPR/MAPS {mapc}/{mapr}/{maps} is not the standard 1/2/3, so the section layout \
             is unknown"
        )));
    }
    Ok(())
}

/// An extent of zero or less is a corrupt header, not an empty image.
fn extent(name: &str, value: i32) -> Result<usize> {
    if value <= 0 {
        return Err(Error::Unsupported(format!(
            "MRC {name} is {value}: an image extent must be positive"
        )));
    }
    Ok(value as usize)
}

fn extended_bytes(nsymbt: i32) -> Result<usize> {
    if nsymbt < 0 {
        return Err(Error::Unsupported(format!(
            "MRC NSYMBT is {nsymbt}: a negative extended-header length is not a byte count"
        )));
    }
    Ok(nsymbt as usize)
}

/// Word `index` (0-based) is the i32 at byte `4 * index`.
fn word(buf: &[u8], index: usize, endian: Endian) -> Result<i32> {
    let at = index * 4;
    let raw = buf
        .get(at..at + 4)
        .ok_or_else(|| Error::Parse(format!("MRC header has no word {index}")))?;
    let bytes = [raw[0], raw[1], raw[2], raw[3]];
    Ok(match endian {
        Endian::Little => i32::from_le_bytes(bytes),
        Endian::Big => i32::from_be_bytes(bytes),
    })
}

fn real(buf: &[u8], index: usize, endian: Endian) -> Result<f32> {
    Ok(f32::from_bits(word(buf, index, endian)? as u32))
}
