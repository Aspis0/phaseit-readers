// SPDX-License-Identifier: Apache-2.0
//! The reads over a level a CZI stores itself: the rect a read names, the whole level pixels it
//! snaps to, and the rects it refuses.

mod support;

use phasit_imread::czi_reader::CziReader;
use phasit_imread::types::PixelRect;
use phasit_imread::ImageReader;

use support::czi::{downsample, full_resolution, level_tile, plane_bytes, scratch, scene, whole_level};
use support::czi_write::write_czi;

/// The rect of a level read names source pixels: the read covers the rect's intersection with the
/// scene in whole level pixels, so a rect off the level's lattice still reads the level, aligned.
#[test]
fn a_rect_off_the_level_lattice_reads_whole_level_pixels() {
    let path = scratch("stored_off_lattice");
    let mut tiles = scene();
    whole_level(&mut tiles);
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    let level = stored
        .region(2, 0, 0, 0, PixelRect::new(1, 0, 128, 64))
        .expect("the read covers whole level pixels");
    assert_eq!(level.plane.shape, [32, 65], "the level pixels over [0, 130)");
    let expected = downsample(&full_resolution(&reader, PixelRect::new(0, 0, 130, 64)), 130, 64, 1, 2);
    for (index, (got, want)) in plane_bytes(&level.plane).iter().zip(&expected).enumerate() {
        assert!(
            got.abs_diff(*want) <= 1,
            "pixel {index}: the level reads {got}, the downsample reads {want}"
        );
    }
}

#[test]
fn a_rect_that_is_not_a_multiple_of_the_factor_reads_whole_level_pixels() {
    let path = scratch("stored_not_a_multiple");
    let mut tiles = scene();
    whole_level(&mut tiles);
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    let level = stored
        .region(2, 0, 0, 0, PixelRect::new(0, 0, 127, 64))
        .expect("the read covers whole level pixels");
    assert_eq!(level.plane.shape, [32, 64], "the level pixels over [0, 128)");
    let expected = downsample(&full_resolution(&reader, PixelRect::new(0, 0, 128, 64)), 128, 64, 1, 2);
    for (index, (got, want)) in plane_bytes(&level.plane).iter().zip(&expected).enumerate() {
        assert!(
            got.abs_diff(*want) <= 1,
            "pixel {index}: the level reads {got}, the downsample reads {want}"
        );
    }
}

/// A read runs on the level's lattice, so its plane may start before the rect: the read names
/// where its first pixel sits, in source pixels. The grid here starts at (33, 1), so the
/// whole-scene read starts a level pixel early on both axes.
#[test]
fn a_read_names_where_its_plane_starts() {
    let path = scratch("stored_plane_origin");
    let mut tiles = scene();
    tiles.push(level_tile(0, 2, 33, 1, 64, 64, 0));
    tiles.push(level_tile(0, 2, 97, 1, 64, 64, 0));
    tiles.push(level_tile(0, 2, 161, 1, 64, 64, 0));
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    let read = stored
        .region(2, 0, 0, 0, PixelRect::new(0, 0, 192, 64))
        .expect("the whole scene reads");
    assert_eq!(read.plane.shape, [33, 97], "the scene in whole level pixels");
    assert_eq!(read.origin, [-1, -1], "the plane's first pixel's corner");
}

#[test]
fn a_rect_without_area_is_refused() {
    let path = scratch("stored_no_area");
    let mut tiles = scene();
    whole_level(&mut tiles);
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    let error = stored
        .region(2, 0, 0, 0, PixelRect::new(0, 0, 0, 64))
        .expect_err("the rect has no area");
    assert!(
        error.to_string().contains("empty"),
        "the error names the empty rect: {error}"
    );
}

#[test]
fn a_rect_that_misses_the_scene_is_refused() {
    let path = scratch("stored_misses_scene");
    let mut tiles = scene();
    whole_level(&mut tiles);
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    let error = stored
        .region(2, 0, 0, 0, PixelRect::new(1000, 0, 64, 64))
        .expect_err("the rect lies wholly outside the scene");
    assert!(
        error.to_string().contains("scene"),
        "the error names the scene: {error}"
    );
}

/// A rect whose edges overflow is refused by name, never a panic: `region` is a public API and
/// the rest of the reader refuses such rects with a checked error.
#[test]
fn a_rect_whose_edges_overflow_is_refused() {
    let path = scratch("stored_overflow_rect");
    let mut tiles = scene();
    whole_level(&mut tiles);
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    let error = stored
        .region(2, 0, 0, 0, PixelRect::new(usize::MAX - 2, 0, 10, 10))
        .expect_err("the rect's right edge overflows usize");
    assert!(
        error.to_string().contains("overflow"),
        "the error names the overflow: {error}"
    );
}

#[test]
fn a_level_that_the_file_does_not_store_is_not_read() {
    let path = scratch("stored_absent_factor");
    let mut tiles = scene();
    whole_level(&mut tiles);
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    let error = stored
        .region(4, 0, 0, 0, PixelRect::new(0, 0, 192, 64))
        .expect_err("the file stores no x4 level");
    assert!(
        error.to_string().contains('4'),
        "the error names the minification: {error}"
    );
}
