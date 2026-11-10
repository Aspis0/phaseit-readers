use std::io::Read;

use ruzstd::decoding::StreamingDecoder;

use crate::error::{Error, Result};
use crate::types::DType;

/// Refuses output longer than `expected` (a decompression bomb) and output shorter than it alike.
pub(super) fn zstd_raw_decode(input: &[u8], expected: usize) -> Result<Vec<u8>> {
    let mut decoder = StreamingDecoder::new(input)
        .map_err(|e| Error::Decode(format!("CZI zstd: {e}")))?;
    let mut out = Vec::with_capacity(expected);
    // Reads at most `expected + 1` bytes: one extra byte proves the stream is longer (bomb guard).
    let cap = expected as u64 + 1;
    decoder
        .by_ref()
        .take(cap)
        .read_to_end(&mut out)
        .map_err(|e| Error::Decode(format!("CZI zstd read: {e}")))?;
    if out.len() != expected {
        return Err(Error::Decode(format!(
            "CZI zstd produced {} bytes, expected {expected}",
            out.len()
        )));
    }
    Ok(out)
}

/// Decodes a CZI Zstd1 subblock (czifile's layout, not verified against libCZI); Hi/Lo data is de-interleaved to
/// little-endian.
pub(super) fn zstd1_decode(input: &[u8], expected: usize, itemsize: usize) -> Result<Vec<u8>> {
    if input.is_empty() {
        return Err(Error::Decode("CZI Zstd1 subblock is empty".to_string()));
    }
    let header_size = input[0] as usize;
    if header_size == 0 || header_size > input.len() {
        return Err(Error::Decode(format!(
            "CZI Zstd1 header size {header_size} invalid for {} input bytes",
            input.len()
        )));
    }
    let mut hilo = false;
    if header_size >= 3 {
        let chunk_type = input[1];
        if chunk_type == 1 {
            hilo = (input[2] & 0x01) != 0;
        }
    }

    let payload = &input[header_size..];
    let raw = zstd_raw_decode(payload, expected)?;

    if hilo && itemsize == 2 {
        Ok(deinterleave_hilo16(&raw))
    } else {
        Ok(raw)
    }
}

pub(super) fn deinterleave_hilo16(raw: &[u8]) -> Vec<u8> {
    let n = raw.len() / 2;
    let (lo, hi) = raw.split_at(n);
    let mut out = vec![0u8; raw.len()];
    for i in 0..n {
        out[2 * i] = lo[i];
        out[2 * i + 1] = hi[i];
    }
    out
}

/// Swaps the B and R samples of an interleaved little-endian tile in place: CZI stores colour as BGR.
/// `item` is the per-sample byte size; a length that is not a multiple of `3 * item` is returned unchanged.
// Plain `%`: `usize::is_multiple_of` needs Rust 1.87, and the crate builds on older stable toolchains.
#[allow(clippy::manual_is_multiple_of)]
pub(super) fn bgr_to_rgb(mut le_bytes: Vec<u8>, item: usize) -> Vec<u8> {
    let pixel = 3 * item;
    if pixel == 0 || le_bytes.len() % pixel != 0 {
        return le_bytes;
    }
    for px in le_bytes.chunks_exact_mut(pixel) {
        let (b, rest) = px.split_at_mut(item);
        let r = &mut rest[item..]; // skip the G sample in the middle
        b.swap_with_slice(r);
    }
    le_bytes
}

/// Colour types are interleaved samples, not extra channels: Bgr24 is (U8, 3) and Bgr48 is (U16, 3).
/// The id space is PixelType (id 4 = Bgr48), not Compression (id 4 = JpegXR).
pub(super) fn pixel_type_to_dtype(pixel_type: i32) -> Result<(DType, u32)> {
    match pixel_type {
        0 => Ok((DType::U8, 1)),
        1 => Ok((DType::U16, 1)),
        2 => Ok((DType::F32, 1)),
        3 => Ok((DType::U8, 3)),
        4 => Ok((DType::U16, 3)),
        other => Err(Error::Unsupported(format!(
            "CZI PixelType {other} is not supported"
        ))),
    }
}
