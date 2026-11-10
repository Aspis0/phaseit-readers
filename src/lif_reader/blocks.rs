use std::collections::HashMap;

use crate::error::{Error, Result};

use super::xml::decode_utf16le;

/// Guards a corrupt or cyclic file; real files have hundreds of blocks at most.
const MAX_MEMORY_BLOCKS: usize = 1_000_000;

/// Stops at EOF or the first malformed header, as liffile does on a short read.
pub(super) fn walk_memory_blocks(
    buf: &[u8],
    start: usize,
    version: u32,
) -> Result<HashMap<String, (usize, usize)>> {
    // v2 (and LIFEXT) uses a u64 block size; v1 uses u32.
    let size_is_u64 = match version {
        2 => true,
        1 => false,
        other => {
            return Err(Error::Unsupported(format!(
                "LIF container version {other} not supported (expected 1 or 2)"
            )))
        }
    };
    // Fixed header: id0 u32 | descSize u32 | id1 u8 | size (u64/u32) | id2 u8 | strlen u32.
    let size_field = if size_is_u64 { 8 } else { 4 };
    let fixed = 4 + 4 + 1 + size_field + 1 + 4;

    let mut blocks: HashMap<String, (usize, usize)> = HashMap::new();
    let mut pos = start;
    let mut count = 0usize;

    while pos + fixed <= buf.len() {
        count += 1;
        if count > MAX_MEMORY_BLOCKS {
            return Err(Error::Parse(
                "LIF memory-block chain exceeds sane bound (corrupt?)".to_string(),
            ));
        }
        // A marker mismatch means the walk ran past the last block (padding or EOF): stop cleanly.
        if read_u32(buf, pos)? != 0x70 || buf[pos + 8] != 0x2A {
            break;
        }
        let size1 = if size_is_u64 {
            usize::try_from(read_u64(buf, pos + 9)?)
                .map_err(|_| Error::Decode("LIF block size overflows usize".to_string()))?
        } else {
            read_u32(buf, pos + 9)? as usize
        };
        let marker2_off = pos + 9 + size_field;
        if buf[marker2_off] != 0x2A {
            break;
        }
        let strlen = read_u32(buf, marker2_off + 1)? as usize;
        let id_off = marker2_off + 5;
        let id_end = match id_off
            .checked_add(strlen.saturating_mul(2))
            .filter(|&e| e <= buf.len())
        {
            Some(e) => e,
            None => break,
        };
        let id = decode_utf16le(&buf[id_off..id_end])?;

        let data_off = id_end;
        let data_end = match data_off.checked_add(size1).filter(|&e| e <= buf.len()) {
            Some(e) => e,
            None => break,
        };
        // Keep the first occurrence of an id, as liffile's dict insertion does for well-formed files.
        blocks.entry(id).or_insert((data_off, size1));
        pos = data_end;
    }

    Ok(blocks)
}

pub(super) fn read_u32(buf: &[u8], off: usize) -> Result<u32> {
    let s = buf
        .get(off..off + 4)
        .ok_or_else(|| bounds(off, 4, buf.len()))?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

pub(super) fn read_u64(buf: &[u8], off: usize) -> Result<u64> {
    let s = buf
        .get(off..off + 8)
        .ok_or_else(|| bounds(off, 8, buf.len()))?;
    Ok(u64::from_le_bytes([
        s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
    ]))
}

pub(super) fn bounds(off: usize, need: usize, len: usize) -> Error {
    Error::Parse(format!(
        "LIF read out of bounds: need {need} bytes at offset {off}, file is {len} bytes"
    ))
}
