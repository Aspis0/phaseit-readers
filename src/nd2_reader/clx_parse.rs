use std::io::Read;

use flate2::read::ZlibDecoder;

use crate::error::{Error, Result};

use super::clx::{clx_type, collapse_level, ClxMap, ClxValue};
use super::cursor::Cursor;

const MAX_CLX_DEPTH: usize = 64;

const MAX_CLX_ITEMS: usize = 10_000_000;

/// Upper bound on inflated CLX metadata (guards a zlib decompression bomb).
const MAX_INFLATED_CLX_BYTES: usize = 256 * 1024 * 1024;

pub(super) fn parse_clx_variant(data: &[u8]) -> Result<ClxMap> {
    let mut cur = Cursor::new(data);
    parse_clx_records(&mut cur, 1, 0)
}

fn parse_clx_records(cur: &mut Cursor, count: usize, depth: usize) -> Result<ClxMap> {
    if depth > MAX_CLX_DEPTH {
        return Err(Error::Parse("ND2 CLX metadata nested too deeply".to_string()));
    }
    let mut map = ClxMap::new();

    for _ in 0..count {
        // A short read at a record boundary is the natural end, as in the reference.
        if cur.remaining() < 2 {
            break;
        }
        let record_start = cur.tell();
        let data_type = cur.read_u8()?;
        let name_length = cur.read_u8()? as usize;

        if data_type == clx_type::COMPRESS {
            // COMPRESS replaces this level: inflate the rest and parse it at `depth + 1`, so MAX_CLX_DEPTH bounds
            // nested COMPRESS.
            // Resetting the depth here would let a nested chain overflow the native stack.
            cur.skip(10)?;
            let inflated = zlib_inflate(cur.rest(), MAX_INFLATED_CLX_BYTES)?;
            return parse_clx_records(&mut Cursor::new(&inflated), 1, depth + 1);
        }
        if data_type == 0 || data_type == 10 {
            return Err(Error::Parse(format!(
                "ND2 CLX unknown/deprecated data type {data_type}"
            )));
        }

        let name_bytes = cur.read_bytes(name_length.checked_mul(2).ok_or_else(|| {
            Error::Parse("ND2 CLX name length overflow".to_string())
        })?)?;
        let name = decode_utf16_name(name_bytes);

        let value = match data_type {
            clx_type::BOOL => ClxValue::Bool(cur.read_u8()? != 0),
            clx_type::INT32 => ClxValue::I32(cur.read_i32()?),
            clx_type::UINT32 => ClxValue::U32(cur.read_u32()?),
            clx_type::INT64 => ClxValue::I64(cur.read_i64()?),
            clx_type::UINT64 => ClxValue::U64(cur.read_u64()?),
            clx_type::DOUBLE => ClxValue::F64(cur.read_f64()?),
            clx_type::VOIDPOINTER => ClxValue::U64(cur.read_u64()?),
            clx_type::STRING => ClxValue::Str(read_clx_string(cur)?),
            clx_type::BYTEARRAY => {
                let size = cur.read_u64()?;
                let size = usize::try_from(size)
                    .map_err(|_| Error::Parse("ND2 CLX bytearray size overflow".to_string()))?;
                ClxValue::Bytes(cur.read_bytes(size)?.to_vec())
            }
            clx_type::LEVEL => {
                let item_count = cur.read_u32()? as usize;
                if item_count > MAX_CLX_ITEMS {
                    return Err(Error::Parse(format!(
                        "ND2 CLX LEVEL item_count {item_count} exceeds cap {MAX_CLX_ITEMS}"
                    )));
                }
                let length = cur.read_u64()?;
                let length = usize::try_from(length)
                    .map_err(|_| Error::Parse("ND2 CLX LEVEL length overflow".to_string()))?;
                let consumed = cur.tell() - record_start;
                let sub_len = length.checked_sub(consumed).ok_or_else(|| {
                    Error::Parse("ND2 CLX LEVEL length shorter than its header".to_string())
                })?;
                let sub = cur.read_bytes(sub_len)?;
                let child = parse_clx_records(&mut Cursor::new(sub), item_count, depth + 1)?;
                cur.skip(item_count.checked_mul(8).ok_or_else(|| {
                    Error::Parse("ND2 CLX LEVEL offset table overflow".to_string())
                })?)?;
                collapse_level(child)
            }
            other => {
                return Err(Error::Parse(format!(
                    "ND2 CLX unexpected data type {other}"
                )))
            }
        };

        map.insert(name, value);
    }

    Ok(map)
}

fn decode_utf16_name(bytes: &[u8]) -> String {
    let mut units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    if !units.is_empty() {
        units.pop();
    }
    String::from_utf16_lossy(&units)
}

fn read_clx_string(cur: &mut Cursor) -> Result<String> {
    let mut units: Vec<u16> = Vec::new();
    while cur.remaining() >= 2 {
        let u = cur.read_u16()?;
        if u == 0 {
            break;
        }
        units.push(u);
    }
    Ok(String::from_utf16_lossy(&units))
}

fn zlib_inflate(input: &[u8], cap: usize) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    ZlibDecoder::new(input)
        .take(cap as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| Error::Parse(format!("ND2 CLX zlib inflate failed: {e}")))?;
    if out.len() > cap {
        return Err(Error::Parse(format!(
            "ND2 CLX inflated metadata exceeds cap {cap} bytes"
        )));
    }
    Ok(out)
}
