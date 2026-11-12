// SPDX-License-Identifier: Apache-2.0

use crate::error::{Error, Result};
use crate::types::{DType, Endian};

pub(super) const HEADER_BYTES: usize = 1024;

const NDVID_OFFSET: usize = 96;
const NDVID_LE: i16 = -16224;

#[derive(Debug)]
pub(super) struct DvHeader {
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    pub dx: f32,
    pub dy: f32,
    pub dz: f32,
    pub next: usize,
    pub num_times: i16,
    pub img_sequence: i16,
    pub num_waves: i16,
    pub endian: Endian,
    pub dtype: DType,
}

pub(super) fn parse(buf: &[u8]) -> Result<DvHeader> {
    if buf.len() < HEADER_BYTES {
        return Err(Error::Parse(format!(
            "DeltaVision file is {} bytes, too short for the {HEADER_BYTES}-byte header",
            buf.len()
        )));
    }
    let endian = byte_order(buf)?;
    let nx = extent("NX", word(buf, 0, endian)?)?;
    let ny = extent("NY", word(buf, 1, endian)?)?;
    let nz = extent("NZ", word(buf, 2, endian)?)?;
    let dtype = dtype_of(word(buf, 3, endian)?)?;
    let dx = real(buf, 10, endian)?;
    let dy = real(buf, 11, endian)?;
    let dz = real(buf, 12, endian)?;
    let next = extended_bytes(word(buf, 23, endian)?)?;
    let num_times = short(buf, 180, endian)?;
    let img_sequence = short(buf, 182, endian)?;
    let num_waves = short(buf, 196, endian)?;
    Ok(DvHeader {
        nx,
        ny,
        nz,
        dx,
        dy,
        dz,
        next,
        num_times,
        img_sequence,
        num_waves,
        endian,
        dtype,
    })
}

fn byte_order(buf: &[u8]) -> Result<Endian> {
    let raw = buf
        .get(NDVID_OFFSET..NDVID_OFFSET + 2)
        .ok_or_else(|| Error::Parse("DeltaVision header has no nDVID magic".to_string()))?;
    let le = i16::from_le_bytes([raw[0], raw[1]]);
    if le == NDVID_LE {
        return Ok(Endian::Little);
    }
    let be = i16::from_be_bytes([raw[0], raw[1]]);
    if be == NDVID_LE {
        return Ok(Endian::Big);
    }
    Err(Error::Unsupported(
        "not a DeltaVision file: the nDVID magic at offset 96 is not -16224".to_string(),
    ))
}

fn dtype_of(mode: i32) -> Result<DType> {
    match mode {
        0 => Ok(DType::U8),
        1 => Ok(DType::I16),
        2 => Ok(DType::F32),
        6 => Ok(DType::U16),
        3 | 4 | 5 | 7 => Err(Error::Unsupported(format!(
            "DeltaVision MODE {mode} stores a sample type this crate cannot represent (complex or 32-bit integer)"
        ))),
        other => Err(Error::Unsupported(format!(
            "DeltaVision MODE {other} is unknown to this reader (it reads 0, 1, 2 and 6)"
        ))),
    }
}

fn extent(name: &str, value: i32) -> Result<usize> {
    if value <= 0 {
        return Err(Error::Unsupported(format!(
            "DeltaVision {name} is {value}: an image extent must be positive"
        )));
    }
    Ok(value as usize)
}

fn extended_bytes(next: i32) -> Result<usize> {
    if next < 0 {
        return Err(Error::Unsupported(format!(
            "DeltaVision next is {next}: a negative extended-header length is not a byte count"
        )));
    }
    Ok(next as usize)
}

fn word(buf: &[u8], index: usize, endian: Endian) -> Result<i32> {
    let at = index * 4;
    let raw = buf
        .get(at..at + 4)
        .ok_or_else(|| Error::Parse(format!("DeltaVision header has no word {index}")))?;
    let bytes = [raw[0], raw[1], raw[2], raw[3]];
    Ok(match endian {
        Endian::Little => i32::from_le_bytes(bytes),
        Endian::Big => i32::from_be_bytes(bytes),
    })
}

/// i16 at byte offset `offset` (the DV-specific fields are 2-byte words at explicit byte offsets).
fn short(buf: &[u8], offset: usize, endian: Endian) -> Result<i16> {
    let raw = buf
        .get(offset..offset + 2)
        .ok_or_else(|| Error::Parse(format!("DeltaVision header has no i16 at byte {offset}")))?;
    let bytes = [raw[0], raw[1]];
    Ok(match endian {
        Endian::Little => i16::from_le_bytes(bytes),
        Endian::Big => i16::from_be_bytes(bytes),
    })
}

fn real(buf: &[u8], index: usize, endian: Endian) -> Result<f32> {
    Ok(f32::from_bits(word(buf, index, endian)? as u32))
}
