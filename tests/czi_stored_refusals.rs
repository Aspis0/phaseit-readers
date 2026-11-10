// SPDX-License-Identifier: Apache-2.0
//! The levels a CZI stores itself that the reader refuses: a tile it cannot read, a format the
//! scene does not have, a plane the level leaves out, a grid the tiles do not make.

mod support;

use phasit_imread::czi_reader::CziReader;
use phasit_imread::ImageReader;

use support::czi::{full_resolution_tiles, level_tile, scratch, scene, whole_level};
use support::czi_write::{break_last_span, write_czi};

/// A compression id the reader does not decode: a tile carrying it refuses its level.
const UNDECODABLE: i32 = 99;

#[test]
fn a_level_whose_format_differs_from_the_scene_is_refused() {
    let path = scratch("stored_format");
    let mut tiles = scene();
    // The level is Bgr24 while the scene is Gray8.
    tiles.push(level_tile(3, 2, 0, 0, 64, 64, 0));
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    assert!(stored.levels().is_empty(), "the level is not offered");
    let refused = stored.refused_levels();
    assert_eq!(refused.len(), 1, "the level is refused");
    assert_eq!(refused[0].factor, 2, "the refused level's minification");
    assert!(
        refused[0].reason.contains("format"),
        "the reason names the format: {}",
        refused[0].reason
    );
}

#[test]
fn a_level_with_a_tile_the_reader_cannot_read_is_refused() {
    let path = scratch("stored_unreadable");
    let mut tiles = scene();
    whole_level(&mut tiles);
    // A tile of a pixel type the reader does not model.
    let mut blind = level_tile(0, 2, 64, 0, 64, 64, 0);
    blind.pixel_type = 10;
    tiles.pop();
    tiles.push(blind);
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    assert!(stored.levels().is_empty(), "the level is not offered");
    let refused = stored.refused_levels();
    assert_eq!(refused.len(), 1, "the level is refused");
    assert_eq!(refused[0].skipped, 1, "one tile is behind the refusal");
    assert!(
        refused[0].reason.contains("pixel type"),
        "the reason names the pixel type: {}",
        refused[0].reason
    );
}

#[test]
fn a_level_with_a_tile_whose_stored_size_names_no_minification_is_refused() {
    let path = scratch("stored_unclean_size");
    let mut tiles = scene();
    whole_level(&mut tiles);
    // The last tile of the level stores 33 pixels of a 64 pixel tile: no minification names it.
    let mut broken = level_tile(0, 2, 128, 0, 64, 64, 0);
    broken.stored = Some((33, 33));
    broken = broken.painted(vec![0u8; 33 * 33]);
    tiles.pop();
    tiles.push(broken);
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    assert!(stored.levels().is_empty(), "the level is not offered");
    let refused = stored.refused_levels();
    assert_eq!(refused.len(), 1, "the level is refused");
    assert_eq!(refused[0].factor, 2, "the refused level's minification");
    assert_eq!(refused[0].skipped, 1, "one tile is behind the refusal");
    assert!(
        refused[0].reason.contains("stored size"),
        "the reason names the stored size: {}",
        refused[0].reason
    );
}

/// A tile whose stored size names no minification refuses the level whose cell is its size —
/// also, and especially, when it is the only tile of that level: the level is then a refusal,
/// not a pyramid the file does not carry.
#[test]
fn a_level_whose_only_tile_is_unclean_is_refused() {
    let path = scratch("stored_only_unclean");
    let mut tiles = scene();
    // The level's only tile stores 33 pixels of a 64 pixel tile: no minification names it.
    let mut broken = level_tile(0, 2, 0, 0, 64, 64, 0);
    broken.stored = Some((33, 33));
    broken = broken.painted(vec![0u8; 33 * 33]);
    tiles.push(broken);
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file carries a level, refused or not");
    assert!(stored.levels().is_empty(), "the level is not offered");
    let refused = stored.refused_levels();
    assert_eq!(refused.len(), 1, "the level is refused");
    assert_eq!(refused[0].skipped, 1, "one tile is behind the refusal");
    assert!(
        refused[0].reason.contains("stored size"),
        "the reason names the stored size: {}",
        refused[0].reason
    );
}

/// A level whose every tile is unclean does not take the good level down with it: the good
/// level stays offered, and the wholly bad one is refused by name.
#[test]
fn a_wholly_bad_level_leaves_the_good_one_offered() {
    let path = scratch("stored_good_and_bad");
    let mut tiles = scene();
    whole_level(&mut tiles);
    // A second level's only tile is 128 pixels stored at 33: no minification names it, and its
    // cell is no level's cell.
    let mut bad = level_tile(0, 2, 0, 0, 128, 128, 0);
    bad.stored = Some((33, 33));
    bad = bad.painted(vec![0u8; 33 * 33]);
    tiles.push(bad);
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    let levels = stored.levels();
    assert_eq!(levels.len(), 1, "the good level stays offered");
    assert_eq!(levels[0].factor, 2, "the good level's minification");
    let refused = stored.refused_levels();
    assert_eq!(refused.len(), 1, "the wholly bad level is refused");
    assert_eq!(refused[0].skipped, 1, "one tile is behind the refusal");
    assert!(
        refused[0].reason.contains("stored size"),
        "the reason names the stored size: {}",
        refused[0].reason
    );
}

