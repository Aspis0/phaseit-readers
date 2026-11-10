// SPDX-License-Identifier: Apache-2.0
//! The CZI containers the scene tests read back: each tile names its scene (`S`), so a scene can be made unreadable.
//! The coarse tiles of the file's own pyramid are written too: their stored size differs from their logical size.

use phasit_imread::{czi_reader::CziReader, types::PixelRect, ImageReader};


use super::scratch::Scratch;

#[derive(Clone)]
pub struct Tile {
    pub scene: i32,
    /// CZI `PixelType` id: 0 Gray8, 1 Gray16, 3 Bgr24, 10 Gray32 (not modelled by the reader).
    pub pixel_type: i32,
    pub width: u32,
    pub height: u32,
    /// The stored size of a sub-sampled (pyramid) tile; `None` means full resolution.
    pub stored: Option<(u32, u32)>,
    pub samples: u32,
    /// The tile's pixel bytes; `None` fills the tile with a constant.
    pub pixels: Option<Vec<u8>>,
    /// The tile's origin in the scene.
    pub x: i32,
    pub y: i32,
    /// The acquisition channel of the tile.
    pub c: i32,
    /// The CZI `PyramidType` byte: 0 is full resolution, 2 is the legacy multi-subblock byte.
    pub pyramid_type: u8,
    /// The CZI `Compression` id; 0 is uncompressed, which is what the helper's pixels are.
    pub compression: i32,
}

impl Tile {
    pub fn full(scene: i32, pixel_type: i32, width: u32, height: u32) -> Tile {
        Tile {
            scene,
            pixel_type,
            width,
            height,
            stored: None,
            samples: samples_of(pixel_type),
            pixels: None,
            x: 0,
            y: 0,
            c: 0,
            pyramid_type: 0,
            compression: 0,
        }
    }

    /// Sub-sampled: its stored size is half its size, so the reader must not compose it into a scene.
    pub fn pyramid(scene: i32, pixel_type: i32, width: u32, height: u32) -> Tile {
        Tile {
            scene,
            pixel_type,
            width,
            height,
            stored: Some((width / 2, height / 2)),
            samples: samples_of(pixel_type),
            pixels: None,
            x: 0,
            y: 0,
            c: 0,
            pyramid_type: 0,
            compression: 0,
        }
    }

    /// Pixels in the FILE's order (BGR for Bgr24): the reader swaps B and R on the way out.
    pub fn with_pixels(
        scene: i32,
        pixel_type: i32,
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    ) -> Tile {
        Tile {
            scene,
            pixel_type,
            width,
            height,
            stored: None,
            samples: samples_of(pixel_type),
            pixels: Some(pixels),
            x: 0,
            y: 0,
            c: 0,
            pyramid_type: 0,
            compression: 0,
        }
    }

    /// The tile at `(x, y)` in the scene.
    pub fn at(mut self, x: i32, y: i32) -> Tile {
        self.x = x;
        self.y = y;
        self
    }

    /// The tile of the acquisition channel `c`.
    pub fn channel(mut self, c: i32) -> Tile {
        self.c = c;
        self
    }

    /// The tile with these pixel bytes.
    pub fn painted(mut self, pixels: Vec<u8>) -> Tile {
        self.pixels = Some(pixels);
        self
    }

    pub fn stored_x(&self) -> u32 {
        self.stored.map(|(x, _)| x).unwrap_or(self.width)
    }

    pub fn stored_y(&self) -> u32 {
        self.stored.map(|(_, y)| y).unwrap_or(self.height)
    }
}

/// The scene of the pyramid tests: three full-resolution tiles of 64 pixels, so 192x64.
pub fn scene() -> Vec<Tile> {
    full_resolution_tiles(3, 1, 64, 0, 0)
}

/// The level of a scene that covers it whole: one tile per 64 source pixels.
pub fn whole_level(tiles: &mut Vec<Tile>) {
    tiles.push(level_tile(0, 2, 0, 0, 64, 64, 0));
    tiles.push(level_tile(0, 2, 64, 0, 64, 64, 0));
    tiles.push(level_tile(0, 2, 128, 0, 64, 64, 0));
}

