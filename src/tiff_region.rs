// SPDX-License-Identifier: Apache-2.0
//! A TIFF page read a rectangle at a time, so a plane over the decode ceiling is still readable window by window.
//! Uncompressed pages: the rectangle's bytes are copied out of the file at the chunk grid's offsets (no decoder).
//! Compressed pages: the touched chunks are decoded, but only while a chunk is much smaller than the plane
//! ([`tiff_layout::chunks_are_cheap`]); a page whose chunk IS the plane keeps the default path.
//! [`page_service`] decides the path once, at open, and reads the uncompressed chunk table there.

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

pub(crate) type TiffDec<'a> = Decoder<Cursor<&'a [u8]>>;

/// How a page's windows are read, decided once per page at open from its own tags.
#[derive(Debug, Clone)]
pub(crate) enum Service {
    /// Uncompressed: a window's bytes are copied out of the file at the chunk table's offsets.
    Raw {
        layout: PageLayout,
        table: ChunkTable,
    },
    Chunks {
        layout: PageLayout,
    },
}

/// The page's window service, or `None` for the default path; the uncompressed chunk table is read here.
pub(crate) fn page_service(
    dec: &mut Decoder<Cursor<&[u8]>>,
    mmap: &[u8],
    facts: &PageFacts,
) -> Option<Service> {
    let layout = tiff_layout::read_layout(dec).ok()?;
    if layout.compression == tiff_layout::COMPRESSION_NONE {
        // A predictor changes the stored bytes, so the file's bytes are not the page's pixels.
        if layout.predictor != tiff_layout::PREDICTOR_NONE {
            return None;
        }
        // Indexed pages: their indices are the pixels, so the file's bytes are used as they are.
        if facts.photometric == tiff_tags::PHOTOMETRIC_PALETTE {
            let table = tiff_palette::window_strips(dec, facts, &layout)?;
            return Some(Service::Raw { layout, table });
        }
        // The crate reshapes some pages (WhiteIsZero, YCbCr, CMYK) and drops samples: the file's bytes then differ.
        if !crate_copies_the_page(dec, facts) || !page_bytes_are_the_pixels(facts) {
            return None;
        }
        let table = ChunkTable::of(dec, &layout.chunking).ok()?;
        let grid = ChunkGrid::of(&layout, facts);
        if !chunks_hold_their_pixels(&grid, &table, mmap.len()) {
            return None;
        }
        Some(Service::Raw { layout, table })
    } else {
        // Compressed: the crate decodes the chunks, so its colour model must match the samples.
        if !crate_copies_the_page(dec, facts) {
            return None;
        }
        let chunk = tiff_layout::chunk_bytes(&layout, facts)?;
        let page = tiff_layout::page_bytes(facts)?;
        if !tiff_layout::chunks_are_cheap(chunk, page) {
            return None;
        }
        Some(Service::Chunks { layout })
    }
}

/// True when the `tiff` crate decodes the page into its own samples unchanged (YCbCr is converted; extra samples drop).
/// Otherwise the crate's plane and the page's bytes differ, so a window read would not match it.
fn crate_copies_the_page(dec: &mut Decoder<Cursor<&[u8]>>, facts: &PageFacts) -> bool {
    match dec.colortype() {
        // YCbCr needs chroma handling this reader does not model.
        Ok(ColorType::YCbCr(_)) => false,
        Ok(color) => color.num_samples() as usize == facts.samples,
        Err(_) => false,
    }
}

/// False for WhiteIsZero, CMYK and YCbCr pages: the crate inverts or converts those, so the file's bytes differ.
fn page_bytes_are_the_pixels(facts: &PageFacts) -> bool {
    matches!(
        facts.photometric,
        tiff_tags::PHOTOMETRIC_BLACK_IS_ZERO | tiff_tags::PHOTOMETRIC_RGB
    )
}

/// Every chunk lies in the file and holds the rows the grid gives it.
/// A short or out-of-file chunk would make the copy fail halfway, so such a page keeps the default path.
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

/// Copies the window `rect` of an UNCOMPRESSED page out of the mapped file, in the FILE's byte order.
/// Only the chunks the rectangle touches are read.
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
            // A chunk's row is its full width, even where a tile hangs over the page's edge.
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

/// Decodes the chunks that hold `rect` and copies the window into `out`, in the host's byte order.
/// Each chunk is decoded as the whole-page read would decode it.
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
            // The crate clips a tile's decoded row to the page's width, not the tile's.
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

    fn service_of(bytes: &[u8]) -> Option<Service> {
        let mut dec = Decoder::new(Cursor::new(bytes)).expect("the decoder");
        dec.seek_to_image(0).expect("the first page");
        let facts = crate::tiff_page::read_page_facts(&mut dec).expect("the page's facts");
        page_service(&mut dec, bytes, &facts)
    }

    /// The service decides what a window read costs, so it is pinned here.
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
