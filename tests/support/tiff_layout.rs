// SPDX-License-Identifier: Apache-2.0
//! A TIFF of one page whose pixels vary from pixel to pixel, in a layout the
//! `tiff` crate's own encoder cannot write.
//!
//! WHY: a region read is checked against a whole-plane read, and a file whose
//! rows all carry the same value cannot tell a right window from a wrong one.
//! The encoder writes the striped layouts only (see [`super::tiff_striped`]);
//! it cannot write TILED or PLANAR pages at all, so those are written here.

use std::io::Write;
use std::path::Path;

use super::tiff_page::{Layout, Page, Pattern, SampleType};

/// How many entries the palette of an indexed page holds: 256 indices, each
/// with a red, a green and a blue component.
const PALETTE_COMPONENTS: usize = 3;
/// How many indices the palette of an indexed page holds.
const PALETTE_ENTRIES: usize = 256;

/// The chunks of a page: what the file holds, and where.
enum ChunkPlan {
    /// Every chunk holds its own bytes, one after the other.
    Own(Vec<Vec<u8>>),
    /// Every chunk points at the SAME block of bytes: the page states more
    /// chunks than the file holds pixels for (a page whose plane is over the
    /// decode ceiling).
    Shared {
        /// How many chunks the page is cut into.
        count: u32,
        /// The block of bytes every chunk points at.
        block: Vec<u8>,
    },
}

impl ChunkPlan {
    /// How many chunks the page has.
    fn count(&self) -> u32 {
        match self {
            ChunkPlan::Own(chunks) => chunks.len() as u32,
            ChunkPlan::Shared { count, .. } => *count,
        }
    }
}

