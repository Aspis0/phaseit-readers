// SPDX-License-Identifier: Apache-2.0
//! A TIFF page read a rectangle at a time.
//!
//! `plane()` decodes a whole page, and the trait's default `region()` decodes
//! the whole page and crops it, so a page whose plane is over the decode
//! ceiling cannot be read at all. This module reads a rectangle out of a page
//! WITHOUT decoding the whole plane, in the two ways a page can be read that
//! way:
//!
//! * **Uncompressed** (`Compression` = 1): the page's chunks hold its pixels
//!   as they are, so the rectangle's bytes are COPIED out of the file at the
//!   offsets the chunk grid gives. No decoder, no chunk buffer.
//! * **Compressed**: the chunks the rectangle touches are decoded with the
//!   `tiff` crate — the same expansion its whole-page read uses — and the
//!   rectangle is copied out of them. This is only worth it while a chunk is
//!   much smaller than the plane (see [`tiff_layout::chunks_are_cheap`]); a
//!   page whose chunk IS the plane keeps the default path, because decoding
//!   the plane once per window would cost more than decoding it once.
//!
//! The decode ceiling guards ONE materialisation, never a plane's size: the
//! uncompressed path materialises the RECTANGLE's bytes, and the compressed
//! path materialises one CHUNK — which the cost rule keeps under 64 MiB. So
//! a page whose plane is over the ceiling is still readable here, one window
//! at a time, as long as its window path can serve it.
//!
//! WHICH path a page takes is decided ONCE, at open, by [`page_service`], so
//! the reader never has to work it out twice — and the uncompressed path's
//! chunk table is read there too, so a window does not re-read the file's
//! offsets.

use std::io::Cursor;

use tiff::decoder::Decoder;
use tiff::ColorType;

use crate::error::{Error, Result};
use crate::rows::{self, RowPlane, RowRange};
use crate::tiff_layout::{self, ChunkGrid, ChunkTable, PageLayout};
use crate::tiff_page::PageFacts;
use crate::tiff_palette;
use crate::tiff_tags;
use crate::types::PixelRect;

/// A decoder over the mapped bytes of a TIFF file.
pub(crate) type TiffDec<'a> = Decoder<Cursor<&'a [u8]>>;

/// How a page's windows are read, and what the read needs of the page.
///
/// Decided ONCE per page, at open, from the page's own tags — so `open()`
/// reads the file's IFD chain once, not twice.
#[derive(Debug, Clone)]
pub(crate) enum Service {
    /// The page is uncompressed: its chunks hold the pixels as they are, so a
    /// window's bytes are COPIED out of the file at the offsets the chunk
    /// table gives. No decoder, no chunk buffer.
    Raw {
        /// How the page is cut into chunks.
        layout: PageLayout,
        /// Where the page's chunks are in the file: read once, at open.
        table: ChunkTable,
    },
    /// The page is compressed: the chunks a window touches are decoded.
    Chunks {
        /// How the page is cut into chunks.
        layout: PageLayout,
    },
}

/// How the page the decoder is on is served by a window read, or `None` when
/// the page keeps the default path (decode the plane, then crop it).
///
/// Decided ONCE per page, at open, from the page's own tags — so `open()`
/// reads the file's IFD chain once, not twice. An uncompressed page's chunk
/// table is read HERE, so a window of it does not re-read the file's offsets.
pub(crate) fn page_service(
    dec: &mut Decoder<Cursor<&[u8]>>,
    mmap: &[u8],
    facts: &PageFacts,
) -> Option<Service> {
    let layout = tiff_layout::read_layout(dec).ok()?;
    if layout.compression == tiff_layout::COMPRESSION_NONE {
        // A predictor changes what the chunk holds (a horizontal difference,
        // say), so the file's bytes are not the page's pixels: such a page
        // keeps the default path, which decodes them.
        if layout.predictor != tiff_layout::PREDICTOR_NONE {
            return None;
        }
        // An indexed page is read from its strips, and its indices ARE the
        // pixels: the file's bytes as they are.
        if facts.photometric == tiff_tags::PHOTOMETRIC_PALETTE {
            let table = tiff_palette::window_strips(dec, facts, &layout)?;
            return Some(Service::Raw { layout, table });
        }
        // The crate reshapes some pages (WhiteIsZero is inverted, YCbCr and
        // CMYK are converted) and drops extra samples: then the crate's
        // plane and the file's bytes are different pictures, and a window
        // read that copies the file's bytes would not match it.
        if !crate_copies_the_page(dec, facts) || !page_bytes_are_the_pixels(facts) {
            return None;
        }
        // The chunks must hold the pixels the grid says they do.
        let table = ChunkTable::of(dec, &layout.chunking).ok()?;
        let grid = ChunkGrid::of(&layout, facts);
        if !chunks_hold_their_pixels(&grid, &table, mmap.len()) {
            return None;
        }
        Some(Service::Raw { layout, table })
    } else {
        // Compressed: the crate decodes the chunks, so the crate's own
        // colour model must match the page's samples.
        if !crate_copies_the_page(dec, facts) {
            return None;
        }
        // The cost rule: a chunk read per window only pays off while a chunk
        // is much smaller than the plane.
        let chunk = tiff_layout::chunk_bytes(&layout, facts)?;
        let page = tiff_layout::page_bytes(facts)?;
        if !tiff_layout::chunks_are_cheap(chunk, page) {
            return None;
        }
        Some(Service::Chunks { layout })
    }
}

