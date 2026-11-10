//! The fill of the parts a level's tiles do not cover: the level-0 pixels they are made of.
//!
//! The fill reads only the level-0 pixels that exist: a part of the hole under which level 0 has
//! no tile is blank at level 0 too, and stays blank here.

use crate::error::Result;
use crate::types::{DType, Endian, PixelRect, PlaneBuf};

use super::coverage::Coverage;
use super::grid::div_ceil;
use super::{FullRes, Snap};

/// The most bytes one composed block of the fill may hold in source pixels.
///
/// WHY the sums are named too: a block also holds its per-level-pixel sample sums, at most twice
/// `MAX_FILL_BYTES` (a factor-of-two block of one-byte samples sums one f64 per source byte), so
/// one block's live memory stays within `3 * MAX_FILL_BYTES`.
const MAX_FILL_BYTES: usize = 8 * 1024 * 1024;

/// The level's pixels over a rect, as they are composed.
pub(super) struct LevelPixels {
    /// The pixels, row-major: `height` rows of `width` pixels.
    pub pixels: Vec<u8>,
    /// The width of the composed plane, in level pixels.
    pub width: usize,
    /// The height of the composed plane, in level pixels.
    pub height: usize,
    /// The pixel type of the level.
    pub dtype: DType,
    /// The samples per pixel.
    pub samples: u32,
}

impl LevelPixels {
    /// The size of one pixel, in bytes.
    fn pixel_bytes(&self) -> usize {
        self.dtype.size_bytes() * self.samples as usize
    }
}

/// The level pixels of one block of the fill.
struct Block {
    /// The first level row of the block.
    row0: i64,
    /// The last level row of the block, exclusive.
    row1: i64,
    /// The first level column of the block.
    col0: i64,
    /// The last level column of the block, exclusive.
    col1: i64,
}

/// The sums of the level-0 samples of one block of level pixels.
struct Sums {
    /// The sum of the samples of each level pixel, in the block's order.
    values: Vec<f64>,
    /// The level pixels in a row of the block.
    columns: usize,
    /// The channels each level pixel carries.
    channels: usize,
}

/// Fills the parts of the read the level's tiles do not cover, from the full resolution.
///
/// WHY a box average: the level's own pixels are the scanner's minification, and the filled ones
/// are phaseit's; a box average of the full resolution is the closest of the two.
pub(super) fn fill(
    level: &Coverage,
    full: &Coverage,
    snap: &Snap,
    factor: usize,
    plane: &mut LevelPixels,
    full_res: &mut FullRes<'_>,
) -> Result<()> {
    let frame = snap.frame(factor);
    // Level 0 exists only inside the scene, so the fill averages only the scene's pixels.
    let holes: Vec<PixelRect> = level
        .uncovered(frame)
        .into_iter()
        .map(|band| intersect(band, snap.scene))
        .filter(|band| !band.is_empty())
        .collect();
    if holes.is_empty() {
        return Ok(());
    }
    // The blocks run over the whole hole, not per band: bands split level pixels, and a pixel's
    // average is over its whole uncovered area, so one pixel is summed and written once.
    let (first_col, last_col) = span_of(&holes, factor, |hole| (hole.x as i64, hole.w as i64));
    let (first_row, last_row) = span_of(&holes, factor, |hole| (hole.y as i64, hole.h as i64));
    let (columns, rows) = block_shape(factor, plane.pixel_bytes());
    for col0 in (first_col..last_col).step_by(columns) {
        for row0 in (first_row..last_row).step_by(rows) {
            let block = Block {
                row0,
                row1: (row0 + rows as i64).min(last_row),
                col0,
                col1: (col0 + columns as i64).min(last_col),
            };
            let parts: Vec<PixelRect> = holes
                .iter()
                .map(|hole| intersect(*hole, block_source(&block, factor)))
                .filter(|part| !part.is_empty())
                .collect();
            if parts.is_empty() {
                continue;
            }
            fill_block(&block, &parts, full, snap, factor, plane, full_res)?;
        }
    }
    Ok(())
}

/// The level's columns or rows the holes together cover.
fn span_of(
    holes: &[PixelRect],
    factor: usize,
    edge: fn(&PixelRect) -> (i64, i64),
) -> (i64, i64) {
    holes
        .iter()
        .map(|hole| {
            let (start, length) = edge(hole);
            span(start, length, factor)
        })
        .fold((i64::MAX, i64::MIN), |(lo, hi), (a, b)| (lo.min(a), hi.max(b)))
}

/// Fills one block of level pixels from the level-0 pixels under its uncovered parts.
fn fill_block(
    block: &Block,
    parts: &[PixelRect],
    full: &Coverage,
    snap: &Snap,
    factor: usize,
    plane: &mut LevelPixels,
    full_res: &mut FullRes<'_>,
) -> Result<()> {
    let mut sums = Sums::new(block, plane.samples as usize);
    for part in parts {
        // Where level 0 has no tile the hole stays blank, so only its covered windows are read.
        for window in full.covered(*part) {
            let source = full_res.region(snap.to_scene(window))?;
            let native = PlaneBuf::from_bytes(plane.dtype, Endian::Little, &source)?;
            sums.add(&native, window, factor, block);
        }
    }
    sums.write(plane, block, parts, factor);
    Ok(())
}

/// The source rectangle of a block of level pixels, counted from the read's own corner.
fn block_source(block: &Block, factor: usize) -> PixelRect {
    let factor = factor as i64;
    PixelRect::new(
        (block.col0 * factor) as usize,
        (block.row0 * factor) as usize,
        ((block.col1 - block.col0) * factor) as usize,
        ((block.row1 - block.row0) * factor) as usize,
    )
}

