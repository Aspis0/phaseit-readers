// SPDX-License-Identifier: Apache-2.0
//! What the CZI reader says about the scenes it could not read.
//!
//! The containers are written by hand (the reader's module docs spell the
//! segment layout out): a `ZISRAWFILE` header, a `ZISRAWDIRECTORY` listing one
//! entry per tile, one `ZISRAWSUBBLOCK` per tile, and a `ZISRAWMETADATA`
//! segment. Each tile names the scene (`S`) it belongs to, so a scene can be
//! made unreadable on purpose.
//!
//! A reader that drops a scene without saying so renumbers every scene after
//! it: the caller asking for scene N of the file then gets a different image.
//! So a refused scene must be NAMED, and the scenes around it must keep their
//! own place in the file.

mod support;

use phasit_imread::czi_reader::CziReader;
use phasit_imread::{Error, ImageReader};
use support::czi::{scratch, write_czi, Tile};

#[test]
fn a_heterogeneous_scene_is_refused_with_its_reason() {
    let path = scratch("heterogeneous");
    write_czi(
        &path,
        &[
            Tile::full(0, 0, 4, 2),
            Tile::full(1, 0, 4, 2),
            // A Gray16 tile in a Gray8 scene: the scene cannot be composed.
            Tile::full(1, 1, 4, 2),
            Tile::full(2, 0, 4, 2),
        ],
    );

    let reader = CziReader::open(&path).expect("the file opens");
    assert_eq!(reader.meta().scene_count(), 2, "the two pure scenes are published");

    let refused = reader.refused_scenes();
    assert_eq!(refused.len(), 1, "the heterogeneous scene is refused");
    assert_eq!(refused[0].image_index, 1, "it is the file's scene 1");
    assert!(
        refused[0].reason.contains("pixel type"),
        "the reason names the cause: {}",
        refused[0].reason
    );
    assert!(
        refused[0].reason.contains("S=1"),
        "the reason names the S value: {}",
        refused[0].reason
    );
}

#[test]
fn a_refused_scene_does_not_renumber_the_scenes_after_it() {
    let path = scratch("positions");
    write_czi(
        &path,
        &[
            Tile::full(0, 0, 4, 2),
            Tile::full(1, 0, 4, 2),
            Tile::full(1, 1, 4, 2),
            Tile::full(2, 0, 4, 2),
        ],
    );

    let reader = CziReader::open(&path).expect("the file opens");
    // The published scenes are the file's scenes 0 and 2: the middle one was
    // refused, and the last scene must still say it is scene 2 of the file.
    assert_eq!(reader.scene_file_index(0), 0);
    assert_eq!(reader.scene_file_index(1), 2);
}

#[test]
fn a_scene_whose_tiles_are_all_unreadable_is_refused_with_its_reason() {
    let path = scratch("unreadable");
    write_czi(
        &path,
        &[
            Tile::full(0, 0, 4, 2),
            // Gray32 is not modelled, so every tile of this scene is skipped.
            Tile::full(1, 10, 4, 2),
            Tile::full(2, 0, 4, 2),
        ],
    );

    let reader = CziReader::open(&path).expect("the file opens");
    assert_eq!(reader.meta().scene_count(), 2);

    let refused = reader.refused_scenes();
    assert_eq!(refused.len(), 1, "the unreadable scene is refused");
    assert_eq!(refused[0].image_index, 1, "it is the file's scene 1");
    assert!(
        refused[0].reason.contains("S=1"),
        "the reason names the S value: {}",
        refused[0].reason
    );
    assert_eq!(reader.scene_file_index(1), 2, "the last scene keeps its place");
}

#[test]
fn a_scene_that_lost_a_tile_is_refused_with_the_count_and_the_cause() {
    let path = scratch("lost_tile");
    write_czi(
        &path,
        &[
            Tile::full(0, 0, 4, 2),
            // A Gray32 tile in scene 0: the reader does not model that pixel
            // type, so the scene loses one of its two tiles. Publishing it
            // would give a scene smaller than the file says.
            Tile::full(0, 10, 4, 2),
            Tile::full(1, 0, 4, 2),
        ],
    );

    let reader = CziReader::open(&path).expect("the file opens");
    assert_eq!(
        reader.meta().scene_count(),
        1,
        "the scene that lost a tile is not published"
    );

    let refused = reader.refused_scenes();
    assert_eq!(refused.len(), 1, "the scene that lost a tile is refused");
    assert_eq!(refused[0].image_index, 0, "it is the file's scene 0");
    assert!(
        refused[0].reason.contains("S=0"),
        "the reason names the S value: {}",
        refused[0].reason
    );
    assert!(
        refused[0].reason.contains("1 of its 2"),
        "the reason counts the skipped tile: {}",
        refused[0].reason
    );
    assert!(
        refused[0].reason.contains("pixel type"),
        "the reason names the cause: {}",
        refused[0].reason
    );
    // The scene after it keeps its own place in the file.
    assert_eq!(reader.scene_file_index(0), 1);
}

#[test]
fn a_scene_of_pyramid_tiles_only_is_neither_published_nor_refused() {
    let path = scratch("pyramid_only");
    write_czi(
        &path,
        &[
            Tile::full(0, 0, 4, 2),
            // Scene 1 holds only a sub-sampled tile: it is not a scene of the
            // file this reader composes, so it is not a refusal either.
            Tile::pyramid(1, 0, 4, 2),
        ],
    );

    let reader = CziReader::open(&path).expect("the file opens");
    assert_eq!(
        reader.meta().scene_count(),
        1,
        "only the full-resolution scene is published"
    );
    assert!(
        reader.refused_scenes().is_empty(),
        "a scene of pyramid tiles only is not a refusal"
    );
    assert_eq!(reader.scene_file_index(0), 0);
}

#[test]
fn a_file_without_a_refused_scene_refuses_nothing() {
    let path = scratch("clean");
    write_czi(
        &path,
        &[Tile::full(0, 0, 4, 2), Tile::full(1, 0, 4, 2)],
    );

    let reader = CziReader::open(&path).expect("the file opens");
    assert_eq!(reader.meta().scene_count(), 2);
    assert!(reader.refused_scenes().is_empty(), "nothing was refused");
    assert_eq!(reader.scene_file_index(0), 0);
    assert_eq!(reader.scene_file_index(1), 1);
}

#[test]
fn a_file_whose_only_scene_lost_a_tile_fails_with_the_reason_of_that_loss() {
    let path = scratch("only_scene_lost_tile");
    write_czi(
        &path,
        &[
            Tile::full(0, 0, 4, 2),
            // A Gray32 tile in the only scene of the file: the reader does not
            // model that pixel type, so the scene loses one of its two tiles
            // and there is nothing left to open.
            Tile::full(0, 10, 4, 2),
        ],
    );

    match CziReader::open(&path) {
        Err(Error::Unsupported(message)) => {
            assert!(
                message.contains("S=0"),
                "the error names the S value of the scene: {message}"
            );
            assert!(
                message.contains("1 of its 2"),
                "the error counts the tiles the scene lost: {message}"
            );
            assert!(
                message.contains("pixel type"),
                "the error names the cause of the loss: {message}"
            );
            assert!(
                !message.contains("no scene with a single supported pixel type"),
                "the error does not misdiagnose a lost tile: {message}"
            );
        }
        Err(other) => panic!("the file must not open, got {other}"),
        Ok(_) => panic!("the file must not open"),
    }
}