/// True when the `tiff` crate decodes the page into the page's own samples,
/// unchanged.
///
/// WHY: the crate compacts a page it does not model — YCbCr is converted,
/// extra samples are dropped — and then the crate's plane and the page's own
/// samples are different pictures. A window read must agree with the plane
/// the crate decodes, so such a page is not served here.
fn crate_copies_the_page(dec: &mut Decoder<Cursor<&[u8]>>, facts: &PageFacts) -> bool {
    match dec.colortype() {
        // A YCbCr page needs the crate's chroma handling, which this reader
        // does not model; no published scene holds one.
        Ok(ColorType::YCbCr(_)) => false,
        Ok(color) => color.num_samples() as usize == facts.samples,
        Err(_) => false,
    }
}

/// True when the page's bytes ARE the crate's pixels: the crate copies them
/// as they are.
///
/// WHY: the crate INVERTS a WhiteIsZero page and CONVERTS a CMYK or YCbCr
/// one, so the file's bytes and the crate's pixels differ there. A window
/// read that copies the file's bytes would then not match the plane.
fn page_bytes_are_the_pixels(facts: &PageFacts) -> bool {
    matches!(
        facts.photometric,
        tiff_tags::PHOTOMETRIC_BLACK_IS_ZERO | tiff_tags::PHOTOMETRIC_RGB
    )
}

/// Whether the page's chunks hold the pixels the grid says they do: every
/// chunk is in the file, and long enough for the rows it holds.
///
/// WHY it matters: the uncompressed path copies a window's bytes straight out
/// of the file, at the offsets the grid gives. A chunk that is short or
/// outside the file would make that copy fail halfway, so such a page keeps
/// the default path.
fn chunks_hold_their_pixels(grid: &ChunkGrid, table: &ChunkTable, file_len: usize) -> bool {
    let count = grid.chunk_count();
    if table.offsets.len() != count || table.bytes.len() != count {
        return false;
    }
    let chunk_row_bytes = grid.chunk_row_bytes();
    for index in 0..count {
        let Ok(offset) = usize::try_from(table.offsets[index]) else {
            return false;
        };
        let Ok(bytes) = usize::try_from(table.bytes[index]) else {
            return false;
        };
        let Some(end) = offset.checked_add(bytes) else {
            return false;
        };
        if end > file_len {
            return false;
        }
        let (_, rows) = grid.chunk_rows(index);
        let Some(needed) = rows.checked_mul(chunk_row_bytes) else {
            return false;
        };
        if bytes < needed {
            return false;
        }
    }
    true
}

/// Copy the window `rect` of an UNCOMPRESSED page out of the mapped file.
///
/// The bytes come out in the FILE's byte order; the caller converts them to
/// native order (see [`crate::types::PlaneBuf::from_bytes`]).
///
/// ONLY the chunks the rectangle touches are read: a window of a page with
/// many chunks touches a few of them, not all.
pub(crate) fn read_raw_window(
    mmap: &[u8],
    layout: &PageLayout,
    table: &ChunkTable,
    facts: &PageFacts,
    rect: PixelRect,
    out: &mut [u8],
) -> Result<()> {
    let grid = ChunkGrid::of(layout, facts);
    let mut ranges = Vec::new();
    let mut runs = Vec::new();
    for index in grid.chunks_of(&rect) {
        let Some(offset) = table.offsets.get(index) else {
            return Err(Error::Decode(format!(
                "TIFF page states no offset for chunk {index}"
            )));
        };
        let Some(bytes) = table.bytes.get(index) else {
            return Err(Error::Decode(format!(
                "TIFF page states no length for chunk {index}"
            )));
        };
        let (chunk_x, chunk_y, _, chunk_h) = grid.chunk_rect(index);
        let range = RowRange {
            offset: *offset,
            bytes: *bytes,
            first_row: chunk_y,
            rows: chunk_h,
            // A chunk's row is the FULL width of the chunk: a tile stores its
            // full width, even where the tile hangs over the page's edge.
            row_bytes: grid.chunk_row_bytes(),
            first_col: chunk_x,
        };
        runs.push(rows::file_run_bytes(mmap, index, &range, "TIFF page")?);
        ranges.push(range);
    }
    let plane = RowPlane {
        pixel_bytes: grid.pixel_bytes(),
    };
    rows::gather_rows(&runs, &ranges, plane, rect, out, "TIFF page")
}

