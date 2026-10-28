// SPDX-License-Identifier: Apache-2.0
//! The channel metadata of an LSM file: the names and the display colours the
//! file's channel-colours block carries.
//!
//! The files are synthetic (see `support::lsm`); the real ones are gated
//! behind `PHASIT_REAL_DATA_DIR` and live in `lsm_real.rs`.

mod support;

use phasit_imread::lsm_reader::LsmReader;
use phasit_imread::ImageReader;
use support::lsm::{build, write_lsm, LsmSpec};
use support::scratch_dir;

/// A three-channel file whose colour block and ScanInfo disagree, as they do
/// in the real files.
fn three_channels() -> LsmSpec {
    LsmSpec {
        x: 2,
        y: 2,
        z: 1,
        t: 1,
        channels: 3,
        positions: 1,
        tiles: 1,
        channel_names: Some(vec![
            "Ch3-T1".to_string(),
            "Ch1-T2".to_string(),
            "Ch2 GaAsP-T3".to_string(),
        ]),
        scan_info_names: vec![
            "Ch3".to_string(),
            "Ch1".to_string(),
            "Ch2 GaAsP".to_string(),
        ],
        pages: None,
        strips_per_channel: 1,
        cz_bytes: 464,
    }
}

#[test]
fn the_channel_names_come_from_the_channel_colours_block() {
    let dir = scratch_dir("lsm_channel_names");
    let path = dir.join("names.lsm");
    // The two blocks disagree, as they do in the real files: the
    // channel-colours block holds the names with the acquisition's suffixes,
    // the ScanInfo the bare ones.
    write_lsm(&path, &three_channels());

    let reader = LsmReader::open(&path).expect("open the file");
    let scene = &reader.meta().scenes[0];
    let names: Vec<Option<&str>> = scene
        .channels
        .iter()
        .map(|channel| channel.name.as_deref())
        .collect();
    assert_eq!(
        names,
        vec![Some("Ch3-T1"), Some("Ch1-T2"), Some("Ch2 GaAsP-T3")],
        "the channel-colours block is where the names come from"
    );
}

#[test]
fn each_channel_publishes_its_display_colour() {
    let dir = scratch_dir("lsm_channel_colours");
    let path = dir.join("colours.lsm");
    write_lsm(&path, &three_channels());

    let reader = LsmReader::open(&path).expect("open the file");
    let scene = &reader.meta().scenes[0];
    let colours: Vec<Option<[u8; 3]>> = scene.channels.iter().map(|c| c.color).collect();
    assert_eq!(
        colours,
        vec![Some([255, 0, 0]), Some([0, 255, 0]), Some([0, 0, 255])],
        "the colour words of the channel-colours block"
    );
}

#[test]
fn a_file_without_a_channel_block_has_unnamed_channels() {
    let dir = scratch_dir("lsm_no_names");
    let path = dir.join("nonames.lsm");
    // The file states no pointer to a channel-colours block at all.
    write_lsm(
        &path,
        &LsmSpec {
            channels: 2,
            channel_names: None,
            ..LsmSpec::plain(2, 2, 1)
        },
    );

    let reader = LsmReader::open(&path).expect("open the file");
    let scene = &reader.meta().scenes[0];
    assert_eq!(scene.channels.len(), 2);
    for channel in &scene.channels {
        assert_eq!(channel.name, None, "no names, and none invented");
        assert_eq!(channel.color, None, "no colours, and none invented");
    }
}

#[test]
fn a_block_whose_name_table_is_unreadable_yields_unnamed_channels() {
    let dir = scratch_dir("lsm_bad_name_table");
    let path = dir.join("badnames.lsm");
    // The file has a channel-colours block, but its name table points past
    // the end of the file: the block states no readable names.
    let (mut bytes, offsets) = build(&three_channels());
    let colors = offsets.colors.expect("the file has a colour block");
    bytes[colors + 16..colors + 20].copy_from_slice(&9_000_000i32.to_le_bytes());
    std::fs::write(&path, &bytes).expect("write the synthetic LSM");

    let reader = LsmReader::open(&path).expect("open the file");
    let scene = &reader.meta().scenes[0];
    for channel in &scene.channels {
        assert_eq!(channel.name, None, "no names, and none invented");
    }
}

#[test]
fn a_colour_pointer_past_the_end_of_the_file_opens_with_unnamed_channels() {
    let dir = scratch_dir("lsm_bad_colour_pointer");
    let path = dir.join("badpointer.lsm");
    // The CZ-LSM-Info points at a channel-colours block that does not fit the
    // file. The file must still open — with unnamed channels.
    let (mut bytes, offsets) = build(&three_channels());
    let cz = offsets.cz;
    let past_the_end = (bytes.len() - 8) as i32;
    bytes[cz + 108..cz + 112].copy_from_slice(&past_the_end.to_le_bytes());
    std::fs::write(&path, &bytes).expect("write the synthetic LSM");

    let reader = LsmReader::open(&path).expect("a bad pointer must not stop the file from opening");
    let scene = &reader.meta().scenes[0];
    for channel in &scene.channels {
        assert_eq!(channel.name, None, "no names, and none invented");
        assert_eq!(channel.color, None);
    }
}

#[test]
fn a_too_long_name_leaves_only_that_channel_unnamed() {
    let dir = scratch_dir("lsm_long_name");
    let path = dir.join("longname.lsm");
    // The first channel's name is beyond what a name may be; the second
    // channel's name is read all the same.
    let long = "x".repeat(4096);
    write_lsm(
        &path,
        &LsmSpec {
            channels: 2,
            channel_names: Some(vec![long, "Ch2 GaAsP".to_string()]),
            ..LsmSpec::plain(2, 2, 1)
        },
    );

    let reader = LsmReader::open(&path).expect("open the file");
    let scene = &reader.meta().scenes[0];
    assert_eq!(scene.channels[0].name, None, "the long name is skipped");
    assert_eq!(scene.channels[1].name.as_deref(), Some("Ch2 GaAsP"));
}

#[test]
fn an_empty_name_stays_empty() {
    let dir = scratch_dir("lsm_empty_name");
    let path = dir.join("emptyname.lsm");
    // The file states a channel with an empty name, which is not the same as
    // a channel the file says nothing about.
    write_lsm(
        &path,
        &LsmSpec {
            channels: 1,
            channel_names: Some(vec![String::new()]),
            ..LsmSpec::plain(2, 2, 1)
        },
    );

    let reader = LsmReader::open(&path).expect("open the file");
    let scene = &reader.meta().scenes[0];
    assert_eq!(scene.channels[0].name.as_deref(), Some(""));
}
