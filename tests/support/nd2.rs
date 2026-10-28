// SPDX-License-Identifier: Apache-2.0
//! Writes the small ND2 containers the region tests read back.
//!
//! The container is written by hand (the reader's module docs spell the
//! framing out): a sequence of chunks (`magic` + `name_len` + `data_len` +
//! name + data), the chunkmap blob, and the trailing locator. The metadata
//! chunks hold `CLxLiteVariant` records, whose byte layout the reader's own
//! docs give.

use std::path::Path;

/// Modern ND2 chunk magic (`0x0ABECEDA`, little-endian).
const ND2_CHUNK_MAGIC: u32 = 0x0ABE_CEDA;
/// Signature at the end of the chunkmap blob.
const SIG_CHUNKMAP_END: &[u8; 32] = b"ND2 CHUNK MAP SIGNATURE 0000001!";
/// Name of the chunk holding the chunkmap blob.
const SIG_FILEMAP: &[u8; 32] = b"ND2 FILEMAP SIGNATURE NAME 0001!";

/// `CLxLiteVariant` record type ids the writer uses.
mod clx_type {
    pub const UINT32: u8 = 3;
    pub const INT64: u8 = 4;
    pub const LEVEL: u8 = 11;
}

/// One frame of a synthetic ND2.
pub struct Nd2Frame {
    /// The frame's pixel bytes, WITHOUT the 8-byte frame prefix: the
    /// interleaved `(Y, X, C)` block of `height * width_bytes` bytes.
    pub pixels: Vec<u8>,
}

/// The geometry of a synthetic ND2 file.
pub struct Nd2File {
    /// The frame's width in pixels.
    pub width: usize,
    /// The frame's height in pixels.
    pub height: usize,
    /// The number of components (channels) per pixel.
    pub comp: usize,
    /// The row stride in BYTES; may exceed `width * comp * itemsize` (padding).
    pub width_bytes: usize,
    /// Bytes per sample: 1 (U8), 2 (U16) or 4 (F32).
    pub itemsize: usize,
    /// The number of Z planes (the experiment's ZStackLoop count).
    pub z: usize,
    /// The `eCompression` the file states, or `None` for a file that states
    /// NO compression value at all. The reader decodes only the uncompressed
    /// value (`2`), so a test writes another value to see it refused.
    pub e_compression: Option<i64>,
    /// The frames, one per Z plane.
    pub frames: Vec<Nd2Frame>,
}

impl Nd2File {
    /// A file of `z` frames of `width` x `height` U16 pixels, one component
    /// per pixel, whose samples follow the pattern
    /// `sample(f, y, x) = (f * 1000 + y * 10 + x) as u16`.
    pub fn gray16(width: usize, height: usize, z: usize) -> Nd2File {
        let mut frames = Vec::with_capacity(z);
        for f in 0..z {
            let mut pixels = Vec::with_capacity(width * height * 2);
            for row in 0..height {
                for col in 0..width {
                    pixels.extend_from_slice(&((f * 1000 + row * 10 + col) as u16).to_le_bytes());
                }
            }
            frames.push(Nd2Frame { pixels });
        }
        Nd2File {
            width,
            height,
            comp: 1,
            width_bytes: width * 2,
            itemsize: 2,
            z,
            e_compression: Some(2),
            frames,
        }
    }
}

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

/// One CLX scalar record.
fn rec_scalar(data_type: u8, name: &str, value: &[u8]) -> Vec<u8> {
    let (nl, nb) = utf16_name(name);
    let mut r = vec![data_type, nl];
    r.extend(nb);
    r.extend_from_slice(value);
    r
}

/// One CLX `u32` record.
fn rec_u32(name: &str, v: u32) -> Vec<u8> {
    rec_scalar(clx_type::UINT32, name, &v.to_le_bytes())
}

/// One CLX `i64` record.
fn rec_i64(name: &str, v: i64) -> Vec<u8> {
    rec_scalar(clx_type::INT64, name, &v.to_le_bytes())
}

/// One CLX LEVEL record: a header, its children, then the offset table.
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

/// Append a chunk to `file` and return the offset of its header.
fn push_chunk(file: &mut Vec<u8>, name: &[u8], data: &[u8]) -> u64 {
    let offset = file.len() as u64;
    file.extend_from_slice(&ND2_CHUNK_MAGIC.to_le_bytes());
    file.extend_from_slice(&(name.len() as u32).to_le_bytes());
    file.extend_from_slice(&(data.len() as u64).to_le_bytes());
    file.extend_from_slice(name);
    file.extend_from_slice(data);
    offset
}

/// One chunkmap entry: the name (including its `!`), the offset and the size.
fn map_entry(blob: &mut Vec<u8>, name: &[u8], offset: u64, size: u64) {
    blob.extend_from_slice(name);
    blob.extend_from_slice(&offset.to_le_bytes());
    blob.extend_from_slice(&size.to_le_bytes());
}

/// Write an ND2 holding `file`'s frames.
pub fn write_nd2(path: &Path, file: &Nd2File) {
    let mut attr_children = vec![
        rec_u32("uiWidth", file.width as u32),
        rec_u32("uiWidthBytes", file.width_bytes as u32),
        rec_u32("uiHeight", file.height as u32),
        rec_u32("uiComp", file.comp as u32),
        rec_u32("uiBpcInMemory", (file.itemsize * 8) as u32),
        rec_u32("uiBpcSignificant", (file.itemsize * 8) as u32),
        rec_u32("uiSequenceCount", file.frames.len() as u32),
    ];
    // A file that states NO compression value at all simply omits the record.
    if let Some(e) = file.e_compression {
        attr_children.push(rec_i64("eCompression", e));
    }
    let attrs = rec_level("SLxImageAttributes", &attr_children);
    let exp = rec_level(
        "SLxExperiment",
        &[
            rec_u32("eType", 4),
            rec_level("uLoopPars", &[rec_u32("uiCount", file.z as u32)]),
        ],
    );

    let mut out: Vec<u8> = Vec::new();
    let mut sig = b"Ver3.0".to_vec();
    sig.resize(64, 0);
    push_chunk(&mut out, b"ND2 FILE SIGNATURE CHUNK NAME01!", &sig);
    let attr_off = push_chunk(&mut out, b"ImageAttributesLV!", &attrs);
    let exp_off = push_chunk(&mut out, b"ImageMetadataLV!", &exp);

    let mut blob: Vec<u8> = Vec::new();
    map_entry(&mut blob, b"ImageAttributesLV!", attr_off, attrs.len() as u64);
    map_entry(&mut blob, b"ImageMetadataLV!", exp_off, exp.len() as u64);
    for (seq, frame) in file.frames.iter().enumerate() {
        // A frame's data is an 8-byte prefix, then its pixel bytes.
        let mut chunk = vec![0u8; 8];
        chunk.extend_from_slice(&frame.pixels);
        let name = format!("ImageDataSeq|{seq}!");
        let off = push_chunk(&mut out, name.as_bytes(), &chunk);
        map_entry(&mut blob, name.as_bytes(), off, chunk.len() as u64);
    }
    blob.extend_from_slice(SIG_CHUNKMAP_END);
    let map_off = push_chunk(&mut out, SIG_FILEMAP, &blob);

    out.extend_from_slice(SIG_CHUNKMAP_END);
    out.extend_from_slice(&map_off.to_le_bytes());

    std::fs::write(path, &out).expect("write the synthetic ND2");
}
