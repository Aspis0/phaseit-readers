// SPDX-License-Identifier: Apache-2.0
//! The CZ-LSM-Info struct of an LSM file: how long it is, and what a file
//! that states nothing in it publishes.
//!
//! The files are synthetic (see `support::lsm`); the real ones are gated
//! behind `PHASIT_REAL_DATA_DIR` and live in `lsm_real.rs`.

mod support;

use std::path::Path;

use phasit_imread::lsm_reader::LsmReader;
use phasit_imread::{Error, ImageReader};
use support::lsm::{build, LsmSpec};
use support::scratch_dir;

/// The offset of the voxel size Z inside the CZ-LSM-Info struct (f64, metres).
const CZ_VOXEL_Z_AT: usize = 56;

/// Write a synthetic LSM whose CZ-LSM-Info states `voxel_z_m` as its Z voxel
/// size (the X and Y voxel sizes are the fixture's own).
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
    // The struct declares 108 bytes, so the channel-colours pointer (at +108)
    // and the tile counts (at +264/+268) are beyond it. The bytes there say
    // "one position, four tiles" and point at a colour block — the reader
    // must not read them: the file states neither. The file itself holds one
    // page, which is what a file without tile counts must hold.
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
    // 1e-19 m is the sentinel some writers put in the Z field: 1e-13 um, no
    // distance a microscope measured. The X/Y voxel sizes are real and are
    // still published, and the scene SAYS that its Z step was not used.
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
    // A zero voxel Z states no Z step at all. The X/Y sizes are real, so the
    // file publishes them: the pixel size is published with the Z step
    // ABSENT (the wire writes z: null), never as "no pixel size at all".
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
