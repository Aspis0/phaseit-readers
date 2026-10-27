// SPDX-License-Identifier: Apache-2.0
//! Writes the small CZI containers the scene tests read back.
//!
//! The containers are written by hand (the reader's module docs spell the
//! segment layout out): a `ZISRAWFILE` header, a `ZISRAWDIRECTORY` listing one
//! entry per tile, one `ZISRAWSUBBLOCK` per tile, and a `ZISRAWMETADATA`
//! segment. Each tile names the scene (`S`) it belongs to, so a scene can be
//! made unreadable on purpose.

use std::path::Path;

use super::scratch::Scratch;

/// One tile of a synthetic CZI.
#[derive(Clone)]
pub struct Tile {
    /// The scene (`S`) the tile belongs to.
    pub scene: i32,
    /// The CZI `PixelType` id: 0 = Gray8, 1 = Gray16, 3 = Bgr24 (colour),
    /// 10 = Gray32 (not modelled by the reader).
    pub pixel_type: i32,
    /// The tile's size in pixels.
    pub width: u32,
    /// The tile's height in pixels.
    pub height: u32,
    /// The tile's STORED size, when it differs from its size: a sub-sampled
    /// (pyramid) tile. `None` means full resolution.
    pub stored: Option<(u32, u32)>,
    /// The samples per pixel the tile's pixel type carries (1 = grayscale,
    /// 3 = colour).
    pub samples: u32,
    /// The tile's pixel bytes, when the test states them. `None` fills the
    /// tile with a constant.
    pub pixels: Option<Vec<u8>>,
}

impl Tile {
    /// A full-resolution tile of the given scene and pixel type.
    pub fn full(scene: i32, pixel_type: i32, width: u32, height: u32) -> Tile {
        Tile {
            scene,
            pixel_type,
            width,
            height,
            stored: None,
            samples: samples_of(pixel_type),
            pixels: None,
        }
    }

    /// A sub-sampled (pyramid) tile: its stored size is half its size, so the
    /// reader must not compose it into a scene.
    pub fn pyramid(scene: i32, pixel_type: i32, width: u32, height: u32) -> Tile {
        Tile {
            scene,
            pixel_type,
            width,
            height,
            stored: Some((width / 2, height / 2)),
            samples: samples_of(pixel_type),
            pixels: None,
        }
    }

    /// A tile whose pixels the test states, in the order the FILE stores them
    /// (BGR for a Bgr24 tile — the reader swaps B and R on the way out, so
    /// the file's order is what a fixture has to state).
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
        }
    }

    /// The tile's stored width in pixels.
    fn stored_x(&self) -> u32 {
        self.stored.map(|(x, _)| x).unwrap_or(self.width)
    }

    /// The tile's stored height in pixels.
    fn stored_y(&self) -> u32 {
        self.stored.map(|(_, y)| y).unwrap_or(self.height)
    }
}

/// Bytes ONE SAMPLE of the tile's pixel type takes.
fn item_size(pixel_type: i32) -> usize {
    match pixel_type {
        1 | 4 => 2, // Gray16, Bgr48
        _ => 1,     // Gray8, Bgr24, and the types the reader does not model
    }
}

/// The samples per pixel the tile's pixel type carries.
fn samples_of(pixel_type: i32) -> u32 {
    match pixel_type {
        3 | 4 => 3, // Bgr24, Bgr48
        _ => 1,
    }
}

/// A segment header: the 16-byte null-padded id, then the allocated and used
/// size (both the same here: the segment holds exactly what it declares).
fn segment_header(id: &[u8; 16], allocated: i64) -> Vec<u8> {
    let mut out = Vec::with_capacity(32);
    out.extend_from_slice(id);
    out.extend_from_slice(&allocated.to_le_bytes());
    out.extend_from_slice(&allocated.to_le_bytes());
    out
}

/// One `DimensionEntryDV1`: the 4-byte dimension, its start, its size, its
/// start coordinate and its stored size.
fn dimension(out: &mut Vec<u8>, name: &[u8; 4], start: i32, size: i32, stored: i32) {
    out.extend_from_slice(name);
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&0f32.to_le_bytes());
    out.extend_from_slice(&stored.to_le_bytes());
}

/// One `DirectoryEntryDV` of the subblock directory, pointing at the tile's
/// `ZISRAWSUBBLOCK` segment.
fn directory_entry(tile: &Tile, file_position: i64) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0u8; 2]); // SchemaType
    out.extend_from_slice(&tile.pixel_type.to_le_bytes());
    out.extend_from_slice(&file_position.to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes()); // FilePart
    out.extend_from_slice(&0i32.to_le_bytes()); // Compression: uncompressed
    out.push(0); // PyramidType
    out.push(0); // spare
    out.extend_from_slice(&[0u8; 4]); // spare
    out.extend_from_slice(&6i32.to_le_bytes()); // DimensionCount: X, Y, S, C, Z, T
    dimension(
        &mut out,
        b"X\0\0\0",
        0,
        tile.width as i32,
        tile.stored_x() as i32,
    );
    dimension(
        &mut out,
        b"Y\0\0\0",
        0,
        tile.height as i32,
        tile.stored_y() as i32,
    );
    dimension(&mut out, b"S\0\0\0", tile.scene, 1, 1);
    dimension(&mut out, b"C\0\0\0", 0, 1, 1);
    dimension(&mut out, b"Z\0\0\0", 0, 1, 1);
    dimension(&mut out, b"T\0\0\0", 0, 1, 1);
    out
}

