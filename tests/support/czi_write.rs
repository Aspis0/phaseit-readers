// SPDX-License-Identifier: Apache-2.0
//! The CZI files the tests write: the file header, the directory, the subblocks, the metadata.

use std::path::Path;

use super::czi::Tile;

/// The length of a segment header: the id, the allocated size and the used size.
const SEGMENT_HEADER: usize = 32;

/// The length of the fixed part of a subblock.
const SUBBLOCK_FIXED: usize = 256;

/// The length of the file header's data.
const FILE_HEADER_DATA: usize = 128;

/// The length of a directory entry: the fixed part and the six dimensions.
const ENTRY_LEN: usize = 32 + 6 * 20;

/// The offset of a subblock's DataSize, from the start of its segment: the segment header, the
/// metadata size and the attachment size.
const DATA_SIZE_OFFSET: usize = 40;

/// Writes a CZI file whose subblocks hold the given tiles.
pub fn write_czi(path: &Path, tiles: &[Tile]) {
    let dir_pos = FILE_HEADER_DATA + SEGMENT_HEADER;
    let mut subblock_pos = dir_pos + SEGMENT_HEADER + directory_size(tiles);
    let mut positions = Vec::with_capacity(tiles.len());
    for tile in tiles {
        positions.push(subblock_pos as i64);
        subblock_pos += SEGMENT_HEADER + SUBBLOCK_FIXED + payload_size(tile);
    }
    let meta_pos = subblock_pos;

    let mut buf = Vec::new();
    write_header(&mut buf, dir_pos as i64, meta_pos as i64);
    write_directory(&mut buf, tiles, &positions);
    for tile in tiles {
        write_subblock(&mut buf, tile);
    }
    write_metadata(&mut buf);
    std::fs::write(path, &buf).expect("write the synthetic CZI");
}

/// The file header: the positions of the directory and of the metadata.
fn write_header(buf: &mut Vec<u8>, dir_pos: i64, meta_pos: i64) {
    buf.extend_from_slice(&segment_header(
        b"ZISRAWFILE\0\0\0\0\0\0",
        FILE_HEADER_DATA as i64,
    ));
    buf.extend_from_slice(&[0u8; FILE_HEADER_DATA]);
    let at = buf.len() - FILE_HEADER_DATA + 52;
    buf[at..at + 8].copy_from_slice(&dir_pos.to_le_bytes());
    buf[at + 8..at + 16].copy_from_slice(&meta_pos.to_le_bytes());
}

/// The directory: one entry per tile.
fn write_directory(buf: &mut Vec<u8>, tiles: &[Tile], positions: &[i64]) {
    buf.extend_from_slice(&segment_header(
        b"ZISRAWDIRECTORY\0",
        directory_size(tiles) as i64,
    ));
    buf.extend_from_slice(&(tiles.len() as i32).to_le_bytes());
    buf.extend_from_slice(&[0u8; 124]);
    for (tile, position) in tiles.iter().zip(positions) {
        buf.extend_from_slice(&directory_entry(tile, *position));
    }
}

/// One subblock: its header, its directory entry and its pixels.
fn write_subblock(buf: &mut Vec<u8>, tile: &Tile) {
    let size = payload_size(tile);
    buf.extend_from_slice(&segment_header(
        b"ZISRAWSUBBLOCK\0\0",
        (SUBBLOCK_FIXED + size) as i64,
    ));
    let data_start = buf.len();
    buf.extend_from_slice(&0i32.to_le_bytes()); // MetadataSize
    buf.extend_from_slice(&0i32.to_le_bytes()); // AttachmentSize
    buf.extend_from_slice(&(size as i64).to_le_bytes()); // DataSize
    buf.extend_from_slice(&directory_entry(tile, 0));
    buf.resize(data_start + SUBBLOCK_FIXED, 0);
    match &tile.pixels {
        Some(pixels) => buf.extend_from_slice(pixels),
        None => buf.extend(std::iter::repeat_n(0x5au8, size)),
    }
}

