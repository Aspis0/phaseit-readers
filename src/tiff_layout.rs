// SPDX-License-Identifier: Apache-2.0
//! How one TIFF page stores its pixels, and what a window of it costs.
//!
//! A page's pixels are stored in chunks — strips (rows) or tiles — and the
//! page's `Compression` and `Predictor` tags say how those chunks are coded.
//! This module reads that layout and answers the ONE question a reader has to
//! answer before it can serve a window: is a window of this page cheaper to
//! read than the whole plane?
//!
//! WHY the question matters: a pyramid asks for 512x512 tiles and reads each
//! of them on its own. A reader that decodes a WHOLE CHUNK per window pays
//! the chunk's cost once per window, so a page whose chunk IS the plane (one
//! strip per plane — the layout Bio-Formats writes) would decode the plane
//! once per tile. The default path (decode the plane once, crop it in memory)
//! is cheaper there, and the file says so: `supports_region()` is false and
//! the reader keeps the default path.
//!
//! The decision is made ONCE per page, at open, from the page's own tags —
//! so `open()` does not read the file's IFD chain a second time to reach it.
//! The decision itself — which path a page is served by — is
//! [`crate::tiff_region::page_service`]'s.

use std::io::Cursor;

use tiff::decoder::{ChunkType, Decoder};
use tiff::tags::Tag;

use crate::error::{Error, Result};
use crate::tiff_page::PageFacts;
use crate::tiff_tags;
use crate::types::{Endian, PixelRect};

/// The `Compression` (259) tag value of a page whose chunks hold the pixels as
/// they are.
pub(crate) const COMPRESSION_NONE: u16 = 1;

/// The `Predictor` (317) tag value of a page stored without a predictor.
pub(crate) const PREDICTOR_NONE: u16 = 1;

/// The most a single chunk may weigh for the chunk path to be the cheaper
/// one: 64 MiB.
///
/// WHY it is public in the crate: the decoder allocates the chunk it decodes,
/// so a window read's decoder limits must be wide enough for the biggest
/// chunk a served page can have — and this is that bound.
pub(crate) const MAX_CHUNK_BYTES: usize = 64 * 1024 * 1024;

/// The most a single chunk may weigh, as a share of the plane it belongs to:
/// a sixteenth.
const MAX_CHUNK_SHARE: usize = 16;

/// The byte order of the file the decoder reads: the order the file's own
/// samples are stored in.
pub(crate) fn file_endian(dec: &Decoder<Cursor<&[u8]>>) -> Endian {
    match dec.byte_order() {
        tiff::tags::ByteOrder::LittleEndian => Endian::Little,
        tiff::tags::ByteOrder::BigEndian => Endian::Big,
    }
}

/// How a page's pixels are cut into chunks.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Chunking {
    /// Rows of `rows_per_strip` pixels, top to bottom.
    Strips { rows_per_strip: usize },
    /// `tile_width` by `tile_length` tiles, left to right and then top to bottom.
    Tiles {
        tile_width: usize,
        tile_length: usize,
    },
}

/// How one page stores its pixels.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PageLayout {
    /// The `Compression` tag: 1 = the chunks hold the pixels as they are.
    pub compression: u16,
    /// The `Predictor` tag: 1 = the chunks hold the pixels as they are.
    pub predictor: u16,
    /// How the page is cut into chunks.
    pub chunking: Chunking,
}

/// Read the layout of the page the decoder is on.
pub(crate) fn read_layout(dec: &mut Decoder<Cursor<&[u8]>>) -> Result<PageLayout> {
    let compression = tiff_tags::compression(dec)?;
    let predictor = tiff_tags::predictor(dec)?;
    let (chunk_width, chunk_height) = dec.chunk_dimensions();
    let chunking = match dec.get_chunk_type() {
        ChunkType::Strip => Chunking::Strips {
            rows_per_strip: chunk_height as usize,
        },
        ChunkType::Tile => Chunking::Tiles {
            tile_width: chunk_width as usize,
            tile_length: chunk_height as usize,
        },
    };
    Ok(PageLayout {
        compression,
        predictor,
        chunking,
    })
}

