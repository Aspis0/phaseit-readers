// SPDX-License-Identifier: Apache-2.0
//! The CZ-LSM-Info struct of an LSM file: how long it is, and what a file
//! that states nothing in it publishes.
//!
//! The files are synthetic (see `support::lsm`); the real ones are gated
//! behind `PHASIT_REAL_DATA_DIR` and live in `lsm_real.rs`.

mod support;

use phasit_imread::lsm_reader::LsmReader;
use phasit_imread::{Error, ImageReader};
use support::lsm::{build, LsmSpec};
use support::scratch_dir;

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
