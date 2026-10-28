// SPDX-License-Identifier: Apache-2.0
//! Nikon ND2 reader — modern, uncompressed frames.
//!
//! The ND2 container, its chunkmap, the `CLxLiteVariant` binary metadata format,
//! and the experiment-loop / coordinate model are ported from the *facts* and
//! byte layout of the reference reader `nd2` (Talley Lambert, BSD-3-Clause,
//! <https://github.com/tlambert03/nd2>). The attribution travels with the
//! derived knowledge as required by the license (see `THIRD-PARTY-NOTICES.md`).
//! Keep this attribution if you touch the container framing, the CLX parser, or
//! the loop/coordinate math.
//!
//! Most of the port is layout and decoding facts: no source code was copied.
//! Two parts are more than that, and both are Rust re-implementations of `nd2`
//! functions, BSD-3-Clause, Copyright (c) Talley Lambert:
//!
//! - the experiment-loop parser (`load_experiment_rec` and its helpers
//!   `single_experiment_loop`, `xy_point_count` and `as_loop_params`), derived
//!   from `nd2`'s `load_experiment` (`nd2/src/nd2/_parse/_parse.py`);
//! - the attribute and channel parsers (`parse_attributes` and
//!   `parse_channels`), derived from `nd2`'s `load_attributes` and
//!   `load_metadata`.
//!
//! # Container shape
//!
//! A modern `.nd2` is a flat sequence of **chunks**. Each chunk is a 16-byte
//! header (`u32 magic = 0x0ABECEDA`, `u32 name_len`, `u64 data_len`), then
//! `name_len` name bytes, then `data_len` data bytes. Everything is
//! little-endian (Intel).
//!
//! The last 40 bytes of the file are `char sig[32]` (`ND2 CHUNK MAP SIGNATURE
//! 0000001!`) + `u64 map_offset`. The chunk at `map_offset` (`ND2 FILEMAP
//! SIGNATURE NAME 0001!`) holds the **chunkmap** blob, a list of
//! `name!` + `u64 offset` + `u64 size` entries terminated by the chunkmap
//! signature. Each `offset` points at a chunk *header* (not its data).
//!
//! # Scope (MVP)
//!
//! * Modern ND2 only (chunk magic `0x0ABECEDA`); legacy JPEG2000 ND2
//!   (`0x0C000000`) is `Error::Unsupported`.
//! * Uncompressed frames only (`eCompression == 2`, the value both real
//!   samples state); every other value — and a file that states none — is
//!   `Error::Unsupported`, naming the value it refused.
//! * Experiment loops limited to `{TimeLoop, NETimeLoop -> T, ZStackLoop -> Z,
//!   XYPosLoop -> scene/P}` (`SpectLoop` is skipped, per the reference). Any
//!   other surviving loop type is `Error::Unsupported`.
//!
//! # Frame layout (byte-exact)
//!
//! An `ImageDataSeq|{n}!` chunk's data is an 8-byte prefix (skipped) then the
//! pixel block of `height * widthBytes` bytes, interleaved `(Y, X, C)` row-major
//! with a per-row stride of `widthBytes` BYTES (which may exceed
//! `width * comp * itemsize` due to padding). Each component is returned as its
//! own grayscale plane (`samples == 1`), matching the LSM/LIF convention.
//!
//! Because the rows sit at computable byte offsets, a SUB-REGION of a plane
//! can be read without decoding the whole plane: `region()` gathers only the
//! rows the rectangle covers (one copy per row, or per pixel when the frame
//! interleaves several components). The result is byte-identical to cropping
//! the whole plane.
//!
//! A file whose plane is over the decode ceiling
//! ([`MAX_DECODED_PLANE_BYTES`](crate::guard::MAX_DECODED_PLANE_BYTES)) is
//! NOT refused: the ceiling guards ONE materialisation, not the file's
//! existence. The file is published, `plane()` refuses the materialisation
//! with [`Error::OversizedAllocation`], and `region()` serves windows of it —
//! exactly as the CZI reader does for a whole-slide scene.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use flate2::read::ZlibDecoder;
use memmap2::Mmap;

use crate::calibration::Calibration;
use crate::error::{Error, Result};
use crate::io::map_file;
use crate::types::{
    checked_region, empty_region_plane, ChannelMeta, DType, Dims5, Endian, ImageMeta, ImageReader,
    PixelRect, Plane, PlaneBuf, SceneMeta,
};

// --- container constants ------------------------------------------------------

/// Modern ND2 chunk magic (`0x0ABECEDA`, little-endian).
const ND2_CHUNK_MAGIC: u32 = 0x0ABE_CEDA;
/// Legacy JPEG2000 ND2 magic — recognized only to reject it clearly.
const JP2_MAGIC: u32 = 0x0C00_0000;
/// Length of a chunk header (`magic` + `name_len` + `data_len`).
const CHUNK_HEADER_LEN: usize = 16;
/// Trailing signature-and-location record at the very end of the file.
const CHUNKMAP_LOCATOR_LEN: usize = 40;

/// Signature at the end of the chunkmap blob (also the trailing locator sig).
const SIG_CHUNKMAP_END: &[u8; 32] = b"ND2 CHUNK MAP SIGNATURE 0000001!";
/// Name of the chunk holding the chunkmap blob.
const SIG_FILEMAP: &[u8; 32] = b"ND2 FILEMAP SIGNATURE NAME 0001!";

// --- safety caps --------------------------------------------------------------

/// Upper bound on chunkmap entries (guards a crafted / corrupt blob). Matches
/// CZI's `MAX_SUBBLOCKS`; real ND2 chunk counts are <= hundreds of thousands.
const MAX_MAP_ENTRIES: usize = 2_000_000;
/// Upper bound on nesting depth of the CLX metadata tree.
const MAX_CLX_DEPTH: usize = 64;
/// Upper bound on item count of one CLX LEVEL (guards a corrupt `item_count`).
const MAX_CLX_ITEMS: usize = 10_000_000;
/// Upper bound on experiment-loop recursion depth.
const MAX_EXP_DEPTH: usize = 64;
/// Highest experiment loop-type id the SDK defines (`ZStackLoopAccurate`).
/// Ids above this are not loops; in-range-but-unhandled ids fail loud.
const MAX_LOOP_TYPE: i64 = 10;
/// Upper bound on any single loop count (scenes / T / Z all flow from these).
/// Real ND2 positions/timepoints/z-planes are <= thousands; this caps the
/// `Vec<SceneMeta>` and dims so a crafted count cannot force an uncatchable
/// allocation abort.
const MAX_LOOP_COUNT: usize = 1_000_000;
/// Upper bound on component count (channels) in one frame.
const MAX_COMPONENTS: usize = 1_024;
/// The `eCompression` value that means "the frames are stored uncompressed",
/// and the only one the reader decodes. Both real samples state exactly this.
const COMPRESSION_NONE: i64 = 2;
/// Upper bound on inflated CLX metadata (guards a zlib decompression bomb).
const MAX_INFLATED_CLX_BYTES: usize = 256 * 1024 * 1024;

// --- little-endian cursor over the mapped bytes -------------------------------