/// Decode the chunks of the page the decoder is on that hold `rect`, and copy
/// the rectangle into `out`.
///
/// `out` holds the rectangle's pixels in the host's byte order, row by row:
/// `rect.h` rows of `rect.w * samples * itemsize` bytes. The crate decodes a
/// chunk into the same pixels its whole-page read would put there.
pub(crate) fn decode_chunks(
    dec: &mut Decoder<Cursor<&[u8]>>,
    layout: &PageLayout,
    facts: &PageFacts,
    rect: PixelRect,
    out: &mut [u8],
) -> Result<()> {
    let grid = ChunkGrid::of(layout, facts);
    let pixel_bytes = grid.pixel_bytes();
    let plane = RowPlane { pixel_bytes };
    // Each chunk is decoded on its own and copied into the window: the crate
    // hands the chunk's bytes out one chunk at a time.
    for index in grid.chunks_of(&rect) {
        let mut chunk = dec
            .read_chunk(index as u32)
            .map_err(|e| Error::Decode(format!("decode chunk {index}: {e}")))?;
        let buffer = chunk.as_buffer(0);
        let bytes = buffer.as_bytes();
        let (chunk_x, chunk_y, chunk_w, chunk_h) = grid.chunk_rect(index);
        let range = RowRange {
            offset: 0,
            bytes: bytes.len() as u64,
            first_row: chunk_y,
            rows: chunk_h,
            // The crate compacts a chunk to the width the PAGE needs, so a
            // tile's decoded row is the clipped width, not the tile's.
            row_bytes: chunk_w * pixel_bytes,
            first_col: chunk_x,
        };
        rows::gather_rows(&[bytes], &[range], plane, rect, out, "TIFF page")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use tiff::encoder::colortype;
    use tiff::encoder::{Compression, DeflateLevel, TiffEncoder};

    /// A striped page of 16x16 grey pixels, `rows_per_strip` rows per strip.
    fn striped(compression: Compression, rows_per_strip: u32) -> Vec<u8> {
        let (width, height) = (16u32, 16u32);
        let data: Vec<u8> = (0..width * height).map(|index| (index % 251) as u8).collect();
        let mut bytes = Vec::new();
        {
            let mut encoder = TiffEncoder::new(Cursor::new(&mut bytes))
                .expect("the TIFF encoder")
                .with_compression(compression);
            let mut image = encoder
                .new_image::<colortype::Gray8>(width, height)
                .expect("the page");
            image
                .rows_per_strip(rows_per_strip)
                .expect("the rows per strip");
            image.write_data(&data).expect("the page data");
        }
        bytes
    }

    /// The service the first page of a TIFF file's bytes is given.
    fn service_of(bytes: &[u8]) -> Option<Service> {
        let mut dec = Decoder::new(Cursor::new(bytes)).expect("the decoder");
        dec.seek_to_image(0).expect("the first page");
        let facts = crate::tiff_page::read_page_facts(&mut dec).expect("the page's facts");
        page_service(&mut dec, bytes, &facts)
    }

    /// The service a page is given decides what a window read COSTS, so it is
    /// pinned here: an uncompressed page's bytes are copied out of the file, a
    /// compressed page's chunks are decoded, and a compressed page whose chunk
    /// IS the plane keeps the default path.
    #[test]
    fn the_service_a_page_is_given_is_pinned() {
        assert!(
            matches!(
                service_of(&striped(Compression::Uncompressed, 4)),
                Some(Service::Raw { .. })
            ),
            "an uncompressed page is served by copying its bytes out of the file"
        );
        assert!(
            matches!(
                service_of(&striped(
                    Compression::Deflate(DeflateLevel::default()),
                    1
                )),
                Some(Service::Chunks { .. })
            ),
            "a compressed page whose chunks are a small part of the plane is served chunk by chunk"
        );
        assert!(
            service_of(&striped(Compression::Deflate(DeflateLevel::default()), 16)).is_none(),
            "a compressed page whose chunk IS the plane keeps the default path"
        );
    }
}
