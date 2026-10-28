// SPDX-License-Identifier: Apache-2.0
//! What a TIFF page of a fixture states: the sample type it carries, the
//! value of a pixel, and how its pixels are cut into chunks.
//!
//! WHY the pixels vary: a region read is checked against a whole-plane read,
//! and a file whose rows all carry the same value cannot tell a right window
//! from a wrong one.

/// The sample type of a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleType {
    U8,
    U16,
    /// Signed 16-bit (`SampleFormat = 2`).
    I16,
    F32,
}

impl SampleType {
    /// The `BitsPerSample` the page states.
    pub fn bits(self) -> u16 {
        match self {
            SampleType::U8 => 8,
            SampleType::U16 | SampleType::I16 => 16,
            SampleType::F32 => 32,
        }
    }

    /// The `SampleFormat` the page states: 1 = unsigned int, 2 = signed int,
    /// 3 = IEEE float.
    pub fn sample_format(self) -> u16 {
        match self {
            SampleType::U8 | SampleType::U16 => 1,
            SampleType::I16 => 2,
            SampleType::F32 => 3,
        }
    }
}

/// The sample value at a coordinate: it changes with every row, column and
/// sample, so a window read from the wrong place is visible.
pub trait Pattern: Sized {
    /// The value of sample `s` of the pixel at `(x, y)`.
    fn at(x: u32, y: u32, s: u16) -> Self;
}

impl Pattern for u8 {
    fn at(x: u32, y: u32, s: u16) -> u8 {
        ((x * 7 + y * 53 + u32::from(s) * 11) % 251) as u8
    }
}

impl Pattern for u16 {
    fn at(x: u32, y: u32, s: u16) -> u16 {
        ((x * 7 + y * 53 + u32::from(s) * 11) % 60013) as u16
    }
}

impl Pattern for i16 {
    fn at(x: u32, y: u32, s: u16) -> i16 {
        // The modulus keeps the value inside a signed 16-bit sample.
        ((x * 7 + y * 53 + u32::from(s) * 11) % 30_011) as i16
    }
}

impl Pattern for f32 {
    fn at(x: u32, y: u32, s: u16) -> f32 {
        // Exactly representable in f32, so a comparison stays exact.
        x as f32 * 0.5 + y as f32 * 1024.0 + s as f32
    }
}

/// How a page's pixels are cut into chunks.
#[derive(Debug, Clone, Copy)]
pub enum Layout {
    /// Rows of `rows_per_strip` pixels, every sample of a pixel together.
    Strips { rows_per_strip: u32 },
    /// Rows of `rows_per_strip` pixels, each sample of a pixel in its own plane.
    PlanarStrips { rows_per_strip: u32 },
    /// `width` by `length` tiles, every sample of a pixel together.
    Tiles { width: u32, length: u32 },
    /// `width` by `length` tiles, each DEFLATE-compressed: the chunks are
    /// DECODED by the reader, not copied out of the file.
    CompressedTiles { width: u32, length: u32 },
    /// `tile_width` by `tile_length` tiles that ALL point at the same block of
    /// pixels: the page states a plane far bigger than the file holds, so the
    /// file is small while the page says its plane is huge.
    SharedTiles {
        tile_width: u32,
        tile_length: u32,
    },
    /// Rows of `rows_per_strip` pixels of INDICES, with the palette in a
    /// `ColorMap` tag: the photometric interpretation is 3 (palette), and the
    /// page's pixels ARE the index values.
    IndexedStrips { rows_per_strip: u32 },
    /// The plane's INDICES cut into two strips whose first one ends in the
    /// MIDDLE of a row: the strips do not hold whole rows, so a window cannot
    /// be read out of them.
    IndexedSplitStrip { first_bytes: u32 },
    /// Rows of INDICES whose strips hold one row MORE than the `RowsPerStrip`
    /// tag says: the tag and the strips disagree, so a reader that follows
    /// the tag reads the page's rows at the wrong places.
    IndexedShiftedStrips { rows_per_strip: u32 },
}

/// One page of a TIFF file, in a layout the `tiff` crate's encoder cannot
/// write.
pub struct Page {
    pub width: u32,
    pub height: u32,
    /// Samples per pixel: 1 = grey, 3 = RGB.
    pub samples: u16,
    pub sample_type: SampleType,
    pub layout: Layout,
}
