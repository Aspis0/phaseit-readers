// SPDX-License-Identifier: Apache-2.0
//! The file's own pyramid: the coarse levels a CZI stores itself, and the reads over them.
//!
//! A coarse subblock is one whose stored size differs from its logical size - libCZI's own rule.
//! The `PyramidType` byte is legacy and is not the discriminator: a full-resolution tile that
//! carries it still reads as it did, and a coarse tile without it is still a level.

mod real_data;
mod support;

use phasit_imread::czi_reader::CziReader;
use phasit_imread::types::PixelRect;
#[cfg(feature = "jpegxr")]
use phasit_imread::types::StoredLevel;
use phasit_imread::ImageReader;

use support::czi::{
    full_resolution, full_resolution_tiles, level_tile, scratch, scene, source_pixels, whole_level,
};
use support::czi_write::write_czi;

#[test]
fn a_file_with_a_stored_pyramid_offers_its_level() {
    let path = scratch("stored_level");
    let mut tiles = scene();
    whole_level(&mut tiles);
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    let levels = stored.levels();
    assert_eq!(levels.len(), 1, "the file carries one coarse level");
    assert_eq!(levels[0].factor, 2, "the level is half the resolution");
}

/// The window of a read: only the tiles under the rect are decoded.
///
/// WHY a zstd payload and not an undecodable compression: a tile this build cannot decode refuses
/// its level, so the level would never be offered and the window would not be read at all.
#[test]
fn a_stored_level_decodes_only_the_tiles_under_the_rect() {
    let path = scratch("stored_window");
    let mut tiles = scene();
    whole_level(&mut tiles);
    // The last tile of the level carries a payload no zstd decoder accepts.
    let mut broken = level_tile(0, 2, 128, 0, 64, 64, 0);
    broken.compression = 5;
    broken = broken.painted(b"not a zstd frame".to_vec());
    tiles.pop();
    tiles.push(broken);
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    let first = stored
        .region(2, 0, 0, 0, PixelRect::new(0, 0, 128, 64))
        .expect("a read that misses the broken tile does not decode it");
    assert_eq!(first.plane.shape, [32, 64], "the level's pixels over the rect");

    // A read that reaches the broken tile fails instead of leaving zeros behind.
    let error = stored
        .region(2, 0, 0, 0, PixelRect::new(0, 0, 192, 64))
        .expect_err("the broken tile fails the read");
    assert!(
        error.to_string().contains("zstd"),
        "the error names the codec: {error}"
    );
}

#[test]
fn a_full_resolution_tile_with_the_legacy_pyramid_byte_reads_as_it_did() {
    let path = scratch("legacy_byte");
    let mut tiles = scene();
    for tile in &mut tiles {
        tile.pyramid_type = 2;
    }
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let scene_meta = reader.meta().scene(0).expect("scene 0");
    assert_eq!(scene_meta.dims.x, 192, "the scene is whole");
    let expected = source_pixels(0, 0, 192, 64, 1);
    assert_eq!(
        full_resolution(&reader, PixelRect::new(0, 0, 192, 64)),
        expected,
        "the scene reads as it did"
    );
    assert!(
        reader.stored_pyramid(0).is_none(),
        "a full-resolution tile is not a coarse level"
    );
}

/// The share of the scene a level's own tiles cover: a level with a hole in it covers less.
#[test]
fn a_level_reports_the_share_of_the_scene_its_own_tiles_cover() {
    let path = scratch("stored_share");
    let mut tiles = scene();
    // The level holds the first and the last cell of the scene: the middle one is a hole.
    tiles.push(level_tile(0, 2, 0, 0, 64, 64, 0));
    tiles.push(level_tile(0, 2, 128, 0, 64, 64, 0));
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    let levels = stored.levels();
    assert_eq!(levels.len(), 1, "the file carries one coarse level");
    assert!(
        (levels[0].coverage_share - 2.0 / 3.0).abs() < 0.001,
        "the level covers two thirds of the scene: {}",
        levels[0].coverage_share
    );
}

/// The share is the worst plane's: two channels whose level tiles cover disjoint halves leave
/// every read of a channel half filled, whatever the union of the tiles covers.
#[test]
fn a_level_reports_the_share_of_the_plane_its_tiles_cover_worst() {
    let path = scratch("stored_share_halves");
    let mut tiles = full_resolution_tiles(2, 1, 64, 0, 0);
    tiles.extend(full_resolution_tiles(2, 1, 64, 0, 1));
    // Channel 0's level tile covers the first cell, channel 1's the second.
    tiles.push(level_tile(0, 2, 0, 0, 64, 64, 0));
    tiles.push(level_tile(0, 2, 64, 0, 64, 64, 1));
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    let levels = stored.levels();
    assert_eq!(levels.len(), 1, "the file carries one coarse level");
    assert!(
        (levels[0].coverage_share - 0.5).abs() < 0.001,
        "each plane of the level covers half the scene: {}",
        levels[0].coverage_share
    );
}