/// The full-resolution pixels of the scene, as the reader reads them.
pub fn full_resolution(reader: &CziReader, rect: PixelRect) -> Vec<u8> {
    let plane = reader.region(0, 0, 0, 0, rect).expect("the full resolution reads");
    plane_bytes(&plane).to_vec()
}

/// The samples a pixel type carries.
fn samples_of(pixel_type: i32) -> u32 {
    match pixel_type {
        3 | 4 => 3, // Bgr24, Bgr48
        _ => 1,
    }
}

/// The full-resolution tiles of a scene of `cols` x `rows` tiles of `tile` pixels.
///
/// The pixels are the source ramp, so a level's tile can be the box average of the pixels under it.
pub fn full_resolution_tiles(
    cols: u32,
    rows: u32,
    tile: u32,
    pixel_type: i32,
    channel: i32,
) -> Vec<Tile> {
    let samples = samples_of(pixel_type);
    let mut tiles = Vec::new();
    for row in 0..rows {
        for col in 0..cols {
            let x = (col * tile) as i32;
            let y = (row * tile) as i32;
            tiles.push(
                Tile::full(0, pixel_type, tile, tile)
                    .at(x, y)
                    .channel(channel)
                    .painted(source_pixels(x, y, tile, tile, samples)),
            );
        }
    }
    tiles
}

/// A tile of the file's own pyramid: `width` x `height` source pixels, stored at `factor`.
///
/// Its pixels are the box average of the source ramp under it, so the level and the full resolution agree.
pub fn level_tile(
    pixel_type: i32,
    factor: u32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    channel: i32,
) -> Tile {
    let samples = samples_of(pixel_type);
    let source = source_pixels(x, y, width, height, samples);
    let mut tile = Tile::full(0, pixel_type, width, height)
        .at(x, y)
        .channel(channel)
        .painted(downsample(&source, width, height, samples, factor));
    tile.stored = Some((width / factor, height / factor));
    tile.pyramid_type = 2;
    tile
}

/// The source pixels of the rectangle at `(x, y)`, in the file's order (BGR for a color tile).
///
/// WHY a ramp: the value changes with the position, so a tile that lands one pixel off is visible.
pub fn source_pixels(x: i32, y: i32, width: u32, height: u32, samples: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((width * height * samples) as usize);
    for row in 0..height {
        for col in 0..width {
            let value = ramp_value(x + col as i32, y + row as i32);
            if samples == 1 {
                out.push(value);
                continue;
            }
            out.push(value / 2);
            out.push(255 - value);
            out.push(value);
        }
    }
    out
}

/// One source pixel's value: a ramp of the position.
fn ramp_value(x: i32, y: i32) -> u8 {
    (x + y).rem_euclid(251) as u8
}

/// The box average of each `factor` x `factor` block of a source plane.
pub fn downsample(source: &[u8], width: u32, height: u32, samples: u32, factor: u32) -> Vec<u8> {
    let out_w = (width / factor) as usize;
    let out_h = (height / factor) as usize;
    let channels = samples as usize;
    let mut out = vec![0u8; out_w * out_h * channels];
    for row in 0..out_h {
        for col in 0..out_w {
            for channel in 0..channels {
                let mut sum = 0u32;
                let mut count = 0u32;
                for dy in 0..factor as usize {
                    for dx in 0..factor as usize {
                        let base = (row * factor as usize + dy) * width as usize
                            + col * factor as usize
                            + dx;
                        let index = base * channels + channel;
                        sum += source[index] as u32;
                        count += 1;
                    }
                }
                out[(row * out_w + col) * channels + channel] = ((sum + count / 2) / count) as u8;
            }
        }
    }
    out
}

/// The bytes of a plane, as the tests compare them.
pub fn plane_bytes(plane: &phasit_imread::Plane) -> &[u8] {
    match &plane.data {
        phasit_imread::PlaneBuf::U8(values) => values,
        other => panic!("expected U8 data, got {:?}", other.dtype()),
    }
}

pub fn scratch(name: &str) -> Scratch {
    Scratch::new(
        std::env::temp_dir().join(format!("phasit_czi_refusal_{name}_{}", std::process::id())),
    )
}