/// The chunk grid of one page: how the page is cut up and how big it is.
pub(crate) struct ChunkGrid {
    chunking: Chunking,
    width: usize,
    height: usize,
    samples: usize,
    itemsize: usize,
}

impl ChunkGrid {
    /// The grid of the page the facts describe.
    pub(crate) fn of(layout: &PageLayout, facts: &PageFacts) -> Self {
        Self {
            chunking: layout.chunking,
            width: facts.width,
            height: facts.height,
            samples: facts.samples,
            itemsize: facts.dtype.size_bytes(),
        }
    }

    /// The bytes of one pixel of the plane (all its samples).
    pub(crate) fn pixel_bytes(&self) -> usize {
        self.samples * self.itemsize
    }

    /// The bytes of one row of the plane (all its pixels).
    pub(crate) fn row_bytes(&self) -> usize {
        self.width * self.pixel_bytes()
    }

    /// The bytes of one row INSIDE a chunk: a strip's row is the plane's row,
    /// a tile's row is the tile's FULL width.
    pub(crate) fn chunk_row_bytes(&self) -> usize {
        match self.chunking {
            Chunking::Strips { .. } => self.row_bytes(),
            Chunking::Tiles { tile_width, .. } => tile_width * self.pixel_bytes(),
        }
    }

    /// How many chunks the page is cut into.
    pub(crate) fn chunk_count(&self) -> usize {
        match self.chunking {
            Chunking::Strips { rows_per_strip } => {
                if rows_per_strip == 0 {
                    0
                } else {
                    self.height.div_ceil(rows_per_strip)
                }
            }
            Chunking::Tiles {
                tile_width,
                tile_length,
            } => {
                if tile_width == 0 || tile_length == 0 {
                    0
                } else {
                    self.width.div_ceil(tile_width) * self.height.div_ceil(tile_length)
                }
            }
        }
    }

    /// The pixels chunk `index` holds: its origin and its size.
    ///
    /// A strip spans the page's width; a tile is clipped at the page's right
    /// and bottom edge, exactly as the `tiff` crate clips it.
    pub(crate) fn chunk_rect(&self, index: usize) -> (usize, usize, usize, usize) {
        match self.chunking {
            Chunking::Strips { rows_per_strip } => {
                let y = index * rows_per_strip;
                (0, y, self.width, rows_per_strip.min(self.height - y))
            }
            Chunking::Tiles {
                tile_width,
                tile_length,
            } => {
                let across = self.width.div_ceil(tile_width);
                let x = (index % across) * tile_width;
                let y = (index / across) * tile_length;
                (
                    x,
                    y,
                    tile_width.min(self.width - x),
                    tile_length.min(self.height - y),
                )
            }
        }
    }

    /// The chunks that hold a pixel of `rect`, in the order they are read.
    pub(crate) fn chunks_of(&self, rect: &PixelRect) -> Vec<usize> {
        if rect.is_empty() {
            return Vec::new();
        }
        match self.chunking {
            Chunking::Strips { rows_per_strip } => {
                if rows_per_strip == 0 {
                    return Vec::new();
                }
                let first = rect.y / rows_per_strip;
                let last = (rect.y + rect.h - 1) / rows_per_strip;
                (first..=last).collect()
            }
            Chunking::Tiles {
                tile_width,
                tile_length,
            } => {
                let across = self.width.div_ceil(tile_width);
                let first_row = rect.y / tile_length;
                let last_row = (rect.y + rect.h - 1) / tile_length;
                let first_col = rect.x / tile_width;
                let last_col = (rect.x + rect.w - 1) / tile_width;
                let mut chunks = Vec::new();
                for row in first_row..=last_row {
                    for col in first_col..=last_col {
                        chunks.push(row * across + col);
                    }
                }
                chunks
            }
        }
    }

    /// The rows chunk `index` holds, and the row it starts at.
    pub(crate) fn chunk_rows(&self, index: usize) -> (usize, usize) {
        let (_, y, _, h) = self.chunk_rect(index);
        (y, h)
    }
}