/// The metadata segment: the XML the reader reads for the channel names.
fn write_metadata(buf: &mut Vec<u8>) {
    let xml = b"<Metadata><Data/></Metadata>";
    buf.extend_from_slice(&segment_header(
        b"ZISRAWMETADATA\0\0",
        (256 + xml.len()) as i64,
    ));
    buf.extend_from_slice(&(xml.len() as i32).to_le_bytes());
    buf.extend_from_slice(&0i32.to_le_bytes());
    buf.resize(buf.len() + 248, 0);
    buf.extend_from_slice(xml);
}

/// The bytes the directory holds: the entry count, the reserved bytes and the entries.
fn directory_size(tiles: &[Tile]) -> usize {
    4 + 124 + tiles.len() * ENTRY_LEN
}

/// The bytes a tile's subblock holds: its painted pixels, or zeros of its stored size.
///
/// WHY the painted length: a compressed tile carries fewer bytes than its pixels.
fn payload_size(tile: &Tile) -> usize {
    let pixels = tile.stored_x() as usize
        * tile.stored_y() as usize
        * tile.samples as usize
        * item_size(tile.pixel_type);
    match &tile.pixels {
        Some(painted) => {
            assert!(
                tile.compression != 0 || painted.len() == pixels,
                "an uncompressed tile's pixels must fill the tile exactly"
            );
            painted.len()
        }
        None => pixels,
    }
}

/// The bytes of one sample of a pixel type.
fn item_size(pixel_type: i32) -> usize {
    match pixel_type {
        1 | 4 => 2, // Gray16, Bgr48
        _ => 1,     // Gray8, Bgr24, and the types the reader does not model
    }
}

/// A segment header: the 16-byte id, then the allocated and used size (the same: the segment holds what it declares).
fn segment_header(id: &[u8; 16], allocated: i64) -> Vec<u8> {
    let mut out = Vec::with_capacity(SEGMENT_HEADER);
    out.extend_from_slice(id);
    out.extend_from_slice(&allocated.to_le_bytes());
    out.extend_from_slice(&allocated.to_le_bytes());
    out
}

/// One dimension entry: its letter, its start, its size and its stored size.
fn dimension(out: &mut Vec<u8>, name: &[u8; 4], start: i32, size: i32, stored: i32) {
    out.extend_from_slice(name);
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&0f32.to_le_bytes());
    out.extend_from_slice(&stored.to_le_bytes());
}

/// The directory entry of a tile: its pixel type, its position and its dimensions.
fn directory_entry(tile: &Tile, file_position: i64) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0u8; 2]); // SchemaType
    out.extend_from_slice(&tile.pixel_type.to_le_bytes());
    out.extend_from_slice(&file_position.to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes()); // FilePart
    out.extend_from_slice(&tile.compression.to_le_bytes()); // Compression
    out.push(tile.pyramid_type); // PyramidType
    out.push(0); // spare
    out.extend_from_slice(&[0u8; 4]); // spare
    out.extend_from_slice(&6i32.to_le_bytes()); // DimensionCount: X, Y, S, C, Z, T
    dimension(
        &mut out,
        b"X\0\0\0",
        tile.x,
        tile.width as i32,
        tile.stored_x() as i32,
    );
    dimension(
        &mut out,
        b"Y\0\0\0",
        tile.y,
        tile.height as i32,
        tile.stored_y() as i32,
    );
    dimension(&mut out, b"S\0\0\0", tile.scene, 1, 1);
    dimension(&mut out, b"C\0\0\0", tile.c, 1, 1);
    dimension(&mut out, b"Z\0\0\0", 0, 1, 1);
    dimension(&mut out, b"T\0\0\0", 0, 1, 1);
    out
}

/// Makes the last subblock of a file claim a data size that runs past the end of the file.
pub fn break_last_span(path: &Path) {
    let mut bytes = std::fs::read(path).expect("read the synthetic CZI");
    let at = bytes
        .windows(14)
        .rposition(|window| window == b"ZISRAWSUBBLOCK")
        .expect("the file holds a subblock");
    let size_at = at + DATA_SIZE_OFFSET;
    bytes[size_at..size_at + 8].copy_from_slice(&u64::MAX.to_le_bytes());
    std::fs::write(path, &bytes).expect("write the synthetic CZI");
}
