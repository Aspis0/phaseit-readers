// SPDX-License-Identifier: Apache-2.0
//! The pixels a stored level reads: against the full resolution's box-downsample, and over the
//! parts of the scene the level's tiles do not cover - the margin its grid leaves and the holes
//! between its tiles. They hold the full resolution's box average, and are blank where it has no
//! pixel.

mod support;

use phasit_imread::czi_reader::CziReader;
use phasit_imread::types::PixelRect;
use phasit_imread::ImageReader;

use support::czi::{
    downsample, full_resolution, full_resolution_tiles, level_tile, plane_bytes, scratch, scene,
    source_pixels, whole_level, Tile,
};
use support::czi_write::write_czi;

#[test]
fn a_stored_level_reads_the_pixels_of_the_full_resolution_downsampled() {
    let path = scratch("stored_pixels");
    let mut tiles = scene();
    whole_level(&mut tiles);
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    let rect = PixelRect::new(0, 0, 192, 64);
    let level = stored.region(2, 0, 0, 0, rect).expect("the level reads");
    assert_eq!(level.plane.shape, [32, 96], "the level's pixels over the rect");

    // The level's pixels are the full resolution's, box-downsampled: the tolerance is the
    // rounding of the average.
    let expected = downsample(&full_resolution(&reader, rect), 192, 64, 1, 2);
    let read = plane_bytes(&level.plane);
    for (index, (got, want)) in read.iter().zip(&expected).enumerate() {
        assert!(
            got.abs_diff(*want) <= 1,
            "pixel {index}: the level reads {got}, the downsample reads {want}"
        );
    }
}

#[test]
fn a_stored_level_of_a_color_scene_reads_its_own_colors() {
    let path = scratch("stored_color");
    let mut tiles = full_resolution_tiles(2, 1, 64, 3, 0);
    tiles.push(level_tile(3, 2, 0, 0, 64, 64, 0));
    tiles.push(level_tile(3, 2, 64, 0, 64, 64, 0));
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    let rect = PixelRect::new(0, 0, 128, 64);
    let level = stored.region(2, 0, 0, 0, rect).expect("the level reads");
    assert_eq!(level.plane.samples, 3, "the level carries the scene's samples");

    let expected = downsample(&full_resolution(&reader, rect), 128, 64, 3, 2);
    for (index, (got, want)) in plane_bytes(&level.plane).iter().zip(&expected).enumerate() {
        assert!(
            got.abs_diff(*want) <= 1,
            "sample {index}: the level reads {got}, the downsample reads {want}"
        );
    }
}

/// The level's pixels over the whole scene, against the full resolution's box-downsampled.
fn assert_level_matches_the_full_resolution(reader: &CziReader, factor: usize, rect: PixelRect) {
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    let level = stored
        .region(factor, 0, 0, 0, rect)
        .expect("the level reads");
    let full = reader
        .region(0, 0, 0, 0, rect)
        .expect("the full resolution reads");
    let expected = downsample(
        plane_bytes(&full),
        rect.w as u32,
        rect.h as u32,
        full.samples,
        factor as u32,
    );
    let read = plane_bytes(&level.plane);
    assert_eq!(
        read.len(),
        expected.len(),
        "the level's pixels over the rect"
    );
    for (index, (got, want)) in read.iter().zip(&expected).enumerate() {
        assert!(
            got.abs_diff(*want) <= 1,
            "pixel {index}: the level reads {got}, the downsample reads {want}"
        );
    }
}

#[test]
fn the_margin_the_grid_leaves_is_filled_from_the_full_resolution() {
    let path = scratch("stored_margin");
    // A scene of two cells; the level's grid starts half a cell in, so the left band is uncovered.
    let mut tiles: Vec<Tile> = full_resolution_tiles(2, 1, 64, 0, 0);
    tiles.push(level_tile(0, 2, 32, 0, 64, 64, 0));
    tiles.push(level_tile(0, 2, 96, 0, 32, 64, 0));
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    assert_eq!(stored.levels().len(), 1, "the level is offered");

    assert_level_matches_the_full_resolution(&reader, 2, PixelRect::new(0, 0, 128, 64));
}