/// Write a CZI whose tiles are `tiles`, one scene per distinct `S` value.
pub fn write_czi(path: &Path, tiles: &[Tile]) {
    const FILE_HEADER_DATA: usize = 128;
    const ENTRY_LEN: usize = 32 + 6 * 20;
    const SUBBLOCK_FIXED: usize = 256;

    let xml = b"<Metadata><Data/></Metadata>";
    let dir_alloc = 4 + 124 + tiles.len() * ENTRY_LEN;
    let tile_bytes: Vec<usize> = tiles
        .iter()
        .map(|t| {
            t.stored_x() as usize
                * t.stored_y() as usize
                * t.samples as usize
                * item_size(t.pixel_type)
        })
        .collect();

    // Lay the segments out: the file header, the directory, one subblock per
    // tile, then the metadata segment.
    let dir_pos = FILE_HEADER_DATA + 32;
    let mut subblock_pos = dir_pos + 32 + dir_alloc;
    let mut subblock_positions = Vec::with_capacity(tiles.len());
    for size in &tile_bytes {
        subblock_positions.push(subblock_pos as i64);
        subblock_pos += 32 + SUBBLOCK_FIXED + size;
    }
    let meta_pos = subblock_pos;

    let mut buf = Vec::new();
    // The `ZISRAWFILE` segment: the directory and metadata positions live in
    // its data, 52 and 60 bytes in.
    buf.extend_from_slice(&segment_header(b"ZISRAWFILE\0\0\0\0\0\0", FILE_HEADER_DATA as i64));
    buf.extend_from_slice(&[0u8; FILE_HEADER_DATA]);
    let dir_at = buf.len() - FILE_HEADER_DATA + 52;
    buf[dir_at..dir_at + 8].copy_from_slice(&(dir_pos as i64).to_le_bytes());
    let meta_at = dir_at + 8;
    buf[meta_at..meta_at + 8].copy_from_slice(&(meta_pos as i64).to_le_bytes());

    // The `ZISRAWDIRECTORY` segment: the entry count, 124 reserved bytes, then
    // one entry per tile.
    buf.extend_from_slice(&segment_header(b"ZISRAWDIRECTORY\0", dir_alloc as i64));
    buf.extend_from_slice(&(tiles.len() as i32).to_le_bytes());
    buf.extend_from_slice(&[0u8; 124]);
    for (tile, position) in tiles.iter().zip(&subblock_positions) {
        buf.extend_from_slice(&directory_entry(tile, *position));
    }

    // One `ZISRAWSUBBLOCK` segment per tile: the fixed part (the header, the
    // tile's own directory entry and the fill) then the pixel bytes.
    for (tile, size) in tiles.iter().zip(&tile_bytes) {
        buf.extend_from_slice(&segment_header(
            b"ZISRAWSUBBLOCK\0\0",
            (SUBBLOCK_FIXED + size) as i64,
        ));
        let data_start = buf.len();
        buf.extend_from_slice(&0i32.to_le_bytes()); // MetadataSize
        buf.extend_from_slice(&0i32.to_le_bytes()); // AttachmentSize
        buf.extend_from_slice(&(*size as i64).to_le_bytes()); // DataSize
        buf.extend_from_slice(&directory_entry(tile, 0));
        buf.resize(data_start + SUBBLOCK_FIXED, 0);
        match &tile.pixels {
            Some(pixels) => {
                assert_eq!(
                    pixels.len(),
                    *size,
                    "the tile's stated pixels must fill the tile exactly"
                );
                buf.extend_from_slice(pixels);
            }
            None => buf.extend(std::iter::repeat_n(0x5au8, *size)),
        }
    }

    // The `ZISRAWMETADATA` segment: the XML starts 256 bytes into the data.
    buf.extend_from_slice(&segment_header(
        b"ZISRAWMETADATA\0\0",
        (256 + xml.len()) as i64,
    ));
    buf.extend_from_slice(&(xml.len() as i32).to_le_bytes());
    buf.extend_from_slice(&0i32.to_le_bytes());
    buf.resize(buf.len() + 248, 0);
    buf.extend_from_slice(xml);

    std::fs::write(path, &buf).expect("write the synthetic CZI");
}

/// A scratch file of this test run.
pub fn scratch(name: &str) -> Scratch {
    Scratch::new(
        std::env::temp_dir().join(format!("phasit_czi_refusal_{name}_{}", std::process::id())),
    )
}
