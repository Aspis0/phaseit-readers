// SPDX-License-Identifier: Apache-2.0
//! The CZ-LSM-Info struct of an LSM file: how long it is, and what a file that states nothing in it publishes.
//! The files are synthetic; the real ones are gated behind `PHASIT_REAL_DATA_DIR` and live in `lsm_real.rs`.

mod support;

use std::path::Path;

use phasit_imread::lsm_reader::LsmReader;
use phasit_imread::{Error, ImageReader};
use support::lsm::{build, LsmSpec};
use support::scratch_dir;

const CZ_VOXEL_Z_AT: usize = 56;

fn write_an_lsm_with_voxel_z(path: &Path, voxel_z_m: f64) {
    let (mut bytes, offsets) = build(&LsmSpec::plain(2, 2, 1));
    let at = offsets.cz + CZ_VOXEL_Z_AT;
    bytes[at..at + 8].copy_from_slice(&voxel_z_m.to_le_bytes());
    std::fs::write(path, &bytes).expect("write the synthetic LSM");
}

#[test]
fn a_short_struct_states_no_tile_counts_and_no_channel_block() {
    let dir = scratch_dir("lsm_short_struct");
    let path = dir.join("shortstruct.lsm");
    // The struct declares 108 bytes, so the colour pointer and tile counts beyond it must not be read.
    let spec = LsmSpec {
        x: 2,
        y: 2,
        z: 1,
        t: 1,
        channels: 1,
        positions: 1,
        tiles: 4,
        channel_names: Some(vec!["Ch2 GaAsP".to_string()]),
        scan_info_names: vec!["Ch2 GaAsP".to_string()],
        pages: Some(1),
        strips_per_channel: 1,
        cz_bytes: 108,
    };
    let (bytes, _) = build(&spec);
    std::fs::write(&path, &bytes).expect("write the synthetic LSM");

    let reader = LsmReader::open(&path).expect("open the file");
    let meta = reader.meta();
    assert_eq!(
        meta.scene_count(),
        1,
        "a struct that states no tile counts is a single-scene file"
    );
    assert_eq!(meta.scenes[0].channels[0].name, None);
}

#[test]
fn a_struct_with_the_wrong_magic_is_refused() {
    let dir = scratch_dir("lsm_bad_magic");
    let path = dir.join("badmagic.lsm");
    let (mut bytes, offsets) = build(&LsmSpec::plain(2, 2, 1));
    let cz = offsets.cz;
    bytes[cz..cz + 4].copy_from_slice(&0x0400_0000u32.to_le_bytes());
    std::fs::write(&path, &bytes).expect("write the synthetic LSM");

    let error = match LsmReader::open(&path) {
        Ok(_) => panic!("a struct that is not an LSM struct must be refused"),
        Err(error) => error,
    };
    match error {
        Error::Decode(message) => assert!(
            message.contains("magic"),
            "the message must name the reason: {message}"
        ),
        other => panic!("expected a decode error, got {other:?}"),
    }
}

#[test]
fn an_lsm_with_a_placeholder_z_step_says_so() {
    // 1e-19 m is a writers' sentinel, not a distance: the X/Y sizes are still published and the Z step is not.
    let dir = scratch_dir("lsm_placeholder_z");
    let path = dir.join("placeholderz.lsm");
    write_an_lsm_with_voxel_z(&path, 1e-19);

    let reader = LsmReader::open(&path).expect("open the file");
    let scene = reader.meta().scene(0).expect("the file has a scene");
    let px = scene.px_size_um.expect("the X/Y size is published");
    assert_eq!(px.z, None, "the placeholder is not a step");
    assert!(
        (px.x as f64 - 0.1).abs() < 1e-6,
        "the X size is published, got {}",
        px.x
    );
    assert!(
        (px.y as f64 - 0.1).abs() < 1e-6,
        "the Y size is published, got {}",
        px.y
    );
    assert_eq!(
        scene.notes,
        vec![phasit_imread::calibration::z_step_note(1e-13)
            .expect("a placeholder earns a note")],
        "the scene says why it has no Z step"
    );
}

#[test]
fn an_lsm_with_an_invalid_voxel_z_still_publishes_its_xy_size() {
    // A zero Z states no Z step: the X/Y size is still published, with `z: null`, never as no pixel size.
    let dir = scratch_dir("lsm_invalid_z");
    let path = dir.join("invalidz.lsm");
    write_an_lsm_with_voxel_z(&path, 0.0);

    let reader = LsmReader::open(&path).expect("open the file");
    let scene = reader.meta().scene(0).expect("the file has a scene");
    let px = scene.px_size_um.expect("the X/Y size is published");
    assert_eq!(px.z, None, "a zero voxel is not a step");
    assert!(
        (px.x as f64 - 0.1).abs() < 1e-6,
        "the X size is published, got {}",
        px.x
    );
    assert!(
        (px.y as f64 - 0.1).abs() < 1e-6,
        "the Y size is published, got {}",
        px.y
    );
    assert!(
        scene.notes.is_empty(),
        "a file that states no step has nothing to explain"
    );
}