#[test]
fn a_hole_between_the_tiles_of_a_level_is_filled_from_the_full_resolution() {
    let path = scratch("stored_hole");
    // A scene of three cells; the level holds the first and the last, the middle one is missing.
    let mut tiles: Vec<Tile> = full_resolution_tiles(3, 1, 64, 0, 0);
    tiles.push(level_tile(0, 2, 0, 0, 64, 64, 0));
    tiles.push(level_tile(0, 2, 128, 0, 64, 64, 0));
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    assert_eq!(stored.levels().len(), 1, "the level is offered");

    assert_level_matches_the_full_resolution(&reader, 2, PixelRect::new(0, 0, 192, 64));
}

/// The scene's first row and column sit in partial level pixels when the level's grid carries a
/// phase: the level image of the whole scene names them, and they read like every other pixel.
#[test]
fn the_level_image_of_the_whole_scene_names_the_first_partial_pixel() {
    let path = scratch("stored_phase");
    // The scene is three cells of 64 pixels; the level's grid starts at (33, 1), so its first
    // pixel column and row hold only part of the scene.
    let mut tiles: Vec<Tile> = full_resolution_tiles(3, 1, 64, 0, 0);
    tiles.push(level_tile(0, 2, 33, 1, 64, 64, 0));
    tiles.push(level_tile(0, 2, 97, 1, 64, 64, 0));
    tiles.push(level_tile(0, 2, 161, 1, 64, 64, 0));
    let stored_pixels: Vec<Vec<u8>> = tiles[3..]
        .iter()
        .map(|tile| tile.pixels.clone().expect("the level tiles are painted"))
        .collect();
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    let plane = stored
        .region(2, 0, 0, 0, PixelRect::new(0, 0, 192, 64))
        .expect("the level image of the whole scene reads");
    assert_eq!(plane.plane.shape, [33, 97], "the whole scene, in whole level pixels");
    let read = plane_bytes(&plane.plane);

    // The level image the read must return: a tile's own pixel where a tile covers the level
    // pixel, the box average of the scene's pixels under it where none does, blank beyond the
    // scene.
    let source = source_pixels(0, 0, 192, 64, 1);
    let pixel = |col: usize, row: usize| -> u8 {
        let (x0, y0) = (-1 + 2 * col as i64, -1 + 2 * row as i64);
        if (17..113).contains(&col) && (1..33).contains(&row) {
            let tile = &stored_pixels[(col - 17) / 32];
            return tile[(row - 1) * 32 + (col - 17) % 32];
        }
        let (mut sum, mut count) = (0u32, 0u32);
        for y in y0.max(0)..(y0 + 2).min(64) {
            for x in x0.max(0)..(x0 + 2).min(192) {
                sum += u32::from(source[(y * 192 + x) as usize]);
                count += 1;
            }
        }
        // No pixel of the scene under the level pixel: blank.
        (sum + count / 2).checked_div(count).unwrap_or(0) as u8
    };
    for row in 0..33 {
        for col in 0..97 {
            assert_eq!(
                read[row * 97 + col],
                pixel(col, row),
                "pixel ({col},{row})"
            );
        }
    }
}

#[test]
fn the_filled_margin_is_read_at_the_level_of_the_rect() {
    let path = scratch("stored_margin_window");
    let mut tiles: Vec<Tile> = full_resolution_tiles(2, 1, 64, 0, 0);
    tiles.push(level_tile(0, 2, 32, 0, 64, 64, 0));
    tiles.push(level_tile(0, 2, 96, 0, 32, 64, 0));
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    // A rect that starts inside the band: the fill covers its left part only.
    let level = stored
        .region(2, 0, 0, 0, PixelRect::new(16, 0, 64, 64))
        .expect("the level reads");
    assert_eq!(level.plane.shape, [32, 32], "the level's pixels over the rect");

    let full = reader
        .region(0, 0, 0, 0, PixelRect::new(16, 0, 64, 64))
        .expect("the full resolution reads");
    let expected = downsample(plane_bytes(&full), 64, 64, 1, 2);
    for (index, (got, want)) in plane_bytes(&level.plane).iter().zip(&expected).enumerate() {
        assert!(
            got.abs_diff(*want) <= 1,
            "pixel {index}: the level reads {got}, the downsample reads {want}"
        );
    }
}