/// Where the page's chunks are in the file, and how long each one is.
#[derive(Debug, Clone)]
pub(crate) struct ChunkTable {
    /// The byte offset of each chunk in the file.
    pub offsets: Vec<u64>,
    /// The byte length of each chunk.
    pub bytes: Vec<u64>,
}

impl ChunkTable {
    /// The table of a page whose chunks are strips or tiles.
    pub(crate) fn of(dec: &mut Decoder<Cursor<&[u8]>>, chunking: &Chunking) -> Result<ChunkTable> {
        let (offsets_tag, counts_tag, offsets_name, counts_name) = match chunking {
            Chunking::Strips { .. } => (
                Tag::StripOffsets,
                Tag::StripByteCounts,
                "StripOffsets",
                "StripByteCounts",
            ),
            Chunking::Tiles { .. } => (
                Tag::TileOffsets,
                Tag::TileByteCounts,
                "TileOffsets",
                "TileByteCounts",
            ),
        };
        let offsets = tiff_tags::tag_uints(dec, offsets_tag, offsets_name)?
            .ok_or_else(|| Error::Decode(format!("TIFF page states no {offsets_name}")))?;
        let bytes = tiff_tags::tag_uints(dec, counts_tag, counts_name)?
            .ok_or_else(|| Error::Decode(format!("TIFF page states no {counts_name}")))?;
        if offsets.len() != bytes.len() {
            return Err(Error::Decode(format!(
                "TIFF page states {} {offsets_name} and {} {counts_name}",
                offsets.len(),
                bytes.len()
            )));
        }
        Ok(ChunkTable { offsets, bytes })
    }

    /// The table of a page's strips, whatever the page's chunks are. An
    /// indexed page is read from its strips.
    pub(crate) fn strips(dec: &mut Decoder<Cursor<&[u8]>>) -> Result<ChunkTable> {
        ChunkTable::of(dec, &Chunking::Strips { rows_per_strip: 1 })
    }
}

/// The bytes of the whole page: every pixel, every sample.
///
/// `None` when the size does not fit `usize`.
pub(crate) fn page_bytes(facts: &PageFacts) -> Option<usize> {
    facts
        .width
        .checked_mul(facts.height)?
        .checked_mul(facts.samples)?
        .checked_mul(facts.dtype.size_bytes())
}

/// The bytes of the biggest chunk of the page: its chunk's pixels, every
/// sample.
///
/// `None` when the size does not fit `usize`.
pub(crate) fn chunk_bytes(layout: &PageLayout, facts: &PageFacts) -> Option<usize> {
    let (chunk_width, chunk_height) = match layout.chunking {
        Chunking::Strips { rows_per_strip } => (facts.width, rows_per_strip),
        Chunking::Tiles {
            tile_width,
            tile_length,
        } => (tile_width, tile_length),
    };
    chunk_width
        .checked_mul(chunk_height)?
        .checked_mul(facts.samples)?
        .checked_mul(facts.dtype.size_bytes())
}

/// The cost rule: is a window read of a COMPRESSED page cheaper than decoding
/// the plane?
///
/// WHY a sixteenth: a pyramid asks for 512x512 tiles, so it reads one window
/// per tile and the default path decodes the whole plane ONCE. A chunk read
/// per window costs `windows * chunk_bytes` instead of `plane_bytes`, so the
/// chunk path only pays off while a chunk is much smaller than the plane. A
/// page whose chunks ARE the plane — one strip per plane, the layout
/// Bio-Formats writes — is therefore NOT served by the chunk path: it would
/// decode the plane once per tile.
///
/// WHY the 64 MiB cap: the crate allocates the chunk it decodes, so one
/// chunk's own buffer must stay inside the memory budget the decode ceiling
/// sets, whatever the plane weighs.
///
/// ONE rule, decided once per page, at open.
pub(crate) fn chunks_are_cheap(chunk_bytes: usize, page_bytes: usize) -> bool {
    chunk_bytes <= MAX_CHUNK_BYTES && chunk_bytes <= page_bytes / MAX_CHUNK_SHARE
}
