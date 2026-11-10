use std::collections::HashMap;

use crate::error::{Error, Result};

pub(super) const ND2_CHUNK_MAGIC: u32 = 0x0ABE_CEDA;

/// Legacy JPEG2000 ND2 magic — recognized only to reject it clearly.
pub(super) const JP2_MAGIC: u32 = 0x0C00_0000;

const CHUNK_HEADER_LEN: usize = 16;

const CHUNKMAP_LOCATOR_LEN: usize = 40;

pub(super) const SIG_CHUNKMAP_END: &[u8; 32] = b"ND2 CHUNK MAP SIGNATURE 0000001!";

const SIG_FILEMAP: &[u8; 32] = b"ND2 FILEMAP SIGNATURE NAME 0001!";

/// Guards a crafted chunkmap; real ND2 counts are at most hundreds of thousands.
const MAX_MAP_ENTRIES: usize = 2_000_000;

pub(super) fn read_chunkmap(buf: &[u8]) -> Result<HashMap<Vec<u8>, (u64, u64)>> {
    if buf.len() < CHUNKMAP_LOCATOR_LEN {
        return Err(Error::Parse("ND2 file too small for a chunkmap locator".to_string()));
    }
    let tail = &buf[buf.len() - CHUNKMAP_LOCATOR_LEN..];
    if &tail[..32] != SIG_CHUNKMAP_END {
        return Err(Error::Parse("ND2 invalid chunkmap locator signature".to_string()));
    }
    let map_offset = u64::from_le_bytes(tail[32..40].try_into().unwrap());
    let blob = chunk_data(buf, map_offset, Some(SIG_FILEMAP))?;
    parse_chunkmap_blob(blob)
}

/// Chunkmap blob: repeated `name!`, `u64 offset`, `u64 size`, ended by the chunkmap signature.
pub(super) fn parse_chunkmap_blob(blob: &[u8]) -> Result<HashMap<Vec<u8>, (u64, u64)>> {
    let mut map: HashMap<Vec<u8>, (u64, u64)> = HashMap::new();
    let mut cur = 0usize;
    loop {
        let rel = blob[cur..]
            .iter()
            .position(|&b| b == b'!')
            .ok_or_else(|| Error::Parse("ND2 chunkmap blob truncated (no terminator)".to_string()))?;
        let bang = cur + rel;
        let name = &blob[cur..=bang];
        if name == SIG_CHUNKMAP_END {
            break;
        }
        let p = bang + 1;
        let field_end = p
            .checked_add(16)
            .filter(|&e| e <= blob.len())
            .ok_or_else(|| Error::Parse("ND2 chunkmap entry runs past the blob".to_string()))?;
        let offset = u64::from_le_bytes(blob[p..p + 8].try_into().unwrap());
        let mut size = u64::from_le_bytes(blob[p + 8..p + 16].try_into().unwrap());
        if size == offset {
            // The reference's unknown-size sentinel: `size` is never read, since each header is re-read at `offset`.
            size = u64::MAX;
        }
        if map.len() >= MAX_MAP_ENTRIES {
            return Err(Error::Parse(format!(
                "ND2 chunkmap exceeds sane cap {MAX_MAP_ENTRIES}"
            )));
        }
        map.insert(name.to_vec(), (offset, size));
        cur = field_end;
    }
    Ok(map)
}

/// Returns a chunk's data, re-reading and validating its 16-byte header at `offset`.
pub(super) fn chunk_data<'a>(buf: &'a [u8], offset: u64, expect_name: Option<&[u8]>) -> Result<&'a [u8]> {
    let off = usize::try_from(offset)
        .map_err(|_| Error::Parse("ND2 chunk offset overflows usize".to_string()))?;
    // `off` is attacker-controlled and may sit near usize::MAX: the header-end sum must be checked.
    let name_start = off
        .checked_add(CHUNK_HEADER_LEN)
        .ok_or_else(|| Error::Parse("ND2 chunk offset overflow".to_string()))?;
    let hdr = buf
        .get(off..name_start)
        .ok_or_else(|| Error::Parse("ND2 chunk header runs past EOF".to_string()))?;
    let magic = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
    if magic != ND2_CHUNK_MAGIC {
        return Err(Error::Parse(format!(
            "ND2 chunk at {off} has bad magic {magic:#010x}"
        )));
    }
    let name_len = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
    let data_len = u64::from_le_bytes(hdr[8..16].try_into().unwrap());
    let data_len = usize::try_from(data_len)
        .map_err(|_| Error::Parse("ND2 chunk data length overflows usize".to_string()))?;

    let name_end = name_start
        .checked_add(name_len)
        .filter(|&e| e <= buf.len())
        .ok_or_else(|| Error::Parse("ND2 chunk name runs past EOF".to_string()))?;
    if let Some(exp) = expect_name {
        if !buf[name_start..name_end].starts_with(exp) {
            return Err(Error::Parse("ND2 chunk name mismatch".to_string()));
        }
    }
    let data_end = name_end
        .checked_add(data_len)
        .filter(|&e| e <= buf.len())
        .ok_or_else(|| Error::Parse("ND2 chunk data runs past EOF".to_string()))?;
    Ok(&buf[name_end..data_end])
}
