// SPDX-License-Identifier: Apache-2.0
//! The fill of the parts a stored level does not cover: the level-0 pixels they are made of.

mod real_data;
mod support;

use phasit_imread::czi_reader::CziReader;
use phasit_imread::types::PixelRect;
use phasit_imread::ImageReader;

use support::czi::{full_resolution_tiles, level_tile, plane_bytes, scratch, Tile};
use support::czi_write::write_czi;

/// A hole of the level over a gap of level 0 reads blank: level 0 has no pixel there either.
#[test]
fn a_level_hole_over_a_level_zero_gap_reads_blank() {
    let path = scratch("stored_hole_over_gap");
    // The scene misses the cell at [64, 128), and the level misses the same cell: the hole of the
    // level has no level-0 pixel under it.
    let mut tiles = full_resolution_tiles(3, 1, 64, 0, 0);
    tiles.remove(1);
    tiles.push(level_tile(0, 2, 0, 0, 64, 64, 0));
    tiles.push(level_tile(0, 2, 128, 0, 64, 64, 0));
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    let rect = PixelRect::new(0, 0, 192, 64);
    let plane = stored
        .region(2, 0, 0, 0, rect)
        .expect("the level reads over its hole");
    assert_eq!(plane.plane.shape, [32, 96], "the level's pixels over the rect");
    let bytes = plane_bytes(&plane.plane);
    let row = &bytes[..96];
    assert!(
        row[..32].iter().any(|&byte| byte != 0),
        "the level's own pixels are there"
    );
    assert!(
        row[32..64].iter().all(|&byte| byte == 0),
        "the hole over the level-0 gap is blank"
    );
    assert!(
        row[64..].iter().any(|&byte| byte != 0),
        "the level's own pixels are behind the hole too"
    );
}

/// The stored tile at `(x, y)` painted with `pixels`, of the given stored size.
fn painted_level(mut tile: Tile, pixels: Vec<u8>, stored: (u32, u32)) -> Tile {
    tile = tile.painted(pixels);
    tile.stored = Some(stored);
    tile.pyramid_type = 2;
    tile
}

/// The step scene: the source is black above row 3 and light gray from it, so a level pixel a
/// band edge cuts in half has a well-defined box average (3 black rows and one light one).
fn step_scene() -> Vec<Tile> {
    let mut tiles = Vec::new();
    for col in 0..3i32 {
        let mut pixels = Vec::with_capacity(64 * 64);
        for row in 0..64u32 {
            for _ in 0..64 {
                pixels.push(if row < 3 { 0u8 } else { 200 });
            }
        }
        tiles.push(Tile::full(0, 0, 64, 64).at(col * 64, 0).painted(pixels));
    }
    tiles
}

/// A level pixel a coverage band edge falls inside is the box average over ALL the source pixels
/// it leaves uncovered, whichever band each source pixel falls in. The x4 level here holds a
/// whole tile over the first cell and one whose top edge (row 3) splits the first level pixel
/// row of the gap between the two: 3 black rows and one light one make 50, not either slice.
#[test]
fn a_level_pixel_split_by_a_band_edge_is_the_box_average_of_its_uncovered_area() {
    let path = scratch("stored_band_edge_pixel");
    let mut tiles = step_scene();
    let mut first_row = vec![50u8; 16];
    first_row.extend(vec![200u8; 16 * 15]);
    tiles.push(painted_level(Tile::full(0, 0, 64, 64).at(0, 0), first_row, (16, 16)));
    tiles.push(painted_level(
        Tile::full(0, 0, 64, 60).at(128, 3),
        vec![200u8; 16 * 15],
        (16, 15),
    ));
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    let plane = stored
        .region(4, 0, 0, 0, PixelRect::new(0, 0, 192, 64))
        .expect("the level reads over the band edge");
    assert_eq!(plane.plane.shape, [16, 48], "the level's pixels over the scene");
    let bytes = plane_bytes(&plane.plane);
    let pixel = |col: usize| bytes[col];
    for col in 16..32 {
        assert_eq!(
            pixel(col), 50,
            "level pixel (0,{col}): the box average of its uncovered area"
        );
    }
    assert_eq!(pixel(0), 50, "the first tile's own first pixel");
    // The second tile's top edge rounds down one level pixel, so the level pixel above it is the
    // fill of its uncovered black rows; the tile's own first pixel lands one row down.
    assert_eq!(pixel(32), 0, "the fill over the rows above the second tile");
    assert_eq!(bytes[48 + 32], 200, "the second tile's own first pixel, one row down");
}