/// Write a TIFF of one page, whose pixels vary from pixel to pixel.
///
/// The page is uncompressed, unless its layout says its chunks are
/// compressed. A tiled page holds every sample of a pixel together (chunky);
/// a planar page holds each sample in its own plane, the way the TIFF
/// specification lays a planar page out.
pub fn write_page(path: &Path, page: &Page) {
    let plan = match page.layout {
        Layout::Strips { rows_per_strip } => {
            ChunkPlan::Own(strip_chunks(page, rows_per_strip, false))
        }
        Layout::PlanarStrips { rows_per_strip } => {
            ChunkPlan::Own(strip_chunks(page, rows_per_strip, true))
        }
        Layout::Tiles { width, length } => ChunkPlan::Own(tile_chunks(page, width, length)),
        Layout::CompressedTiles { width, length } => {
            ChunkPlan::Own(compressed_tile_chunks(page, width, length))
        }
        Layout::SharedTiles {
            tile_width,
            tile_length,
        } => ChunkPlan::Shared {
            count: tile_count(page, tile_width, tile_length),
            block: tile_block(page, tile_width, tile_length),
        },
        Layout::IndexedStrips { rows_per_strip } => {
            ChunkPlan::Own(strip_chunks(page, rows_per_strip, false))
        }
        Layout::IndexedSplitStrip { first_bytes } => {
            ChunkPlan::Own(split_strip_chunks(page, first_bytes))
        }
        Layout::IndexedShiftedStrips { rows_per_strip } => {
            ChunkPlan::Own(shifted_strip_chunks(page, rows_per_strip))
        }
    };
    let chunk_count = plan.count();

    // The IFD's size is known before the entries are: the seven tags every
    // page states, the two chunk tables, the layout's own tag and the sample
    // format.
    let layout_tags = match page.layout {
        Layout::Tiles { .. } | Layout::CompressedTiles { .. } | Layout::SharedTiles { .. } => 2,
        _ => 1,
    };
    // An indexed page states its palette in one more tag, whose array is
    // written with the other arrays.
    let indexed = matches!(
        page.layout,
        Layout::IndexedStrips { .. }
            | Layout::IndexedSplitStrip { .. }
            | Layout::IndexedShiftedStrips { .. }
    );
    let entry_count = 7 + 2 + layout_tags + 1 + usize::from(indexed);
    let ifd_offset = 8usize;
    let arrays_offset = ifd_offset + 2 + entry_count * 12 + 4;
    let offsets_at = arrays_offset;
    let counts_at = arrays_offset + chunk_count as usize * 4;
    let palette_at = counts_at + chunk_count as usize * 4;
    let data_at = if indexed {
        palette_at + PALETTE_COMPONENTS * PALETTE_ENTRIES * 2
    } else {
        palette_at
    };

    // The chunk table and the chunk data.
    let mut offsets: Vec<u8> = Vec::with_capacity(chunk_count as usize * 4);
    let mut counts: Vec<u8> = Vec::with_capacity(chunk_count as usize * 4);
    let mut data: Vec<u8> = Vec::new();
    match &plan {
        ChunkPlan::Own(chunks) => {
            for chunk in chunks {
                offsets.extend_from_slice(&(data_at as u32 + data.len() as u32).to_le_bytes());
                counts.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
                data.extend_from_slice(chunk);
            }
        }
        ChunkPlan::Shared { count, block } => {
            // Every chunk points at the same block: the file holds ONE tile's
            // pixels, and the page states a plane of many more.
            for _ in 0..*count {
                offsets.extend_from_slice(&(data_at as u32).to_le_bytes());
                counts.extend_from_slice(&(block.len() as u32).to_le_bytes());
            }
            data.extend_from_slice(block);
        }
    }

    let planar = matches!(page.layout, Layout::PlanarStrips { .. });
    let photometric = if indexed {
        3 // palette
    } else if page.samples == 3 {
        2 // RGB
    } else {
        1 // BlackIsZero
    };
    let mut entries: Vec<(u16, u16, u32, u32)> = vec![
        (256, 4, 1, page.width),                         // ImageWidth
        (257, 4, 1, page.height),                        // ImageLength
        (258, 3, 1, u32::from(page.sample_type.bits())), // BitsPerSample
        (259, 3, 1, compression(page.layout)),           // Compression
        (262, 3, 1, photometric),                        // PhotometricInterpretation
        (277, 3, 1, u32::from(page.samples)),            // SamplesPerPixel
        (284, 3, 1, if planar { 2 } else { 1 }),         // PlanarConfiguration
    ];
    match page.layout {
        Layout::Tiles { width, length }
        | Layout::CompressedTiles { width, length }
        | Layout::SharedTiles {
            tile_width: width,
            tile_length: length,
        } => {
            entries.push((322, 4, 1, width)); // TileWidth
            entries.push((323, 4, 1, length)); // TileLength
            entries.push((324, 4, chunk_count, offsets_at as u32)); // TileOffsets
            entries.push((325, 4, chunk_count, counts_at as u32)); // TileByteCounts
        }
        _ => {
            entries.push((273, 4, chunk_count, offsets_at as u32)); // StripOffsets
            entries.push((278, 4, 1, rows_per_strip(page))); // RowsPerStrip
            entries.push((279, 4, chunk_count, counts_at as u32)); // StripByteCounts
        }
    }
    entries.push((339, 3, 1, u32::from(page.sample_type.sample_format()))); // SampleFormat
    if indexed {
        entries.push((
            320,
            3,
            (PALETTE_COMPONENTS * PALETTE_ENTRIES) as u32,
            palette_at as u32,
        )); // ColorMap
    }
    entries.sort_by_key(|entry| entry.0);
    assert_eq!(
        entries.len(),
        entry_count,
        "the IFD holds the entries it says"
    );

    let mut out: Vec<u8> = Vec::with_capacity(data_at + data.len());
    out.extend_from_slice(b"II");
    out.extend_from_slice(&42u16.to_le_bytes());
    out.extend_from_slice(&(ifd_offset as u32).to_le_bytes());
    out.extend_from_slice(&(entry_count as u16).to_le_bytes());
    for (tag, kind, count, value) in &entries {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        // A SHORT count of one fits in the entry's value field; anything else
        // is an offset to the array written after the IFD.
        let mut field = [0u8; 4];
        if *kind == 3 && *count == 1 {
            field[..2].copy_from_slice(&(*value as u16).to_le_bytes());
        } else {
            field.copy_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&field);
    }
    out.extend_from_slice(&0u32.to_le_bytes()); // no next IFD
    assert_eq!(out.len(), arrays_offset, "the arrays follow the IFD");
    out.extend_from_slice(&offsets);
    out.extend_from_slice(&counts);
    if indexed {
        out.extend_from_slice(&palette());
    }
    assert_eq!(out.len(), data_at, "the pixel data follows the arrays");
    out.extend_from_slice(&data);
    std::fs::write(path, out).expect("write the TIFF");
}

