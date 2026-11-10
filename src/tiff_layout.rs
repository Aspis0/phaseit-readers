// SPDX-License-Identifier: Apache-2.0
//! How one TIFF page stores its pixels, and whether a window of it is cheaper to read than the whole plane.
//! A page whose chunk IS the plane (one strip per plane, as Bio-Formats writes) would be decoded once per tile
//! by the chunk path, so it keeps the default path. The decision is made once per page, at open.

use std::io::Cursor;

use tiff::decoder::{ChunkType, Decoder};
use tiff::tags::Tag;

use crate::error::{Error, Result};
use crate::tiff_page::PageFacts;
use crate::tiff_tags;
use crate::types::{Endian, PixelRect};

pub(crate) const COMPRESSION_NONE: u16 = 1;

pub(crate) const PREDICTOR_NONE: u16 = 1;

/// The most a single chunk may weigh (64 MiB): the decoder allocates the chunk, so this bounds a window read's limits.
/// It must cover the biggest chunk a served page can have.
pub(crate) const MAX_CHUNK_BYTES: usize = 64 * 1024 * 1024;

const MAX_CHUNK_SHARE: usize = 16;

pub(crate) fn file_endian(dec: &Decoder<Cursor<&[u8]>>) -> Endian {
    match dec.byte_order() {
        tiff::tags::ByteOrder::LittleEndian => Endian::Little,
        tiff::tags::ByteOrder::BigEndian => Endian::Big,
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Chunking {
    Strips { rows_per_strip: usize },
    /// `tile_width` by `tile_length` tiles, left to right and then top to bottom.
    Tiles {
        tile_width: usize,
        tile_length: usize,
    },
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PageLayout {
    pub compression: u16,
    pub predictor: u16,
    pub chunking: Chunking,
}

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

pub(crate) struct ChunkGrid {
    chunking: Chunking,
    width: usize,
    height: usize,
    samples: usize,
    itemsize: usize,
}

impl ChunkGrid {
    pub(crate) fn of(layout: &PageLayout, facts: &PageFacts) -> Self {
        Self {
            chunking: layout.chunking,
            width: facts.width,
            height: facts.height,
            samples: facts.samples,
            itemsize: facts.dtype.size_bytes(),
        }
    }

    pub(crate) fn pixel_bytes(&self) -> usize {
        self.samples * self.itemsize
    }

    pub(crate) fn row_bytes(&self) -> usize {
        self.width * self.pixel_bytes()
    }

    /// A tile's row is its FULL width, which is wider than the plane's last tile column.
    pub(crate) fn chunk_row_bytes(&self) -> usize {
        match self.chunking {
            Chunking::Strips { .. } => self.row_bytes(),
            Chunking::Tiles { tile_width, .. } => tile_width * self.pixel_bytes(),
        }
    }

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

    /// A tile is clipped at the right and bottom edge exactly as the `tiff` crate clips it.
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

    pub(crate) fn chunk_rows(&self, index: usize) -> (usize, usize) {
        let (_, y, _, h) = self.chunk_rect(index);
        (y, h)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ChunkTable {
    pub offsets: Vec<u64>,
    pub bytes: Vec<u64>,
}

impl ChunkTable {
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

    /// An indexed page is read from its strips, whatever its chunks are.
    pub(crate) fn strips(dec: &mut Decoder<Cursor<&[u8]>>) -> Result<ChunkTable> {
        ChunkTable::of(dec, &Chunking::Strips { rows_per_strip: 1 })
    }
}

pub(crate) fn page_bytes(facts: &PageFacts) -> Option<usize> {
    facts
        .width
        .checked_mul(facts.height)?
        .checked_mul(facts.samples)?
        .checked_mul(facts.dtype.size_bytes())
}

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

/// Is a window read of a compressed page cheaper than decoding the plane once? A pyramid reads one window per tile,
/// so a chunk read per window costs `windows * chunk_bytes`: a page whose chunk IS the plane takes the default path.
/// The 64 MiB cap keeps one chunk's own buffer inside the decode-ceiling budget, whatever the plane weighs.
pub(crate) fn chunks_are_cheap(chunk_bytes: usize, page_bytes: usize) -> bool {
    chunk_bytes <= MAX_CHUNK_BYTES && chunk_bytes <= page_bytes / MAX_CHUNK_SHARE
}