/// The overlap of two rectangles.
fn intersect(a: PixelRect, b: PixelRect) -> PixelRect {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.w).min(b.x + b.w);
    let y1 = (a.y + a.h).min(b.y + b.h);
    PixelRect::new(
        x0,
        y0,
        x1.saturating_sub(x0),
        y1.saturating_sub(y0),
    )
}

impl Sums {
    /// The sums of the level pixels of a block.
    fn new(block: &Block, channels: usize) -> Sums {
        let columns = (block.col1 - block.col0) as usize;
        let rows = (block.row1 - block.row0) as usize;
        Sums {
            values: vec![0.0; columns * rows * channels],
            columns,
            channels,
        }
    }

    /// Adds the samples of a source rectangle to the level pixels they fall in.
    ///
    /// WHY the floor division: the part may start inside a level pixel — the first column of the
    /// scene is a partial one — and every source pixel belongs to the pixel that contains it.
    fn add(
        &mut self,
        source: &PlaneBuf,
        part: PixelRect,
        factor: usize,
        block: &Block,
    ) {
        for row in 0..part.h {
            let level_row = (part.y + row) / factor - block.row0 as usize;
            for column in 0..part.w {
                let level_column = (part.x + column) / factor - block.col0 as usize;
                let index = (level_row * self.columns + level_column) * self.channels;
                let source_index = row * part.w + column;
                for channel in 0..self.channels {
                    self.values[index + channel] +=
                        sample(source, source_index, channel, self.channels);
                }
            }
        }
    }

    /// Writes the averages of the block's level pixels.
    fn write(&self, plane: &mut LevelPixels, block: &Block, parts: &[PixelRect], factor: usize) {
        for row in block.row0..block.row1 {
            for column in block.col0..block.col1 {
                if row < 0 || column < 0 || row as usize >= plane.height || column as usize >= plane.width
                {
                    continue;
                }
                let count = block_count(row, column, parts, factor);
                if count == 0 {
                    continue;
                }
                let index = ((row - block.row0) as usize * self.columns
                    + (column - block.col0) as usize)
                    * self.channels;
                let target =
                    (row as usize * plane.width + column as usize) * plane.pixel_bytes();
                for channel in 0..self.channels {
                    let sum = self.values[index + channel];
                    let average = match plane.dtype {
                        DType::F32 => sum / count as f64,
                        _ => ((sum + (count / 2) as f64) / count as f64).floor(),
                    };
                    write_sample(plane, target, channel, average);
                }
            }
        }
    }
}

/// The number of source pixels the average of one level pixel divides by: its uncovered area,
/// over every part of the hole that meets it.
///
/// WHY the whole area: level 0 has no pixel where the scene has no tile, and the average it makes
/// of such a hole is a lower one.
fn block_count(row: i64, column: i64, parts: &[PixelRect], factor: usize) -> usize {
    let factor = factor as i64;
    let (x0, x1) = (column * factor, (column + 1) * factor);
    let (y0, y1) = (row * factor, (row + 1) * factor);
    parts
        .iter()
        .map(|part| {
            let px0 = (part.x as i64).max(x0);
            let px1 = ((part.x + part.w) as i64).min(x1);
            let py0 = (part.y as i64).max(y0);
            let py1 = ((part.y + part.h) as i64).min(y1);
            // Each extent clamps on its own: clamping the product would let a part that misses
            // the pixel on both axes add its two negatives' positive product to the divisor.
            ((px1 - px0).max(0) * (py1 - py0).max(0)) as usize
        })
        .sum()
}

/// One sample of a native buffer, as a number.
fn sample(buffer: &PlaneBuf, index: usize, channel: usize, channels: usize) -> f64 {
    let at = index * channels + channel;
    match buffer {
        PlaneBuf::U8(values) => values[at] as f64,
        PlaneBuf::U16(values) => values[at] as f64,
        PlaneBuf::I16(values) => values[at] as f64,
        PlaneBuf::F32(values) => values[at] as f64,
    }
}

/// One sample of the composed plane, written as the little-endian bytes the plane holds.
fn write_sample(plane: &mut LevelPixels, target: usize, channel: usize, value: f64) {
    let offset = target + channel * plane.dtype.size_bytes();
    match plane.dtype {
        DType::U8 => plane.pixels[offset] = value as u8,
        DType::U16 => {
            let bytes = (value as u16).to_le_bytes();
            plane.pixels[offset..offset + 2].copy_from_slice(&bytes);
        }
        DType::I16 => {
            let bytes = (value as i16).to_le_bytes();
            plane.pixels[offset..offset + 2].copy_from_slice(&bytes);
        }
        DType::F32 => {
            let bytes = (value as f32).to_le_bytes();
            plane.pixels[offset..offset + 4].copy_from_slice(&bytes);
        }
    }
}

/// The level's columns that a source span covers.
fn span(start: i64, length: i64, factor: usize) -> (i64, i64) {
    let factor = factor as i64;
    (start.div_euclid(factor), div_ceil(start + length, factor))
}

/// The level pixels of one block of the fill, so that the source it composes stays within the cap.
///
/// WHY the cap: a hole of the level can be as wide as the scene, and the level-0 pixels under it
/// would then be one plane of their own.
fn block_shape(factor: usize, pixel_bytes: usize) -> (usize, usize) {
    let bytes = factor
        .saturating_mul(factor)
        .saturating_mul(pixel_bytes)
        .max(1);
    let pixels = (MAX_FILL_BYTES / bytes).max(1);
    let columns = pixels.isqrt().max(1);
    let rows = (pixels / columns).max(1);
    (columns, rows)
}

#[cfg(test)]
mod tests;
