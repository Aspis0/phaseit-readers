// SPDX-License-Identifier: Apache-2.0
//! The levels a CZI stores itself whose tiles share pixels: two tiles of one plane overlap, and the
//! tiles of two planes may sit at the same place without overlapping.

mod support;

use phasit_imread::czi_reader::CziReader;
use phasit_imread::ImageReader;

use support::czi::{full_resolution_tiles, level_tile, scratch, scene, whole_level};
use support::czi_write::write_czi;

#[test]
fn a_level_with_overlapping_tiles_is_refused() {
    let path = scratch("stored_overlap");
    let mut tiles = scene();
    whole_level(&mut tiles);
    // A tile on top of the one before it.
    tiles.push(level_tile(0, 2, 64, 0, 64, 64, 0));
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    assert!(stored.levels().is_empty(), "the level is not offered");
    let refused = stored.refused_levels();
    assert_eq!(refused.len(), 1, "the level is refused");
    assert!(
        refused[0].reason.contains("overlap"),
        "the reason names the overlap: {}",
        refused[0].reason
    );
}

/// The tiles of two planes may sit at the same place: they are two images of the same scene, so
/// the level is offered and not called an overlap.
#[test]
fn a_level_whose_tiles_of_two_planes_share_a_place_is_offered() {
    let path = scratch("stored_two_planes");
    let mut tiles = full_resolution_tiles(3, 1, 64, 0, 0);
    tiles.extend(full_resolution_tiles(3, 1, 64, 0, 1));
    // The level's tiles of the two channels sit at the same three places of the grid.
    for x in [0, 64, 128] {
        tiles.push(level_tile(0, 2, x, 0, 64, 64, 0));
        tiles.push(level_tile(0, 2, x, 0, 64, 64, 1));
    }
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    assert_eq!(
        reader.meta().scene(0).expect("scene 0").dims.c,
        2,
        "two channels"
    );
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    let levels = stored.levels();
    assert_eq!(
        levels.len(),
        1,
        "the level is offered: {:?}",
        stored.refused_levels()
    );
    assert_eq!(levels[0].factor, 2, "the level's minification");
}