#[test]
fn a_file_without_a_stored_pyramid_offers_none() {
    let path = scratch("no_stored");
    write_czi(&path, &scene());

    let reader = CziReader::open(&path).expect("the file opens");
    assert!(
        reader.stored_pyramid(0).is_none(),
        "a file that stores no pyramid offers none"
    );
}

/// The two files that store no pyramid of their own keep today's path: they offer none, and
/// their pixels are the ones the reader read before.
#[test]
fn the_real_files_without_a_stored_pyramid_offer_none() {
    for name in ["czi/mosaic.czi", "czi/stitched.czi"] {
        let Some(path) = real_data::real_file(name) else {
            return;
        };
        let reader = CziReader::open(&path).expect("the file opens");
        assert!(
            reader.stored_pyramid(0).is_none(),
            "{name} stores no pyramid of its own"
        );
        let scene = reader.meta().scene(0).expect("scene 0");
        let rect = PixelRect::new(0, 0, 64.min(scene.dims.x), 64.min(scene.dims.y));
        let plane = reader
            .region(0, 0, 0, 0, rect)
            .expect("the scene reads as it did");
        assert_eq!(plane.shape, [rect.h, rect.w], "the scene's shape");
    }
}

/// The whole slide stores its levels as JPEG-XR, so they are offered only where they can be
/// decoded.
#[cfg(feature = "jpegxr")]
#[test]
fn the_real_whole_slide_offers_the_levels_of_its_own_pyramid() {
    let Some(path) = real_data::real_file("czi/brightfield-jxr.czi") else {
        return;
    };

    let reader = CziReader::open(&path).expect("the Axioscan CZI opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    let factors: Vec<usize> = stored.levels().iter().map(|level| level.factor).collect();
    assert_eq!(
        factors,
        vec![2, 4, 8, 16, 32, 64, 128, 256],
        "the levels the file stores itself"
    );
    for level in stored.levels() {
        eprintln!(
            "level x{}: cell {:?}, origin {:?}, extent {:?}",
            level.factor, level.cell, level.origin, level.extent
        );
    }
    for refused in stored.refused_levels() {
        eprintln!("level x{} refused: {}", refused.factor, refused.reason);
    }
}

/// The level image of the whole scene reads at every level whose plane the test can hold: its
/// first pixel column and row are the partial ones that contain the scene's first source row and
/// the left band.
#[cfg(feature = "jpegxr")]
#[test]
fn the_real_whole_slide_reads_the_whole_scene_at_its_coarse_levels() {
    let Some(path) = real_data::real_file("czi/brightfield-jxr.czi") else {
        return;
    };

    let reader = CziReader::open(&path).expect("the Axioscan CZI opens");
    let scene = reader.meta().scene(0).expect("scene 0");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    for level in stored.levels() {
        // The x8 whole scene is 1.4 GB; from x16 up the planes are affordable (174 MB, 43 MB, ...).
        if level.factor < 16 {
            continue;
        }
        let plane = stored
            .region(
                level.factor,
                0,
                0,
                0,
                PixelRect::new(0, 0, scene.dims.x, scene.dims.y),
            )
            .expect("the level image of the whole scene reads");
        assert_eq!(
            plane.plane.shape,
            whole_scene_shape(level, scene.dims.x, scene.dims.y),
            "level x{}",
            level.factor
        );
    }
}

/// The shape of the level image of the whole scene: the scene, in whole level pixels of the
/// level's lattice.
#[cfg(feature = "jpegxr")]
fn whole_scene_shape(level: &StoredLevel, width: usize, height: usize) -> [usize; 2] {
    let factor = level.factor as i64;
    let x0 = level.origin[0] + (0 - level.origin[0]).div_euclid(factor) * factor;
    let y0 = level.origin[1] + (0 - level.origin[1]).div_euclid(factor) * factor;
    let cols = ((width as i64 - x0) + factor - 1).div_euclid(factor) as usize;
    let rows = ((height as i64 - y0) + factor - 1).div_euclid(factor) as usize;
    [rows, cols]
}
