// SPDX-License-Identifier: Apache-2.0
//! What a TIFF page of a fixture states: its sample type, its pixel values and its chunk layout.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleType {
    U8,
    U16,
    I16,
    F32,
}

impl SampleType {
    pub fn bits(self) -> u16 {
        match self {
            SampleType::U8 => 8,
            SampleType::U16 | SampleType::I16 => 16,
            SampleType::F32 => 32,
        }
    }

    pub fn sample_format(self) -> u16 {
        match self {
            SampleType::U8 | SampleType::U16 => 1,
            SampleType::I16 => 2,
            SampleType::F32 => 3,
        }
    }
}

pub trait Pattern: Sized {
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

#[derive(Debug, Clone, Copy)]
pub enum Layout {
    Strips { rows_per_strip: u32 },
    PlanarStrips { rows_per_strip: u32 },
    Tiles { width: u32, length: u32 },
    /// DEFLATE-compressed tiles: the reader decodes them, it does not copy them out of the file.
    CompressedTiles { width: u32, length: u32 },
    /// Every tile points at one block: the page states a plane far bigger than the file holds.
    SharedTiles {
        tile_width: u32,
        tile_length: u32,
    },
    /// Index values, with the palette in a `ColorMap` tag (photometric 3).
    IndexedStrips { rows_per_strip: u32 },
    /// The strips do not hold whole rows, so a window cannot be read out of them.
    IndexedSplitStrip { first_bytes: u32 },
    /// The strips hold one row more than the tag says, so a reader following the tag reads the wrong rows.
    IndexedShiftedStrips { rows_per_strip: u32 },
}

pub struct Page {
    pub width: u32,
    pub height: u32,
    pub samples: u16,
    pub sample_type: SampleType,
    pub layout: Layout,
}