/// A bounds-checked forward cursor over a byte slice. Every read that would run
/// past the end returns [`Error::Parse`] rather than panicking.
struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Cursor { buf, pos: 0 }
    }

    fn tell(&self) -> usize {
        self.pos
    }

    fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    fn read_bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.buf.len())
            .ok_or_else(|| clx_bounds(self.pos, n, self.buf.len()))?;
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn rest(&self) -> &'a [u8] {
        &self.buf[self.pos.min(self.buf.len())..]
    }

    fn skip(&mut self, n: usize) -> Result<()> {
        self.read_bytes(n).map(|_| ())
    }

    fn read_u8(&mut self) -> Result<u8> {
        Ok(self.read_bytes(1)?[0])
    }

    fn read_u16(&mut self) -> Result<u16> {
        let s = self.read_bytes(2)?;
        Ok(u16::from_le_bytes([s[0], s[1]]))
    }

    fn read_i32(&mut self) -> Result<i32> {
        let s = self.read_bytes(4)?;
        Ok(i32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    fn read_u32(&mut self) -> Result<u32> {
        let s = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    fn read_i64(&mut self) -> Result<i64> {
        let s = self.read_bytes(8)?;
        Ok(i64::from_le_bytes(s.try_into().unwrap()))
    }

    fn read_u64(&mut self) -> Result<u64> {
        let s = self.read_bytes(8)?;
        Ok(u64::from_le_bytes(s.try_into().unwrap()))
    }

    fn read_f64(&mut self) -> Result<f64> {
        Ok(f64::from_bits(self.read_u64()?))
    }
}

fn clx_bounds(off: usize, need: usize, len: usize) -> Error {
    Error::Parse(format!(
        "ND2 read out of bounds: need {need} bytes at offset {off}, buffer is {len} bytes"
    ))
}

// --- CLxLiteVariant metadata tree ---------------------------------------------

/// A decoded `CLxLiteVariant` value. Only the pieces this reader needs are
/// modelled; unhandled kinds (bytearrays) are kept opaque.
#[allow(dead_code)] // general-purpose tree; not every kind/getter is used by the MVP
#[derive(Debug, Clone, PartialEq)]
enum ClxValue {
    Bool(bool),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    F64(f64),
    Str(String),
    Bytes(Vec<u8>),
    Level(ClxMap),
    List(Vec<ClxValue>),
}

#[allow(dead_code)]
impl ClxValue {
    /// Coerce any integer-ish scalar to `u64` (rejecting negatives / floats < 0).
    fn as_u64(&self) -> Option<u64> {
        match self {
            ClxValue::Bool(b) => Some(*b as u64),
            ClxValue::I32(v) => u64::try_from(*v).ok(),
            ClxValue::U32(v) => Some(*v as u64),
            ClxValue::I64(v) => u64::try_from(*v).ok(),
            ClxValue::U64(v) => Some(*v),
            ClxValue::F64(v) if *v >= 0.0 => Some(*v as u64),
            _ => None,
        }
    }

    fn as_f64(&self) -> Option<f64> {
        match self {
            ClxValue::Bool(b) => Some(*b as u64 as f64),
            ClxValue::I32(v) => Some(*v as f64),
            ClxValue::U32(v) => Some(*v as f64),
            ClxValue::I64(v) => Some(*v as f64),
            ClxValue::U64(v) => Some(*v as f64),
            ClxValue::F64(v) => Some(*v),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            ClxValue::Str(s) => Some(s),
            _ => None,
        }
    }

    fn as_level(&self) -> Option<&ClxMap> {
        match self {
            ClxValue::Level(m) => Some(m),
            _ => None,
        }
    }

    /// Truthy in the LIM sense (used for `pItemValid` flags).
    fn is_truthy(&self) -> bool {
        self.as_u64().map(|v| v != 0).unwrap_or(false)
    }
}

/// An ordered map of `(name, value)` records — the shape of one CLX LEVEL. Names
/// are usually distinct; repeated empty (`""`) names collect into a list, exactly
/// as the reference format uses empty names for list items.
#[derive(Debug, Clone, PartialEq)]
struct ClxMap {
    entries: Vec<(String, ClxValue)>,
}

#[allow(dead_code)]
impl ClxMap {
    fn new() -> Self {
        ClxMap { entries: Vec::new() }
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn get(&self, key: &str) -> Option<&ClxValue> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn get_level(&self, key: &str) -> Option<&ClxMap> {
        self.get(key).and_then(ClxValue::as_level)
    }

    fn get_u32(&self, key: &str) -> Option<u32> {
        self.get(key).and_then(ClxValue::as_u64).and_then(|v| u32::try_from(v).ok())
    }

    fn get_usize(&self, key: &str) -> Option<usize> {
        self.get(key).and_then(ClxValue::as_u64).and_then(|v| usize::try_from(v).ok())
    }

    fn get_i64(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(|v| match v {
            ClxValue::Bool(b) => Some(*b as i64),
            ClxValue::I32(x) => Some(*x as i64),
            ClxValue::U32(x) => Some(*x as i64),
            ClxValue::I64(x) => Some(*x),
            ClxValue::U64(x) => i64::try_from(*x).ok(),
            ClxValue::F64(x) => Some(*x as i64),
            _ => None,
        })
    }

    fn get_f64(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(ClxValue::as_f64)
    }

    fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(ClxValue::as_str)
    }

    /// Insert one record, replicating the reference's empty-name list behaviour:
    /// a repeated `""` key accumulates its values into a [`ClxValue::List`]; a
    /// repeated non-empty key overwrites (last wins).
    fn insert(&mut self, name: String, value: ClxValue) {
        if name.is_empty() {
            if let Some(slot) = self.entries.iter_mut().find(|(k, _)| k.is_empty()) {
                match &mut slot.1 {
                    ClxValue::List(v) => v.push(value),
                    _ => {
                        let old = std::mem::replace(&mut slot.1, ClxValue::List(Vec::new()));
                        if let ClxValue::List(v) = &mut slot.1 {
                            v.push(old);
                            v.push(value);
                        }
                    }
                }
                return;
            }
        } else if let Some(slot) = self.entries.iter_mut().find(|(k, _)| *k == name) {
            slot.1 = value;
            return;
        }
        self.entries.push((name, value));
    }
}

/// CLX record type ids (`ELxLiteVariantType`).
mod clx_type {
    pub const BOOL: u8 = 1;
    pub const INT32: u8 = 2;
    pub const UINT32: u8 = 3;
    pub const INT64: u8 = 4;
    pub const UINT64: u8 = 5;
    pub const DOUBLE: u8 = 6;
    pub const VOIDPOINTER: u8 = 7;
    pub const STRING: u8 = 8;
    pub const BYTEARRAY: u8 = 9;
    pub const LEVEL: u8 = 11;
    pub const COMPRESS: u8 = 76; // 'L'
}

/// Parse a whole `CLxLiteVariant` blob (a single top-level record, `count == 1`).
fn parse_clx_variant(data: &[u8]) -> Result<ClxMap> {
    let mut cur = Cursor::new(data);
    parse_clx_records(&mut cur, 1, 0)
}

/// Parse `count` consecutive CLX records from `cur` into a [`ClxMap`].
fn parse_clx_records(cur: &mut Cursor, count: usize, depth: usize) -> Result<ClxMap> {
    if depth > MAX_CLX_DEPTH {
        return Err(Error::Parse("ND2 CLX metadata nested too deeply".to_string()));
    }
    let mut map = ClxMap::new();

    for _ in 0..count {
        // A short read at a record boundary marks the natural end (matches the
        // reference, which yields ("", -1) and breaks).
        if cur.remaining() < 2 {
            break;
        }
        let record_start = cur.tell();
        let data_type = cur.read_u8()?;
        let name_length = cur.read_u8()? as usize;

        if data_type == clx_type::COMPRESS {
            // COMPRESS: no name; skip 10 bytes, then zlib-inflate the REST and
            // parse it as a single record — it replaces this level. Thread
            // `depth + 1` (NOT `parse_clx_variant`, which resets depth to 0) so
            // that MAX_CLX_DEPTH also bounds a nested-COMPRESS matryoshka chain
            // (otherwise tens of thousands of layers overflow the native stack).
            cur.skip(10)?;
            let inflated = zlib_inflate(cur.rest(), MAX_INFLATED_CLX_BYTES)?;
            return parse_clx_records(&mut Cursor::new(&inflated), 1, depth + 1);
        }
        if data_type == 0 || data_type == 10 {
            return Err(Error::Parse(format!(
                "ND2 CLX unknown/deprecated data type {data_type}"
            )));
        }

        // name = UTF-16LE of `name_length * 2` bytes, drop the trailing NUL.
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
                // Bytes consumed since the record start (header + name + IQ).
                let consumed = cur.tell() - record_start;
                let sub_len = length.checked_sub(consumed).ok_or_else(|| {
                    Error::Parse("ND2 CLX LEVEL length shorter than its header".to_string())
                })?;
                let sub = cur.read_bytes(sub_len)?;
                let child = parse_clx_records(&mut Cursor::new(sub), item_count, depth + 1)?;
                // Skip the trailing offset table (`item_count` x u64).
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

/// Collapse a LEVEL whose only entry is the empty-name list marker into a
/// [`ClxValue::List`] (mirrors the reference `if len(val)==1 and "" in val`).
fn collapse_level(mut child: ClxMap) -> ClxValue {
    if child.entries.len() == 1 && child.entries[0].0.is_empty() {
        match child.entries.pop().unwrap().1 {
            ClxValue::List(v) => ClxValue::List(v),
            single => ClxValue::List(vec![single]),
        }
    } else {
        ClxValue::Level(child)
    }
}

/// Decode a fixed-length UTF-16LE name, dropping the trailing NUL code unit.
fn decode_utf16_name(bytes: &[u8]) -> String {
    let mut units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    if !units.is_empty() {
        units.pop(); // trailing NUL
    }
    String::from_utf16_lossy(&units)
}

/// Read a NUL-terminated UTF-16LE string value (2-byte steps until `\x00\x00`).
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

/// Zlib-inflate `input`, refusing output larger than `cap` (decompression bomb).
fn zlib_inflate(input: &[u8], cap: usize) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    // Read at most `cap + 1` bytes: one extra proves the stream is over-cap.
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

// --- chunkmap -----------------------------------------------------------------

/// Read and parse the chunkmap: `name -> (offset, size)` for every chunk.
fn read_chunkmap(buf: &[u8]) -> Result<HashMap<Vec<u8>, (u64, u64)>> {
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

/// Parse the chunkmap blob: repeated `name!` + `u64 offset` + `u64 size`,
/// terminated by the chunkmap signature.
fn parse_chunkmap_blob(blob: &[u8]) -> Result<HashMap<Vec<u8>, (u64, u64)>> {
    let mut map: HashMap<Vec<u8>, (u64, u64)> = HashMap::new();
    let mut cur = 0usize;
    loop {
        // The chunk name runs from `cur` up to and including the next '!'.
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
            // Reference sentinel for "unknown size"; we never rely on `size`
            // (we re-read each chunk header at `offset`), so mark and move on.
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

/// Return a chunk's DATA slice, given its header offset. Re-reads the 16-byte
/// header, validates the magic, optionally checks the name prefix, and bounds
/// the data against the buffer.
fn chunk_data<'a>(buf: &'a [u8], offset: u64, expect_name: Option<&[u8]>) -> Result<&'a [u8]> {
    let off = usize::try_from(offset)
        .map_err(|_| Error::Parse("ND2 chunk offset overflows usize".to_string()))?;
    // `off` is an attacker-controlled file offset that can sit near usize::MAX,
    // so `off + CHUNK_HEADER_LEN` must be checked (a raw `+` panics in debug).
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

// --- attributes ---------------------------------------------------------------

/// The `SLxImageAttributes` fields this reader consumes.
struct Nd2Attributes {
    width: usize,
    width_bytes: usize,
    height: usize,
    comp: usize,
    dtype: DType,
}

/// The image attributes (`SLxImageAttributes`), read as `nd2`'s
/// `load_attributes` reads them.
fn parse_attributes(buf: &[u8], map: &HashMap<Vec<u8>, (u64, u64)>) -> Result<Nd2Attributes> {
    let (off, _) = map
        .get(b"ImageAttributesLV!".as_slice())
        .ok_or_else(|| Error::Parse("ND2 missing ImageAttributesLV! chunk".to_string()))?;
    let data = chunk_data(buf, *off, None)?;
    let root = parse_clx_variant(data)?;
    let a = root
        .get_level("SLxImageAttributes")
        .ok_or_else(|| Error::Parse("ND2 missing SLxImageAttributes".to_string()))?;

    let width = a
        .get_usize("uiWidth")
        .ok_or_else(|| Error::Parse("ND2 attributes missing uiWidth".to_string()))?;
    let width_bytes = a
        .get_usize("uiWidthBytes")
        .ok_or_else(|| Error::Parse("ND2 attributes missing uiWidthBytes".to_string()))?;
    let height = a
        .get_usize("uiHeight")
        .ok_or_else(|| Error::Parse("ND2 attributes missing uiHeight".to_string()))?;
    let comp = a
        .get_usize("uiComp")
        .ok_or_else(|| Error::Parse("ND2 attributes missing uiComp".to_string()))?;
    let bpc = a
        .get_u32("uiBpcInMemory")
        .ok_or_else(|| Error::Parse("ND2 attributes missing uiBpcInMemory".to_string()))?;

    // Compression: the reader decodes UNCOMPRESSED frames only, and the value
    // it accepts is the one Nikon means by "no compression" — `2`. Both real
    // samples (the reference stack and the corpus file) state exactly that,
    // so there is no evidence for reading any other value as uncompressed.
    // Every other value is refused BY VALUE, so the reason names what the
    // file said instead of guessing what it meant; a file that states NO
    // value at all is refused too, because an absent value says nothing about
    // how the frames are stored.
    match a.get_i64("eCompression") {
        Some(e) if e != COMPRESSION_NONE => {
            return Err(Error::Unsupported(format!(
                "ND2 eCompression {e} is not uncompressed ({COMPRESSION_NONE}); compressed ND2 \
                 not yet supported"
            )));
        }
        None => {
            return Err(Error::Unsupported(
                "ND2 states no eCompression; only uncompressed frames are supported".to_string(),
            ));
        }
        Some(_) => {}
    }

    let dtype = match bpc {
        8 => DType::U8,
        16 => DType::U16,
        32 => DType::F32,
        other => {
            return Err(Error::Unsupported(format!(
                "ND2 uiBpcInMemory {other} not supported (expected 8/16/32)"
            )))
        }
    };

    if width == 0 || height == 0 || comp == 0 {
        return Err(Error::Parse("ND2 attributes have a zero dimension".to_string()));
    }
    if comp > MAX_COMPONENTS {
        return Err(Error::Unsupported(format!(
            "ND2 component count {comp} exceeds cap {MAX_COMPONENTS}"
        )));
    }
    // The row stride must hold every component of every column.
    let min_stride = width
        .checked_mul(comp)
        .and_then(|v| v.checked_mul(dtype.size_bytes()))
        .ok_or_else(|| Error::Parse("ND2 row size overflow".to_string()))?;
    if width_bytes < min_stride {
        return Err(Error::Parse(format!(
            "ND2 uiWidthBytes {width_bytes} is smaller than width*comp*itemsize {min_stride}"
        )));
    }

    Ok(Nd2Attributes { width, width_bytes, height, comp, dtype })
}

// --- experiment loops ---------------------------------------------------------

/// A frame-sequence axis a loop maps onto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    /// Time (TimeLoop / NETimeLoop).
    T,
    /// Focal depth (ZStackLoop).
    Z,
    /// Stage position / scene (XYPosLoop).
    P,
}

/// Reference loop type ids (`ELxExperimentLoopType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopType {
    Time,   // 1
    XYPos,  // 2
    ZStack, // 4
    Spect,  // 6
    Custom, // 7
    NETime, // 8
}

/// A loop while the ordered flattened list is being built (keeps `nesting` for
/// the reference's replace-at-same-level dedup).
struct BuiltLoop {
    ltype: LoopType,
    count: usize,
    nesting: i64,
}

/// A flattened, axis-mapped experiment loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Loop {
    axis: Axis,
    count: usize,
}

/// Parse the `SLxExperiment` tree into an ordered (outer -> inner) list of
/// flattened, axis-mapped loops. `SpectLoop` is skipped; unsupported loops error.
fn load_experiment(exp: &ClxMap) -> Result<Vec<Loop>> {
    let mut built: Vec<BuiltLoop> = Vec::new();
    load_experiment_rec(exp, 0, &mut built, 0)?;

    let mut loops: Vec<Loop> = Vec::new();
    let mut seen = [false; 3]; // T, Z, P
    for b in &built {
        let axis = match b.ltype {
            LoopType::Time | LoopType::NETime => Axis::T,
            LoopType::ZStack => Axis::Z,
            LoopType::XYPos => Axis::P,
            LoopType::Custom => {
                return Err(Error::Unsupported(
                    "ND2 CustomLoop is not supported".to_string(),
                ))
            }
            LoopType::Spect => continue, // never appended, but be defensive
        };
        let slot = match axis {
            Axis::T => 0,
            Axis::Z => 1,
            Axis::P => 2,
        };
        if seen[slot] {
            return Err(Error::Unsupported(format!(
                "ND2 has multiple {axis:?} loops, which is not supported"
            )));
        }
        seen[slot] = true;
        loops.push(Loop { axis, count: b.count });
    }
    Ok(loops)
}

/// Recursively walk one experiment record, appending its loop (unless SpectLoop)
/// and descending into `ppNextLevelEx`. Mirrors `load_experiment` in the
/// reference, including the level bookkeeping and same-level dedup.
fn load_experiment_rec(
    exp: &ClxMap,
    level: i64,
    dest: &mut Vec<BuiltLoop>,
    depth: usize,
) -> Result<()> {
    if depth > MAX_EXP_DEPTH {
        return Err(Error::Parse("ND2 experiment tree nested too deeply".to_string()));
    }

    let (ltype, count) = match single_experiment_loop(exp)? {
        Some(x) => x,
        None => return Ok(()),
    };
    if count == 0 {
        return Ok(());
    }

    let effective_level = if ltype == LoopType::Spect {
        level - 1
    } else {
        if dest.is_empty() || dest.last().unwrap().nesting < level {
            dest.push(BuiltLoop { ltype, count, nesting: level });
        } else {
            let prev = dest.last_mut().unwrap();
            if prev.nesting == level && prev.ltype == ltype && prev.count < count {
                prev.count = count;
            }
            // else: reference drops the loop (no append)
        }
        level
    };

    if let Some(next) = exp.get("ppNextLevelEx") {
        for child in exp_children(next) {
            load_experiment_rec(child, effective_level + 1, dest, depth + 1)?;
        }
    }
    Ok(())
}

/// Extract the loop type and count from one experiment record. Mirrors
/// `_load_single_experiment_loop`.
///
/// Returns `Ok(None)` when the record carries no loop (missing/empty
/// `uLoopPars`, `Unknown` type `0`, or an id above the SDK ceiling
/// [`MAX_LOOP_TYPE`], or a zero-count TimeLoop). An in-range but UNHANDLED SDK
/// type (`XYDiscrLoop=3`, `PolarLoop=5`, `ManTimeLoop=9`, `ZStackLoopAccurate=10`)
/// is an `Err(Unsupported)`: the reference raises rather than silently dropping
/// the subtree, which would otherwise mis-decode dims / alias frames with no
/// error. Any loop count over [`MAX_LOOP_COUNT`] is also an `Err(Unsupported)`.
fn single_experiment_loop(exp: &ClxMap) -> Result<Option<(LoopType, usize)>> {
    let e_type = exp.get_i64("eType").unwrap_or(0);
    let lp = match exp.get("uLoopPars").and_then(as_loop_params) {
        Some(m) => m,
        None => return Ok(None),
    };
    // Unknown (0) is not a loop; ids above the SDK ceiling are not loops either
    // (mirrors the reference's `loop_type > max(LoopType)` -> None).
    if e_type == 0 || e_type > MAX_LOOP_TYPE {
        return Ok(None);
    }

    let (ltype, count) = match e_type {
        1 => {
            // TimeLoop: no count -> not a loop (reference returns None).
            let count = lp.get_usize("uiCount").unwrap_or(0);
            if count == 0 {
                return Ok(None);
            }
            (LoopType::Time, count)
        }
        2 => (LoopType::XYPos, xy_point_count(exp, lp)),
        4 => (LoopType::ZStack, lp.get_usize("uiCount").unwrap_or(0)),
        6 => {
            // SpectLoop count: uiCount, overridden by pPlanes.uiCount when present.
            let mut count = lp.get_usize("uiCount").unwrap_or(0);
            if let Some(planes) = lp.get_level("pPlanes") {
                if let Some(c) = planes.get_usize("uiCount") {
                    count = c;
                }
            }
            (LoopType::Spect, count)
        }
        7 => (LoopType::Custom, lp.get_usize("uiCount").unwrap_or(0)),
        8 => (LoopType::NETime, lp.get_usize("uiCount").unwrap_or(0)),
        // In-range (3,5,9,10) but unhandled: fail loud, do not drop the subtree.
        other => {
            return Err(Error::Unsupported(format!(
                "ND2 experiment loop type {other} is not supported"
            )))
        }
    };

    if count > MAX_LOOP_COUNT {
        return Err(Error::Unsupported(format!(
            "ND2 loop count {count} exceeds cap {MAX_LOOP_COUNT}"
        )));
    }
    Ok(Some((ltype, count)))
}

/// Resolve a `uLoopPars` value to a map, unwrapping the single-item indexed-list
/// shape seen in older files (`{"i0000000000": {...}}` -> the inner map).
fn as_loop_params(v: &ClxValue) -> Option<&ClxMap> {
    match v {
        ClxValue::Level(m) => Some(m),
        ClxValue::List(l) if l.len() == 1 => l[0].as_level(),
        _ => None,
    }
}

/// Count valid XY positions: number of `Points`, filtered by `pItemValid` flags.
fn xy_point_count(exp: &ClxMap, lp: &ClxMap) -> usize {
    let points: Vec<()> = match lp.get("Points") {
        Some(ClxValue::List(l)) => l.iter().map(|_| ()).collect(),
        Some(ClxValue::Level(m)) => m.entries.iter().map(|_| ()).collect(),
        _ => Vec::new(),
    };
    let n_points = points.len();
    if n_points == 0 {
        // Fall back to uiCount when Points is absent.
        return lp.get_usize("uiCount").unwrap_or(0);
    }
    match exp.get("pItemValid") {
        Some(ClxValue::List(valid)) if !valid.is_empty() => valid
            .iter()
            .take(n_points)
            .filter(|v| v.is_truthy())
            .count(),
        _ => n_points,
    }
}

/// Yield each child experiment record from a `ppNextLevelEx` value (a list of
/// levels, or a level whose values are levels).
fn exp_children(v: &ClxValue) -> Vec<&ClxMap> {
    match v {
        ClxValue::List(l) => l.iter().filter_map(ClxValue::as_level).collect(),
        ClxValue::Level(m) => m.entries.iter().filter_map(|(_, x)| x.as_level()).collect(),
        _ => Vec::new(),
    }
}

// --- coordinate mapping -------------------------------------------------------

/// Number of scenes = the XYPosLoop (P) count if present, else 1.
fn scene_count(loops: &[Loop]) -> usize {
    loops
        .iter()
        .find(|l| l.axis == Axis::P)
        .map(|l| l.count)
        .unwrap_or(1)
}

/// Count along an axis (loop count if present, else 1).
fn axis_count(loops: &[Loop], axis: Axis) -> usize {
    loops.iter().find(|l| l.axis == axis).map(|l| l.count).unwrap_or(1)
}

/// Row-major sequence index for `(scene, t, z)`: fold outer -> inner with the
/// inner loop varying fastest (`seq = seq * count + index`). C is not a loop
/// axis (components live within each frame), so it does not participate.
fn seq_index(loops: &[Loop], scene: usize, t: usize, z: usize) -> Result<usize> {
    let mut seq = 0usize;
    for l in loops {
        let idx = match l.axis {
            Axis::P => scene,
            Axis::T => t,
            Axis::Z => z,
        };
        seq = seq
            .checked_mul(l.count)
            .and_then(|s| s.checked_add(idx))
            .ok_or_else(|| Error::Parse("ND2 sequence index overflow".to_string()))?;
    }
    Ok(seq)
}

// --- best-effort metadata (never gates open) ----------------------------------

/// X/Y calibration in micrometres per pixel from `ImageCalibrationLV|0!`.
fn parse_calibration_um(buf: &[u8], map: &HashMap<Vec<u8>, (u64, u64)>) -> Option<f64> {
    let (off, _) = map.get(b"ImageCalibrationLV|0!".as_slice())?;
    let data = chunk_data(buf, *off, None).ok()?;
    let root = parse_clx_variant(data).ok()?;
    let cal = root.get_level("SLxCalibration")?;
    cal.get_f64("dCalibration").filter(|&v| v > 0.0)
}

/// Z step in micrometres from the ZStackLoop parameters (`dZStep`, or derived
/// from `dZHigh`/`dZLow`/count). Best-effort; recurses into `ppNextLevelEx`.
fn find_z_step(exp: &ClxMap, depth: usize) -> Option<f64> {
    if depth > MAX_EXP_DEPTH {
        return None;
    }
    if exp.get_i64("eType") == Some(4) {
        if let Some(lp) = exp.get("uLoopPars").and_then(as_loop_params) {
            if let Some(step) = lp.get_f64("dZStep").filter(|&v| v != 0.0) {
                return Some(step.abs());
            }
            let count = lp.get_usize("uiCount").unwrap_or(0);
            if count > 1 {
                if let (Some(hi), Some(lo)) = (lp.get_f64("dZHigh"), lp.get_f64("dZLow")) {
                    return Some((hi - lo).abs() / (count as f64 - 1.0));
                }
            }
        }
    }
    if let Some(next) = exp.get("ppNextLevelEx") {
        for child in exp_children(next) {
            if let Some(s) = find_z_step(child, depth + 1) {
                return Some(s);
            }
        }
    }
    None
}

/// Per-channel names/colors from `ImageMetadataSeqLV|0!`, padded to `comp`.
/// Best-effort: any parse miss yields defaults. Read as `nd2`'s `load_metadata`
/// reads them.
fn parse_channels(buf: &[u8], map: &HashMap<Vec<u8>, (u64, u64)>, comp: usize) -> Vec<ChannelMeta> {
    let mut out: Vec<ChannelMeta> = Vec::new();
    if let Some((off, _)) = map.get(b"ImageMetadataSeqLV|0!".as_slice()) {
        if let Ok(data) = chunk_data(buf, *off, None) {
            if let Ok(root) = parse_clx_variant(data) {
                let planes = root
                    .get_level("SLxPictureMetadata")
                    .and_then(|m| m.get_level("sPicturePlanes"))
                    .and_then(|pp| pp.get_level("sPlaneNew").or_else(|| pp.get_level("sPlane")));
                if let Some(planes) = planes {
                    for (_, v) in &planes.entries {
                        if let ClxValue::Level(plane) = v {
                            let name = plane
                                .get_str("sDescription")
                                .map(str::to_string)
                                .filter(|s| !s.is_empty());
                            let color = plane.get_u32("uiColor").map(abgr_to_rgb);
                            out.push(ChannelMeta { name, color });
                        }
                    }
                }
            }
        }
    }
    // NOTE: this maps one metadata plane record -> one physical component (1:1).
    // The reference allows a single plane record to carry `uiCompCount > 1`
    // (RGB-style), in which case the trailing components get default (unnamed,
    // uncolored) metadata here. Metadata-only, best-effort; no effect on pixels.
    out.resize(comp, ChannelMeta::default());
    out
}

/// Decode a LIM `uiColor` (packed ABGR, R in the low byte) to display `[R,G,B]`.
fn abgr_to_rgb(v: u32) -> [u8; 3] {
    [(v & 0xFF) as u8, ((v >> 8) & 0xFF) as u8, ((v >> 16) & 0xFF) as u8]
}

// --- reader -------------------------------------------------------------------

/// Nikon ND2 reader (modern container, uncompressed frames). `Mmap` is
/// `Send + Sync` and every other field is plain data, so the reader is shareable.
pub struct Nd2Reader {
    mmap: Mmap,
    meta: ImageMeta,
    map: HashMap<Vec<u8>, (u64, u64)>,
    dtype: DType,
    comp: usize,
    height: usize,
    width: usize,
    width_bytes: usize,
    scenes: usize,
    dims: Dims5,
    loops: Vec<Loop>,
}

impl ImageReader for Nd2Reader {
    fn open(path: &Path) -> Result<Self> {
        let mmap = map_file(path)?;
        let buf = &mmap[..];

        // Magic: modern ND2 only; legacy JPEG2000 ND2 is out of scope.
        let head = buf
            .get(0..4)
            .ok_or_else(|| Error::Parse("ND2 file too small for a header".to_string()))?;
        let magic = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
        if magic == JP2_MAGIC {
            return Err(Error::Unsupported(
                "legacy JPEG2000 ND2 not supported".to_string(),
            ));
        }
        if magic != ND2_CHUNK_MAGIC {
            return Err(Error::Parse(format!(
                "not a modern ND2 file (magic {magic:#010x})"
            )));
        }

        let map = read_chunkmap(buf)?;

        // --- attributes (dims / dtype / component count) ---
        let attrs = parse_attributes(buf, &map)?;

        // --- experiment loops (T / Z / P) ---
        let exp_off = map
            .get(b"ImageMetadataLV!".as_slice())
            .ok_or_else(|| Error::Parse("ND2 missing ImageMetadataLV! chunk".to_string()))?
            .0;
        let exp_root = parse_clx_variant(chunk_data(buf, exp_off, None)?)?;
        let exp = exp_root
            .get_level("SLxExperiment")
            .ok_or_else(|| Error::Parse("ND2 missing SLxExperiment".to_string()))?;
        let loops = load_experiment(exp)?;

        let scenes = scene_count(&loops);
        let dims = Dims5 {
            t: axis_count(&loops, Axis::T),
            c: attrs.comp,
            z: axis_count(&loops, Axis::Z),
            y: attrs.height,
            x: attrs.width,
        };
        // Size the plane WITHOUT the decode ceiling: the ceiling guards ONE
        // materialisation, not the file's existence. A file whose plane is too
        // big to hold in RAM is still PUBLISHED — `plane()` refuses the
        // materialisation and `region()` serves windows of it — exactly as the
        // CZI reader does for a whole-slide scene. Only a size that OVERFLOWS
        // the reader's byte arithmetic refuses the file.
        dims.plane_bytes_no_ceiling(attrs.dtype)?;

        // --- best-effort metadata (never gates open) ---
        let cal_um = parse_calibration_um(buf, &map);
        let z_step_um = find_z_step(exp, 0);
        let calibration = cal_um
            .map(|c| Calibration::sized(c as f32, c as f32, z_step_um.map(|v| v as f32)))
            .unwrap_or_default();
        let channels = parse_channels(buf, &map, attrs.comp);
        let stem = file_stem(path);

        let scene_metas: Vec<SceneMeta> = (0..scenes)
            .map(|s| SceneMeta {
                dims,
                dtype: attrs.dtype,
                samples: 1,
                px_size_um: calibration.px_size_um,
                notes: calibration.notes.clone(),
                channels: channels.clone(),
                palette: None,
                name: if scenes == 1 {
                    stem.clone()
                } else {
                    format!("{stem} (P{s})")
                },
            })
            .collect();

        let meta = ImageMeta { name: stem, scenes: scene_metas };

        Ok(Nd2Reader {
            mmap,
            meta,
            map,
            dtype: attrs.dtype,
            comp: attrs.comp,
            height: attrs.height,
            width: attrs.width,
            width_bytes: attrs.width_bytes,
            scenes,
            dims,
            loops,
        })
    }

    fn meta(&self) -> &ImageMeta {
        &self.meta
    }

    fn plane(&self, scene: usize, t: usize, c: usize, z: usize) -> Result<Plane> {
        if scene >= self.scenes {
            return Err(Error::SceneOutOfRange { scene, count: self.scenes });
        }
        let d = self.dims;
        if t >= d.t || c >= d.c || z >= d.z {
            return Err(Error::OutOfRange { scene, t, c, z });
        }
        let seq = seq_index(&self.loops, scene, t, z)?;
        self.decode_plane(seq, c)
    }

    /// ND2 frames are stored uncompressed (the reader refuses every file whose
    /// `eCompression` is not the uncompressed value), with every row at a
    /// computable byte offset, so a sub-rectangle of a plane is read from the
    /// frame's rows alone — the whole plane is never decoded.
    fn supports_region(&self) -> bool {
        true
    }

    /// Read only the rows of `rect` out of the frame's pixel block.
    ///
    /// The result is byte-identical to `self.plane(scene, t, c, z)?.crop(rect)`:
    /// the same little-endian bytes are converted to native endianness once,
    /// over the rect alone. The output buffer is sized by the RECT, so the
    /// decode ceiling applies to what is materialised, never to the full
    /// plane — a file whose plane is over the ceiling is still readable here,
    /// one window at a time.
    fn region(
        &self,
        scene: usize,
        t: usize,
        c: usize,
        z: usize,
        rect: PixelRect,
    ) -> Result<Plane> {
        if scene >= self.scenes {
            return Err(Error::SceneOutOfRange { scene, count: self.scenes });
        }
        // Validate the coordinate FIRST, then the rect — the same order and the
        // same wording as every other reader (the shared helper).
        checked_region("ND2 region", &self.dims, scene, t, c, z, rect)?;

        // An empty rectangle covers no pixels: the answer is an empty plane,
        // the same one the trait's default returns.
        if rect.is_empty() {
            return empty_region_plane(self.dtype, 1, rect);
        }

        // Size the OUTPUT by the RECT, never by the plane: the ceiling
        // (`MAX_DECODED_PLANE_BYTES`) guards what is materialised, so a small
        // window of a huge plane is not refused.
        let rect_dims = Dims5 {
            t: 1,
            c: 1,
            z: 1,
            y: rect.h,
            x: rect.w,
        };
        let out_bytes = rect_dims.checked_plane_bytes(self.dtype)?;

        let seq = seq_index(&self.loops, scene, t, z)?;
        let pixels = self.frame_pixels(seq)?;
        // The frame must hold every row it declares — the SAME check
        // `decode_plane` makes. A caller that reads only regions must be able
        // to tell a short frame from a whole one.
        self.check_frame_length(pixels)?;

        let mut raw = vec![0u8; out_bytes];
        if self.comp == 1 {
            // One component per pixel: the rect's pixels are CONTIGUOUS within
            // a row (a padded row's padding sits AFTER its pixels), so a row
            // of the rect is one copy.
            self.gather_contiguous_rows(pixels, rect, &mut raw)?;
        } else {
            // Interleaved components: a pixel of component `c` sits at
            // `x*comp*itemsize + c*itemsize` inside its row, so the rect's
            // pixels are one copy each.
            self.gather_interleaved_pixels(pixels, rect, c, &mut raw)?;
        }

        // Raw pixel bytes are little-endian; `from_bytes` byteswaps to native.
        let data = PlaneBuf::from_bytes(self.dtype, Endian::Little, &raw)?;
        Plane::new(data, [rect.h, rect.w])
    }
}

impl Nd2Reader {
    /// The pixel bytes of frame `seq`: the chunk `ImageDataSeq|{seq}!`'s data
    /// after its 8-byte prefix.
    fn frame_pixels(&self, seq: usize) -> Result<&[u8]> {
        let name = format!("ImageDataSeq|{seq}!");
        let (off, _) = self.map.get(name.as_bytes()).ok_or_else(|| {
            Error::Decode(format!("ND2 missing frame chunk {name}"))
        })?;
        let data = chunk_data(&self.mmap, *off, None)?;
        // 8-byte prefix, then `height * widthBytes` pixel bytes.
        data.get(8..)
            .ok_or_else(|| Error::Decode("ND2 frame chunk shorter than its 8-byte prefix".to_string()))
    }

    /// The frame must hold every row it declares: `height * widthBytes` pixel
    /// bytes. A frame that is short is a broken file, and `plane()` and
    /// `region()` must say so the same way.
    fn check_frame_length(&self, pixels: &[u8]) -> Result<()> {
        let need = self
            .height
            .checked_mul(self.width_bytes)
            .ok_or_else(|| Error::Decode("ND2 frame size overflow".to_string()))?;
        if pixels.len() < need {
            return Err(Error::Decode(format!(
                "ND2 frame has {} pixel bytes, expected at least {need}",
                pixels.len()
            )));
        }
        Ok(())
    }

    /// Copy the rect's rows out of a frame whose pixels are CONTIGUOUS within
    /// a row (one component per pixel): a row of the rect is one copy.
    fn gather_contiguous_rows(
        &self,
        pixels: &[u8],
        rect: PixelRect,
        raw: &mut [u8],
    ) -> Result<()> {
        let bps = self.dtype.size_bytes();
        let row_bytes = rect
            .w
            .checked_mul(bps)
            .ok_or_else(|| Error::Decode("ND2 region row size overflow".to_string()))?;
        let col_offset = rect
            .x
            .checked_mul(bps)
            .ok_or_else(|| Error::Decode("ND2 region column offset overflow".to_string()))?;
        for row in 0..rect.h {
            let src = (rect.y + row)
                .checked_mul(self.width_bytes)
                .and_then(|v| v.checked_add(col_offset))
                .ok_or_else(|| Error::Decode("ND2 region offset overflow".to_string()))?;
            let end = src
                .checked_add(row_bytes)
                .ok_or_else(|| Error::Decode("ND2 region offset overflow".to_string()))?;
            let span = pixels.get(src..end).ok_or_else(|| {
                Error::Decode("ND2 region row runs past the frame".to_string())
            })?;
            let dst = row * row_bytes;
            raw[dst..dst + row_bytes].copy_from_slice(span);
        }
        Ok(())
    }

    /// Copy the rect's pixels out of a frame whose components are INTERLEAVED:
    /// a pixel of component `c` sits at `x*comp*itemsize + c*itemsize` inside
    /// its row, so the rect's pixels are one copy each.
    fn gather_interleaved_pixels(
        &self,
        pixels: &[u8],
        rect: PixelRect,
        c: usize,
        raw: &mut [u8],
    ) -> Result<()> {
        let bps = self.dtype.size_bytes();
        let row_bytes = rect
            .w
            .checked_mul(bps)
            .ok_or_else(|| Error::Decode("ND2 region row size overflow".to_string()))?;
        // Byte distance between two neighbouring pixels of a row, and the
        // offset of this component inside one pixel.
        let pixel_stride = self
            .comp
            .checked_mul(bps)
            .ok_or_else(|| Error::Decode("ND2 region pixel stride overflow".to_string()))?;
        let comp_offset = c
            .checked_mul(bps)
            .ok_or_else(|| Error::Decode("ND2 region component offset overflow".to_string()))?;
        for row in 0..rect.h {
            let row_start = (rect.y + row)
                .checked_mul(self.width_bytes)
                .ok_or_else(|| Error::Decode("ND2 region offset overflow".to_string()))?;
            for i in 0..rect.w {
                let src = row_start
                    .checked_add(
                        (rect.x + i)
                            .checked_mul(pixel_stride)
                            .ok_or_else(|| Error::Decode("ND2 region offset overflow".to_string()))?,
                    )
                    .and_then(|v| v.checked_add(comp_offset))
                    .ok_or_else(|| Error::Decode("ND2 region offset overflow".to_string()))?;
                let end = src
                    .checked_add(bps)
                    .ok_or_else(|| Error::Decode("ND2 region offset overflow".to_string()))?;
                let span = pixels.get(src..end).ok_or_else(|| {
                    Error::Decode("ND2 region pixel runs past the frame".to_string())
                })?;
                let dst = row * row_bytes + i * bps;
                raw[dst..dst + bps].copy_from_slice(span);
            }
        }
        Ok(())
    }

    /// Decode component `c` of frame `seq` into a contiguous grayscale plane.
    fn decode_plane(&self, seq: usize, c: usize) -> Result<Plane> {
        // Guard the OUTPUT plane size BEFORE anything else: the decode ceiling
        // guards what is materialised, so a plane over it is refused even when
        // the frame is short — the reason is the ceiling (the file's own
        // problem) and not the frame's.
        let plane_dims = Dims5 { t: 1, c: 1, z: 1, y: self.height, x: self.width };
        plane_dims.checked_plane_bytes(self.dtype)?;

        let pixels = self.frame_pixels(seq)?;
        self.check_frame_length(pixels)?;

        let bps = self.dtype.size_bytes();
        let need = self
            .height
            .checked_mul(self.width_bytes)
            .ok_or_else(|| Error::Decode("ND2 frame size overflow".to_string()))?;

        // Fast path: a single, unpadded component IS the contiguous plane.
        if self.comp == 1 && self.width_bytes == self.width * bps {
            let block = &pixels[..need];
            let data = PlaneBuf::from_bytes(self.dtype, Endian::Little, block)?;
            return Plane::new(data, [self.height, self.width]);
        }

        // General path: gather component `c` honouring the row byte stride.
        let out_len = self
            .width
            .checked_mul(self.height)
            .and_then(|px| px.checked_mul(bps))
            .ok_or_else(|| Error::Decode("ND2 gathered plane size overflow".to_string()))?;
        let mut gathered = vec![0u8; out_len];
        for y in 0..self.height {
            let row = y * self.width_bytes;
            let dst_row = y * self.width * bps;
            for x in 0..self.width {
                // In-bounds by the open()-time `width_bytes >= width*comp*bps` check.
                let src = row + (x * self.comp + c) * bps;
                let dst = dst_row + x * bps;
                gathered[dst..dst + bps].copy_from_slice(&pixels[src..src + bps]);
            }
        }
        let data = PlaneBuf::from_bytes(self.dtype, Endian::Little, &gathered)?;
        Plane::new(data, [self.height, self.width])
    }
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // --- CLX record builders (explicit byte layout) --------------------------

    /// UTF-16LE name bytes plus its `name_length` (code units INCLUDING the NUL).
    fn utf16_name(s: &str) -> (u8, Vec<u8>) {
        let mut units: Vec<u16> = s.encode_utf16().collect();
        units.push(0); // trailing NUL, counted in name_length
        let name_length = units.len() as u8;
        let mut bytes = Vec::with_capacity(units.len() * 2);
        for u in units {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        (name_length, bytes)
    }

    fn rec_scalar(data_type: u8, name: &str, value: &[u8]) -> Vec<u8> {
        let (nl, nb) = utf16_name(name);
        let mut r = vec![data_type, nl];
        r.extend(nb);
        r.extend_from_slice(value);
        r
    }

    fn rec_u32(name: &str, v: u32) -> Vec<u8> {
        rec_scalar(clx_type::UINT32, name, &v.to_le_bytes())
    }

    fn rec_f64(name: &str, v: f64) -> Vec<u8> {
        rec_scalar(clx_type::DOUBLE, name, &v.to_le_bytes())
    }

    fn rec_str(name: &str, s: &str) -> Vec<u8> {
        let mut value: Vec<u8> = Vec::new();
        for u in s.encode_utf16() {
            value.extend_from_slice(&u.to_le_bytes());
        }
        value.extend_from_slice(&[0, 0]); // \x00\x00 terminator
        rec_scalar(clx_type::STRING, name, &value)
    }

    fn rec_level(name: &str, children: &[Vec<u8>]) -> Vec<u8> {
        let (nl, nb) = utf16_name(name);
        let item_count = children.len();
        let child_bytes: Vec<u8> = children.iter().flatten().copied().collect();
        let length = (2 + nb.len() + 12 + child_bytes.len()) as u64;
        let mut r = vec![clx_type::LEVEL, nl];
        r.extend(nb);
        r.extend_from_slice(&(item_count as u32).to_le_bytes());
        r.extend_from_slice(&length.to_le_bytes());
        r.extend(child_bytes);
        r.extend(vec![0u8; item_count * 8]); // offset table
        r
    }

    // --- CLX parser tests ----------------------------------------------------

    #[test]
    fn clx_level_with_scalars_roundtrips() {
        let root = rec_level(
            "Root",
            &[
                rec_u32("W", 2868),
                rec_f64("D", 0.275),
                rec_str("S", "Alx546"),
            ],
        );
        let parsed = parse_clx_variant(&root).unwrap();
        let inner = parsed.get_level("Root").expect("Root level");
        assert_eq!(inner.get_u32("W"), Some(2868));
        assert_eq!(inner.get_f64("D"), Some(0.275));
        assert_eq!(inner.get_str("S"), Some("Alx546"));
        assert_eq!(inner.len(), 3);
    }

    #[test]
    fn clx_nested_level_arithmetic() {
        // A LEVEL inside a LEVEL: exercises the length/skip math one level deep.
        let root = rec_level(
            "SLxExperiment",
            &[
                rec_u32("eType", 4),
                rec_level("uLoopPars", &[rec_u32("uiCount", 25), rec_f64("dZStep", 0.275)]),
            ],
        );
        let parsed = parse_clx_variant(&root).unwrap();
        let exp = parsed.get_level("SLxExperiment").unwrap();
        assert_eq!(exp.get_i64("eType"), Some(4));
        let lp = exp.get_level("uLoopPars").unwrap();
        assert_eq!(lp.get_usize("uiCount"), Some(25));
        assert_eq!(lp.get_f64("dZStep"), Some(0.275));
    }

    #[test]
    fn clx_empty_name_items_collapse_to_list() {
        // A LEVEL whose two children have empty names becomes a List.
        let root = rec_level(
            "ppNextLevelEx",
            &[
                rec_level("", &[rec_u32("eType", 4)]),
                rec_level("", &[rec_u32("eType", 4)]),
            ],
        );
        let parsed = parse_clx_variant(&root).unwrap();
        match parsed.get("ppNextLevelEx") {
            Some(ClxValue::List(items)) => {
                assert_eq!(items.len(), 2);
                assert!(items.iter().all(|v| v.as_level().is_some()));
            }
            other => panic!("expected a List, got {other:?}"),
        }
    }

    #[test]
    fn clx_compress_roundtrips() {
        // Build a small tree, zlib-compress it, and wrap it in a COMPRESS record.
        let tree = rec_level("Root", &[rec_u32("W", 7), rec_str("S", "hi")]);
        let plain = parse_clx_variant(&tree).unwrap();

        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&tree).unwrap();
        let compressed = encoder.finish().unwrap();

        let mut input = vec![clx_type::COMPRESS, 0];
        input.extend_from_slice(&[0u8; 10]); // 10 skip bytes
        input.extend_from_slice(&compressed);

        let inflated = parse_clx_variant(&input).unwrap();
        assert_eq!(inflated, plain);
        assert_eq!(inflated.get_level("Root").unwrap().get_u32("W"), Some(7));
    }

    // --- chunkmap blob test --------------------------------------------------

    fn map_entry(blob: &mut Vec<u8>, name: &[u8], offset: u64, size: u64) {
        blob.extend_from_slice(name); // includes trailing '!'
        blob.extend_from_slice(&offset.to_le_bytes());
        blob.extend_from_slice(&size.to_le_bytes());
    }

    #[test]
    fn chunkmap_blob_parses_entries_and_stops_at_signature() {
        let mut blob = Vec::new();
        map_entry(&mut blob, b"ImageAttributesLV!", 100, 580);
        map_entry(&mut blob, b"ImageDataSeq|0!", 200, 16450856);
        // size == offset -> the -1 sentinel (stored as u64::MAX).
        map_entry(&mut blob, b"ImageMetadataLV!", 300, 300);
        blob.extend_from_slice(SIG_CHUNKMAP_END);
        // Trailing bytes after the stop signature must be ignored.
        blob.extend_from_slice(&[0xAA; 8]);

        let map = parse_chunkmap_blob(&blob).unwrap();
        assert_eq!(map.len(), 3);
        assert_eq!(map.get(b"ImageAttributesLV!".as_slice()), Some(&(100, 580)));
        assert_eq!(map.get(b"ImageDataSeq|0!".as_slice()), Some(&(200, 16450856)));
        assert_eq!(map.get(b"ImageMetadataLV!".as_slice()), Some(&(300, u64::MAX)));
    }

    // --- coordinate mapping tests --------------------------------------------

    #[test]
    fn seq_index_is_row_major_product() {
        let loops = vec![
            Loop { axis: Axis::P, count: 2 },
            Loop { axis: Axis::T, count: 3 },
            Loop { axis: Axis::Z, count: 4 },
        ];
        assert_eq!(scene_count(&loops), 2);
        assert_eq!(axis_count(&loops, Axis::T), 3);
        assert_eq!(axis_count(&loops, Axis::Z), 4);
        // ((scene*3)+t)*4 + z
        assert_eq!(seq_index(&loops, 0, 0, 0).unwrap(), 0);
        assert_eq!(seq_index(&loops, 0, 0, 1).unwrap(), 1);
        assert_eq!(seq_index(&loops, 0, 1, 0).unwrap(), 4);
        assert_eq!(seq_index(&loops, 1, 0, 0).unwrap(), 12);
        assert_eq!(seq_index(&loops, 1, 2, 3).unwrap(), 23);
    }

    #[test]
    fn single_zstack_defaults_absent_axes_to_one() {
        let loops = vec![Loop { axis: Axis::Z, count: 25 }];
        assert_eq!(scene_count(&loops), 1);
        assert_eq!(axis_count(&loops, Axis::T), 1);
        assert_eq!(axis_count(&loops, Axis::Z), 25);
        assert_eq!(seq_index(&loops, 0, 0, 12).unwrap(), 12);
    }

    // --- experiment flattening tests -----------------------------------------

    #[test]
    fn spectloop_is_skipped_and_zstack_survives() {
        // Mirror the real sample: SpectLoop(6, count via pPlanes) -> ppNextLevelEx
        // -> ZStackLoop(4, uiCount=25). Flattened result must be [Z(25)].
        let zstack = rec_level(
            "",
            &[
                rec_u32("eType", 4),
                rec_level("uLoopPars", &[rec_u32("uiCount", 25)]),
            ],
        );
        let exp = rec_level(
            "SLxExperiment",
            &[
                rec_u32("eType", 6),
                rec_level(
                    "uLoopPars",
                    &[rec_level("pPlanes", &[rec_u32("uiCount", 1)])],
                ),
                rec_level("ppNextLevelEx", &[zstack]),
            ],
        );
        let parsed = parse_clx_variant(&exp).unwrap();
        let loops = load_experiment(parsed.get_level("SLxExperiment").unwrap()).unwrap();
        assert_eq!(loops, vec![Loop { axis: Axis::Z, count: 25 }]);
    }

    #[test]
    fn custom_loop_is_unsupported() {
        let exp = rec_level(
            "SLxExperiment",
            &[
                rec_u32("eType", 7),
                rec_level("uLoopPars", &[rec_u32("uiCount", 3)]),
            ],
        );
        let parsed = parse_clx_variant(&exp).unwrap();
        let err = load_experiment(parsed.get_level("SLxExperiment").unwrap());
        assert!(matches!(err, Err(Error::Unsupported(_))));
    }

    #[test]
    fn unhandled_loop_type_errors_not_silently_dropped() {
        // ZStackLoopAccurate (10) is in-range but unhandled: it must fail loud,
        // NOT return an empty loop list (which would mis-decode dims / frames).
        let exp = rec_level(
            "SLxExperiment",
            &[
                rec_u32("eType", 10),
                rec_level("uLoopPars", &[rec_u32("uiCount", 25)]),
            ],
        );
        let parsed = parse_clx_variant(&exp).unwrap();
        assert!(matches!(
            load_experiment(parsed.get_level("SLxExperiment").unwrap()),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn oversized_loop_count_is_refused() {
        // A crafted XYPosLoop with a huge uiCount (and no Points) must be refused
        // before it can size a ~billion-element Vec<SceneMeta> (uncatchable abort).
        let exp = rec_level(
            "SLxExperiment",
            &[
                rec_u32("eType", 2),
                rec_level("uLoopPars", &[rec_u32("uiCount", u32::MAX)]),
            ],
        );
        let parsed = parse_clx_variant(&exp).unwrap();
        assert!(matches!(
            load_experiment(parsed.get_level("SLxExperiment").unwrap()),
            Err(Error::Unsupported(_))
        ));
    }

    // --- synthetic end-to-end ND2 (open + decode + out-of-range) -------------
    //
    // The CONTAINER itself is written by the one ND2 writer of this crate,
    // `tests/support/nd2.rs`; the tests that need a whole file live in
    // `tests/nd2_open.rs`. What stays here are the record builders for the
    // CLx parsing below, which are unit tests of private functions.

    fn temp_path() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "phasit_synth_{:?}_{}_{}.nd2",
            std::thread::current().id(),
            std::process::id(),
            n
        ))
    }

    #[test]
    fn legacy_jp2_magic_is_unsupported() {
        let path = temp_path();
        let mut bytes = JP2_MAGIC.to_le_bytes().to_vec();
        bytes.resize(64, 0);
        std::fs::write(&path, &bytes).unwrap();
        let r = Nd2Reader::open(&path);
        let _ = std::fs::remove_file(&path);
        assert!(matches!(r, Err(Error::Unsupported(_))));
    }
}
