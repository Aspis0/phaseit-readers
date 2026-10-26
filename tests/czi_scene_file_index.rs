// SPDX-License-Identifier: Apache-2.0
//! Where a published CZI scene says it sits in the file.
//!
//! `scene_file_index` is the RANK of a scene among the sorted distinct `S`
//! values of the file's full-resolution subblocks. It differs from the
//! scene's own index when an earlier scene of the file was refused, so the
//! refusal has to hold its place for the scenes around it to keep theirs.
//!
//! The containers are written by hand (see `support::czi`): one scene per
//! distinct `S` value, and a scene made unreadable on purpose by mixing two
//! pixel types in it.

mod support;

use phasit_imread::czi_reader::CziReader;
use phasit_imread::ImageReader;
use support::czi::{scratch, write_czi, Tile};

/// A scene the reader refuses: Gray8 and Gray16 tiles do not compose.
fn refused_scene(scene: i32) -> [Tile; 2] {
    [
        Tile::full(scene, 0, 4, 2),
        Tile::full(scene, 1, 4, 2),
    ]
}

#[test]
fn a_refused_scene_first_leaves_the_published_scene_at_file_index_one() {
    let path = scratch("refused_first");
    let mut tiles = refused_scene(0).to_vec();
    tiles.push(Tile::full(1, 0, 4, 2));
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    assert_eq!(reader.meta().scene_count(), 1, "the pure scene is published");
    assert_eq!(
        reader.scene_file_index(0),
        1,
        "the published scene is the file's scene 1"
    );
    assert_eq!(reader.refused_scenes().len(), 1);
    assert_eq!(reader.refused_scenes()[0].image_index, 0);
}

#[test]
fn a_refused_scene_last_leaves_the_published_scene_at_file_index_zero() {
    let path = scratch("refused_last");
    let mut tiles = vec![Tile::full(0, 0, 4, 2)];
    tiles.extend(refused_scene(1));
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    assert_eq!(reader.meta().scene_count(), 1, "the pure scene is published");
    assert_eq!(
        reader.scene_file_index(0),
        0,
        "the published scene is the file's scene 0"
    );
    assert_eq!(reader.refused_scenes().len(), 1);
    assert_eq!(reader.refused_scenes()[0].image_index, 1);
}

#[test]
fn two_refused_scenes_keep_both_sides_in_place() {
    let path = scratch("two_refusals");
    let mut tiles = refused_scene(0).to_vec();
    tiles.push(Tile::full(1, 0, 4, 2));
    tiles.extend(refused_scene(2));
    tiles.push(Tile::full(3, 0, 4, 2));
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    assert_eq!(reader.meta().scene_count(), 2, "the two pure scenes are published");
    assert_eq!(reader.scene_file_index(0), 1, "the first published scene");
    assert_eq!(reader.scene_file_index(1), 3, "the last published scene");

    let refused = reader.refused_scenes();
    assert_eq!(refused.len(), 2, "both scenes are refused");
    assert_eq!(refused[0].image_index, 0);
    assert_eq!(refused[1].image_index, 2);
}

#[test]
fn a_file_whose_scenes_start_at_one_numbers_the_ranks_from_zero() {
    let path = scratch("s_from_one");
    write_czi(
        &path,
        &[Tile::full(1, 0, 4, 2), Tile::full(2, 0, 4, 2)],
    );

    let reader = CziReader::open(&path).expect("the file opens");
    assert_eq!(reader.meta().scene_count(), 2);
    // The file numbers its scenes from 1; the rank is a position, so it counts
    // from 0.
    assert_eq!(reader.scene_file_index(0), 0);
    assert_eq!(reader.scene_file_index(1), 1);
}

#[test]
fn a_file_whose_scenes_have_a_gap_numbers_the_ranks_from_zero() {
    let path = scratch("s_gap");
    write_czi(
        &path,
        &[Tile::full(0, 0, 4, 2), Tile::full(5, 0, 4, 2)],
    );

    let reader = CziReader::open(&path).expect("the file opens");
    assert_eq!(reader.meta().scene_count(), 2);
    assert_eq!(reader.scene_file_index(0), 0);
    assert_eq!(reader.scene_file_index(1), 1);
}

#[test]
fn a_refused_scene_of_a_file_that_starts_at_one_is_ranked_not_numbered() {
    let path = scratch("s_from_one_refused");
    let mut tiles = refused_scene(1).to_vec();
    tiles.push(Tile::full(2, 0, 4, 2));
    write_czi(&path, &tiles);

    let reader = CziReader::open(&path).expect("the file opens");
    assert_eq!(reader.meta().scene_count(), 1);
    // The published scene is the file's scene 2, and its rank is 1.
    assert_eq!(reader.scene_file_index(0), 1);
    let refused = reader.refused_scenes();
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0].image_index, 0, "the rank of the refused scene");
    assert!(
        refused[0].reason.contains("S=1"),
        "the reason names the S value of the file: {}",
        refused[0].reason
    );
}