/// A hole level pixel of the owner's x128 level is cut in half by a coverage band edge (source
/// row 55361): it is the box average of the full resolution under it, not a half's average.
#[cfg(feature = "jpegxr")]
#[test]
fn the_real_whole_slide_fills_a_band_split_pixel_with_the_box_average() {
    let Some(path) = real_data::real_file("czi/brightfield-jxr.czi") else {
        return;
    };

    let reader = CziReader::open(&path).expect("the Axioscan CZI opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");

    // One whole level pixel of the x128 lattice (origin x -66, y -127): (62..190, 55297..55425).
    let rect = PixelRect::new(62, 55297, 128, 128);
    let plane = stored
        .region(128, 0, 0, 0, rect)
        .expect("the level pixel reads");
    assert_eq!(plane.plane.shape, [1, 1], "the rect is one whole level pixel");
    let read = plane_bytes(&plane.plane);
    let full = reader.region(0, 0, 0, 0, rect).expect("the full resolution reads");
    let source = plane_bytes(&full);
    for channel in 0..3 {
        let mut sum = 0u64;
        for index in (channel..source.len()).step_by(3) {
            sum += u64::from(source[index]);
        }
        let count = (source.len() / 3) as u64;
        let average = ((sum + count / 2) / count) as u8;
        assert_eq!(
            read[channel], average,
            "channel {channel}: the filled pixel is the box average of the full resolution"
        );
    }
}

/// A rect that reaches past the scene reads the scene's part of itself: the level pixels beyond
/// the scene would cost a plane of their own and hold no pixel of the scene.
#[test]
fn a_rect_beyond_the_scene_reads_the_scene_part_of_itself() {
    let path = scratch("stored_fill_cost");
    let mut tiles = full_resolution_tiles(3, 1, 256, 0, 0);
    for x in [0, 256, 512] {
        tiles.push(level_tile(0, 256, x, 0, 256, 256, 0));
    }
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    let rect = PixelRect::new(0, 0, 100_096, 50_176);
    let plane = stored
        .region(256, 0, 0, 0, rect)
        .expect("the hole beyond the scene costs nothing");
    assert_eq!(plane.plane.shape, [1, 3], "the scene's part of the rect");
    let bytes = plane_bytes(&plane.plane);
    assert!(bytes[0] > 0, "the level's own pixels are there");
}

/// A hole cell reads the same value through the whole scene's window and through a 12-px window
/// around it: a pixel's average is over its uncovered area, not over the window it came from.
///
/// WHY two diagonal tiles: the level's hole then splits into two parts, one up and left of the
/// other's pixels, so a part can miss a pixel on both axes inside one fill block.
#[test]
fn a_hole_cell_reads_the_same_whatever_window_it_is_read_from() {
    let path = scratch("stored_window_independent_fill");
    let mut tiles = full_resolution_tiles(4, 4, 64, 0, 0);
    tiles.push(level_tile(0, 2, 0, 0, 128, 128, 0));
    tiles.push(level_tile(0, 2, 128, 128, 128, 128, 0));
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    let big = stored
        .region(2, 0, 0, 0, PixelRect::new(0, 0, 256, 256))
        .expect("the whole level reads");
    let small = stored
        .region(2, 0, 0, 0, PixelRect::new(40, 160, 12, 12))
        .expect("the cell's own window reads");
    let big_bytes = plane_bytes(&big.plane);
    let small_bytes = plane_bytes(&small.plane);
    let at = 80 * 128 + 20;
    assert_eq!(big.origin, [0, 0], "the whole level reads from its own corner");
    assert_eq!(small.origin, [40, 160], "the small window snaps onto the cell");
    assert_eq!(
        big_bytes[at], small_bytes[0],
        "the cell at level (80, 20) reads the same through both windows"
    );
    assert_eq!(
        big_bytes[at], 201,
        "the cell is the box average of its four source pixels"
    );
}


/// A hole cell the caller's rect cuts reads the same as through a window that holds it whole:
/// the read snaps outward to the whole cell, and the cell's average is over all of it.
#[test]
fn a_hole_cell_the_rect_cuts_is_averaged_over_the_whole_cell() {
    let path = scratch("stored_cut_cell_fill");
    let mut tiles = full_resolution_tiles(4, 4, 64, 0, 0);
    tiles.push(level_tile(0, 2, 0, 0, 128, 128, 0));
    tiles.push(level_tile(0, 2, 128, 128, 128, 128, 0));
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    let whole = stored
        .region(2, 0, 0, 0, PixelRect::new(40, 160, 12, 12))
        .expect("the cell's own window reads");
    let cut = stored
        .region(2, 0, 0, 0, PixelRect::new(41, 161, 11, 11))
        .expect("a window that starts inside the cell reads");
    assert_eq!(cut.origin, [40, 160], "the cut window snaps outward onto the cell");
    assert_eq!(
        plane_bytes(&cut.plane)[0],
        plane_bytes(&whole.plane)[0],
        "the cell at level (80, 20) is the average of its four source pixels, not of the one the rect holds"
    );
}