#[test]
fn a_level_with_a_tile_this_build_cannot_decode_is_refused() {
    for compression in [1, 2, UNDECODABLE] {
        let path = scratch(&format!("stored_undecodable_{compression}"));
        let mut tiles = scene();
        whole_level(&mut tiles);
        let mut broken = level_tile(0, 2, 128, 0, 64, 64, 0);
        broken.compression = compression;
        tiles.pop();
        tiles.push(broken);
        write_czi(&path, &tiles);

        let reader = CziReader::open(&path).expect("the file opens");
        let stored = reader
            .stored_pyramid(0)
            .expect("the file's own pyramid is offered");
        assert!(stored.levels().is_empty(), "the level is not offered");
        let refused = stored.refused_levels();
        assert_eq!(refused.len(), 1, "the level is refused");
        assert_eq!(refused[0].factor, 2, "the refused level's minification");
        assert_eq!(refused[0].skipped, 1, "one tile is behind the refusal");
        assert!(
            refused[0].reason.contains("compression"),
            "the reason names the compression: {}",
            refused[0].reason
        );
    }
}

/// JPEG-XR is a compression this build decodes only with the `jpegxr` feature.
#[cfg(not(feature = "jpegxr"))]
#[test]
fn a_level_with_a_jpeg_xr_tile_is_refused_without_the_feature() {
    let path = scratch("stored_jpegxr");
    let mut tiles = scene();
    whole_level(&mut tiles);
    let mut broken = level_tile(0, 2, 128, 0, 64, 64, 0);
    broken.compression = 4;
    tiles.pop();
    tiles.push(broken);
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    assert!(stored.levels().is_empty(), "the level is not offered");
    let refused = stored.refused_levels();
    assert_eq!(refused.len(), 1, "the level is refused");
    assert!(
        refused[0].reason.contains("JPEG-XR"),
        "the reason names the codec: {}",
        refused[0].reason
    );
}

#[test]
fn a_level_without_a_tile_for_every_plane_is_refused() {
    let path = scratch("stored_missing_plane");
    let mut tiles = full_resolution_tiles(2, 1, 64, 0, 0);
    tiles.extend(full_resolution_tiles(2, 1, 64, 0, 1));
    // The level holds the first channel only.
    tiles.push(level_tile(0, 2, 0, 0, 64, 64, 0));
    tiles.push(level_tile(0, 2, 64, 0, 64, 64, 0));
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
    assert!(stored.levels().is_empty(), "the level is not offered");
    let refused = stored.refused_levels();
    assert_eq!(refused.len(), 1, "the level is refused");
    assert!(
        refused[0].reason.contains("plane"),
        "the reason names the plane: {}",
        refused[0].reason
    );
}

#[test]
fn a_level_that_stops_more_than_a_cell_short_of_the_scene_is_refused() {
    let path = scratch("stored_short");
    let mut tiles = scene();
    // One tile of the level: the scene is three cells wide, so two cells are missing.
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
        refused[0].reason.contains("scene"),
        "the reason names the scene: {}",
        refused[0].reason
    );
}

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

/// A coarse tile whose stored size is zero is skipped by name, and its size matches the good
/// level's cell, so the conservative refusal takes that level with it.
#[test]
fn a_level_with_a_tile_of_zero_size_is_refused() {
    let path = scratch("stored_zero_size");
    let mut tiles = scene();
    whole_level(&mut tiles);
    let mut zero = level_tile(0, 2, 128, 0, 64, 64, 0);
    zero.stored = Some((0, 0));
    zero = zero.painted(Vec::new());
    tiles.pop();
    tiles.push(zero);
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    assert!(stored.levels().is_empty(), "the level is not offered");
    let refused = stored.refused_levels();
    assert_eq!(refused.len(), 1, "the level is refused");
    assert_eq!(refused[0].factor, 2, "the refused level's minification");
    assert_eq!(refused[0].skipped, 1, "one tile is behind the refusal");
    assert!(
        refused[0].reason.contains("zero"),
        "the reason names the zero size: {}",
        refused[0].reason
    );
}

/// A tile stored at half its height only names no minification, so it refuses the level.
#[test]
fn a_one_axis_sub_sampled_tile_is_refused() {
    let path = scratch("stored_one_axis");
    let mut tiles = scene();
    whole_level(&mut tiles);
    let mut one_axis = level_tile(0, 2, 128, 0, 64, 64, 0);
    one_axis.stored = Some((64, 32));
    one_axis = one_axis.painted(vec![0u8; 64 * 32]);
    tiles.pop();
    tiles.push(one_axis);
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    assert!(stored.levels().is_empty(), "the level is not offered");
    let refused = stored.refused_levels();
    assert_eq!(refused.len(), 1, "the level is refused");
    assert!(
        refused[0].reason.contains("axes"),
        "the reason names the axes: {}",
        refused[0].reason
    );
}

/// The `PyramidType` byte is legacy: a coarse tile without it is still a level.
#[test]
fn a_coarse_tile_without_the_legacy_byte_is_still_a_level() {
    let path = scratch("stored_no_legacy_byte");
    let mut tiles = scene();
    whole_level(&mut tiles);
    for tile in &mut tiles {
        tile.pyramid_type = 0;
    }
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    let levels = stored.levels();
    assert_eq!(levels.len(), 1, "the file carries one coarse level");
    assert_eq!(levels[0].factor, 2, "the level is half the resolution");
}

/// A coarse tile whose pixel span runs out of the file refuses its level.
#[test]
fn a_level_with_a_tile_whose_span_runs_out_of_the_file_is_refused() {
    let path = scratch("stored_bad_span");
    let mut tiles = scene();
    whole_level(&mut tiles);
    write_czi(&path, &tiles);
    break_last_span(&path);

    let reader = CziReader::open(&path).expect("the file opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    assert!(stored.levels().is_empty(), "the level is not offered");
    let refused = stored.refused_levels();
    assert_eq!(refused.len(), 1, "the level is refused");
    assert_eq!(refused[0].skipped, 1, "one tile is behind the refusal");
    assert!(
        refused[0].reason.contains("span"),
        "the reason names the span: {}",
        refused[0].reason
    );
}