/// The palette of an indexed page: 256 indices, each with a red, a green and
/// a blue component, the way TIFF states a `ColorMap`.
fn palette() -> Vec<u8> {
    let mut out = Vec::with_capacity(PALETTE_COMPONENTS * PALETTE_ENTRIES * 2);
    for component in 0..PALETTE_COMPONENTS {
        for index in 0..PALETTE_ENTRIES {
            let value: u16 = match component {
                0 => (index as u16) * 257,
                1 => u16::MAX - (index as u16) * 257,
                _ => (index as u16) * 128,
            };
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    out
}

/// The `Compression` tag a layout states: 1 = the chunks hold the pixels as
/// they are, 8 = DEFLATE.
fn compression(layout: Layout) -> u32 {
    match layout {
        Layout::CompressedTiles { .. } => 8,
        _ => 1,
    }
}

/// The rows each strip of a striped page holds.
fn rows_per_strip(page: &Page) -> u32 {
    match page.layout {
        Layout::Strips { rows_per_strip } | Layout::PlanarStrips { rows_per_strip } => {
            rows_per_strip
        }
        Layout::IndexedStrips { rows_per_strip } => rows_per_strip,
        // The shifted strips hold one row more than the tag says, and the tag
        // says the rows the GRID would cut the page into.
        Layout::IndexedShiftedStrips { rows_per_strip } => rows_per_strip,
        // The split strip does not end on a row boundary: the file states the
        // rows a strip WOULD hold, so the crate expects two strips of the page.
        Layout::IndexedSplitStrip { .. } => page.height.div_ceil(2),
        Layout::Tiles { .. }
        | Layout::CompressedTiles { .. }
        | Layout::SharedTiles { .. } => 0,
    }
}

/// The strips of a page: the rows of the page, `rows_per_strip` at a time.
/// A planar page holds one strip per sample per group of rows.
fn strip_chunks(page: &Page, rows_per_strip: u32, planar: bool) -> Vec<Vec<u8>> {
    let strips = page.height.div_ceil(rows_per_strip);
    let mut chunks = Vec::new();
    let samples = if planar { page.samples } else { 1 };
    for sample in 0..samples {
        for strip in 0..strips {
            let mut bytes = Vec::new();
            let first_row = strip * rows_per_strip;
            let last_row = (first_row + rows_per_strip).min(page.height);
            for y in first_row..last_row {
                for x in 0..page.width {
                    if planar {
                        push_sample(&mut bytes, page.sample_type, x, y, sample);
                    } else {
                        for s in 0..page.samples {
                            push_sample(&mut bytes, page.sample_type, x, y, s);
                        }
                    }
                }
            }
            chunks.push(bytes);
        }
    }
    chunks
}

/// The tiles of a page: `width` by `length` pixels each, left to right and
/// then top to bottom. A tile is always a full tile: the pixels past the
/// page's edge are padding.
fn tile_chunks(page: &Page, width: u32, length: u32) -> Vec<Vec<u8>> {
    let across = page.width.div_ceil(width);
    let down = page.height.div_ceil(length);
    let padding = page.sample_type.bits() as usize / 8 * page.samples as usize;
    let mut chunks = Vec::new();
    for tile_row in 0..down {
        for tile_col in 0..across {
            let mut bytes = Vec::new();
            for y in 0..length {
                for x in 0..width {
                    let px = tile_col * width + x;
                    let py = tile_row * length + y;
                    if px < page.width && py < page.height {
                        for s in 0..page.samples {
                            push_sample(&mut bytes, page.sample_type, px, py, s);
                        }
                    } else {
                        bytes.extend(std::iter::repeat_n(0u8, padding));
                    }
                }
            }
            chunks.push(bytes);
        }
    }
    chunks
}

/// The tiles of a page, each DEFLATE-compressed: the chunks a reader DECODES
/// instead of copying them out of the file.
fn compressed_tile_chunks(page: &Page, width: u32, length: u32) -> Vec<Vec<u8>> {
    tile_chunks(page, width, length)
        .into_iter()
        .map(|tile| deflate(&tile))
        .collect()
}

/// The bytes of a chunk, DEFLATE-compressed the way TIFF states them
/// (`Compression = 8`: the zlib stream the `tiff` crate decodes).
fn deflate(bytes: &[u8]) -> Vec<u8> {
    let mut encoder =
        flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).expect("compress the chunk");
    encoder.finish().expect("finish the compression")
}

/// How many tiles a page is cut into.
fn tile_count(page: &Page, width: u32, length: u32) -> u32 {
    page.width.div_ceil(width) * page.height.div_ceil(length)
}

/// The pixels of ONE tile — the tile at the origin — with the padding a tile
/// that hangs over the page's edge carries.
///
/// WHY one tile: a page whose plane is over the decode ceiling states far
/// more tiles than the file holds pixels for, so every tile points at this
/// block.
fn tile_block(page: &Page, width: u32, length: u32) -> Vec<u8> {
    let padding = page.sample_type.bits() as usize / 8 * page.samples as usize;
    let mut bytes = Vec::new();
    for y in 0..length {
        for x in 0..width {
            if x < page.width && y < page.height {
                for s in 0..page.samples {
                    push_sample(&mut bytes, page.sample_type, x, y, s);
                }
            } else {
                bytes.extend(std::iter::repeat_n(0u8, padding));
            }
        }
    }
    bytes
}

/// The strips of an indexed page whose first strip ends in the middle of a
/// row: the plane's index bytes, cut at `first_bytes`.
///
/// WHY: a strip that ends in the middle of a row does not say where the next
/// row starts, so a window cannot be read out of such a page.
fn split_strip_chunks(page: &Page, first_bytes: u32) -> Vec<Vec<u8>> {
    let mut all = Vec::with_capacity((page.width * page.height) as usize);
    for y in 0..page.height {
        for x in 0..page.width {
            all.push(u8::at(x, y, 0));
        }
    }
    let cut = (first_bytes as usize).min(all.len());
    vec![all[..cut].to_vec(), all[cut..].to_vec()]
}

/// The strips of an indexed page whose strips hold one row MORE than the
/// `RowsPerStrip` tag says: the tag and the strips disagree, so a reader
/// that follows the tag reads the page's rows at the wrong places.
///
/// WHY: the plane is read by concatenating the strips, so the plane's rows
/// are right; the window path reads the strips by the grid, which cuts the
/// page into strips of `RowsPerStrip` rows. A page whose strips disagree with
/// the tag must therefore keep the default path.
fn shifted_strip_chunks(page: &Page, rows_per_strip: u32) -> Vec<Vec<u8>> {
    let strips = page.height.div_ceil(rows_per_strip);
    let mut chunks = Vec::new();
    for strip in 0..strips {
        let first_row = strip * (rows_per_strip + 1);
        let last_row = (first_row + rows_per_strip + 1).min(page.height);
        let mut bytes = Vec::new();
        for y in first_row..last_row {
            for x in 0..page.width {
                bytes.push(u8::at(x, y, 0));
            }
        }
        chunks.push(bytes);
    }
    chunks
}

/// The bytes of one sample, in the page's sample type.
fn push_sample(out: &mut Vec<u8>, sample_type: SampleType, x: u32, y: u32, s: u16) {
    match sample_type {
        SampleType::U8 => out.push(u8::at(x, y, s)),
        SampleType::U16 => out.extend_from_slice(&u16::at(x, y, s).to_le_bytes()),
        SampleType::I16 => out.extend_from_slice(&i16::at(x, y, s).to_le_bytes()),
        SampleType::F32 => out.extend_from_slice(&f32::at(x, y, s).to_le_bytes()),
    }
}
