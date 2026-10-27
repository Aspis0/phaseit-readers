// SPDX-License-Identifier: Apache-2.0
//! A tile/position LSM file: the counts in its CZ-LSM-Info say how many
//! separate acquisitions it holds, and the reader publishes one scene per
//! acquisition.
//!
//! The files are synthetic (see `support::lsm`), because no committed fixture
//! carries a tile scan — the real one is gated behind `PHASIT_REAL_DATA_DIR`
//! and lives in `lsm_real.rs`.

mod support;

use phasit_imread::lsm_reader::LsmReader;
use phasit_imread::{Error, ImageReader, PlaneBuf};
use support::lsm::{write_lsm, LsmSpec};
use support::scratch_dir;

/// A file whose counts say four tiles, so it holds four scenes.
fn tile_scan(dir: &std::path::Path, pages: Option<usize>) -> std::path::PathBuf {
    let path = dir.join("tilescan.lsm");
    write_lsm(
        &path,
        &LsmSpec {
            x: 4,
            y: 3,
            z: 1,
            t: 1,
            channels: 1,
            positions: 1,
            tiles: 4,
            channel_names: Some(vec!["Ch2 GaAsP".to_string()]),
            scan_info_names: vec!["Ch2 GaAsP".to_string()],
            pages,
            cz_bytes: 464,
        },
    );
    path
}

#[test]
fn a_tile_scan_publishes_one_scene_per_tile() {
    let dir = scratch_dir("lsm_tile_scan");
    let path = tile_scan(&dir, None);

    let reader = LsmReader::open(&path).expect("open the tile scan");
    let meta = reader.meta();
    assert_eq!(meta.scene_count(), 4, "one scene per tile");
    assert_eq!(meta.name, "tilescan");

    // The scene names carry Bio-Formats' suffix: "<name> #1" .. "<name> #4".
    let names: Vec<&str> = meta.scenes.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["tilescan #1", "tilescan #2", "tilescan #3", "tilescan #4"]
    );

    // Every scene carries the file's own geometry.
    for scene in &meta.scenes {
        assert_eq!(scene.dims.t, 1);
        assert_eq!(scene.dims.c, 1);
        assert_eq!(scene.dims.z, 1);
        assert_eq!(scene.dims.y, 3);
        assert_eq!(scene.dims.x, 4);
        assert_eq!(scene.channels.len(), 1);
        assert_eq!(scene.channels[0].name.as_deref(), Some("Ch2 GaAsP"));
    }

    // Each scene reads its own page: the synthetic pages hold distinct values.
    for scene in 0..4 {
        let plane = reader.plane(scene, 0, 0, 0).expect("the scene's plane");
        let PlaneBuf::U8(bytes) = &plane.data else {
            panic!("expected a U8 plane");
        };
        assert_eq!(bytes.len(), 12);
        assert!(bytes.iter().all(|&b| b == scene as u8), "scene {scene}");
    }

    // A scene outside the file is out of range, never a panic.
    assert!(matches!(
        reader.plane(4, 0, 0, 0),
        Err(Error::OutOfRange { scene: 4, .. })
    ));
}

#[test]
fn counts_that_do_not_explain_the_pages_fail_closed() {
    let dir = scratch_dir("lsm_tile_mismatch");
    // The counts say four tiles, so the file must hold four pages — it holds
    // three. The reader must refuse rather than guess a layout.
    let path = tile_scan(&dir, Some(3));

    let error = match LsmReader::open(&path) {
        Ok(_) => panic!("the counts do not add up, but the file opened"),
        Err(error) => error,
    };
    match error {
        Error::Decode(message) => assert!(
            message.contains("3 real pages") && message.contains("T*Z*P*M = 4"),
            "the message must name the counts: {message}"
        ),
        other => panic!("expected a decode error, got {other:?}"),
    }
}

#[test]
fn negative_counts_are_refused() {
    let dir = scratch_dir("lsm_negative_counts");
    let path = dir.join("negative.lsm");
    write_lsm(
        &path,
        &LsmSpec {
            positions: -1,
            tiles: 4,
            ..LsmSpec::plain(2, 2, 1)
        },
    );

    let error = match LsmReader::open(&path) {
        Ok(_) => panic!("a negative tile count is malformed, but the file opened"),
        Err(error) => error,
    };
    match error {
        Error::Decode(message) => assert!(
            message.contains("negative DimensionP"),
            "the message must name the count: {message}"
        ),
        other => panic!("expected a decode error, got {other:?}"),
    }
}

#[test]
fn a_multi_timepoint_file_is_refused() {
    let dir = scratch_dir("lsm_multi_time");
    let path = dir.join("multitime.lsm");
    write_lsm(
        &path,
        &LsmSpec {
            t: 2,
            z: 1,
            pages: Some(2),
            ..LsmSpec::plain(2, 2, 1)
        },
    );

    // The order of a file's time points is not established, so the reader
    // refuses rather than misassign planes.
    let error = match LsmReader::open(&path) {
        Ok(_) => panic!("a multi-timepoint file is not modelled, but it opened"),
        Err(error) => error,
    };
    match error {
        Error::Unsupported(message) => assert!(
            message.contains("multi-timepoint"),
            "the message must name the reason: {message}"
        ),
        other => panic!("expected an unsupported error, got {other:?}"),
    }
}

#[test]
fn a_plain_file_is_still_one_scene() {
    let dir = scratch_dir("lsm_plain");
    let path = dir.join("plain.lsm");
    write_lsm(&path, &LsmSpec::plain(4, 3, 2));

    let reader = LsmReader::open(&path).expect("open the plain file");
    let meta = reader.meta();
    assert_eq!(meta.scene_count(), 1, "a plain file holds one scene");
    assert_eq!(meta.scenes[0].name, "plain", "no suffix on a single scene");
    assert_eq!(meta.scenes[0].dims.z, 2);
}

