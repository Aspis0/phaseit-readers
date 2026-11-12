// SPDX-License-Identifier: Apache-2.0
//! Every way an ICS file is refused: the version, the axis order, the sample type, the compression, the byte order,
//! a header without `end`, a `layout parameters` that disagrees with the layout, a missing `.ids` sibling, a deflated
//! plane that is not contiguous and data that runs past the end of the file.
//! The files are written by `tests/support/ics.rs` and removed by `Scratch`, so a failing assertion leaves nothing.

mod support;

use std::ops::Deref;

use phasit_imread::ics_reader::IcsReader;
use phasit_imread::{Error, ImageReader};

use support::ics::IcsFile;
use support::scratch::Scratch;

fn scratch(name: &str) -> Scratch {
    Scratch::new(std::env::temp_dir().join(format!(
        "phasit_ics_refuse_{name}_{}.ics",
        std::process::id()
    )))
}

/// The sentence a file is refused with, whatever the kind.
fn refusal_text(file: &IcsFile, name: &str) -> String {
    let path = scratch(name);
    file.write(&path);
    match IcsReader::open(&path) {
        Err(Error::Unsupported(message)) | Err(Error::Parse(message)) => message,
        Err(other) => panic!("expected a refusal, got {other:?}"),
        Ok(_) => panic!("the file opened"),
    }
}

#[test]
fn a_version_this_reader_does_not_read_is_refused() {
    for version in ["0.9", "3.0", "2", "1.1"] {
        let file = IcsFile {
            version_text: version.to_string(),
            ..IcsFile::new(&["x", "y"], &[4, 3], 16)
        };
        let message = refusal_text(&file, &format!("version_{version}"));
        assert!(
            message.contains(&format!("ICS version {version}")),
            "{message}"
        );
    }
}

#[test]
fn a_header_without_a_version_is_not_an_ics_file() {
    let file = IcsFile {
        version_text: String::new(),
        ..IcsFile::new(&["x", "y"], &[4, 3], 16)
    };
    let message = refusal_text(&file, "no_version");
    assert!(message.contains("ics_version"), "{message}");
}

#[test]
fn an_axis_name_this_reader_cannot_use_is_refused_by_name() {
    for (axes, expected, name) in [
        (&["x", "y", "q"][..], "axis \"q\"", "unknown"),
        (&["x", "y", "x"][..], "more than once", "repeated"),
        (&["y", "z"][..], "does not name axis", "no_x"),
        (&["x", "z"][..], "does not name axis", "no_y"),
    ] {
        let file = IcsFile::new(axes, &[4, 3, 2], 16);
        let message = refusal_text(&file, name);
        assert!(message.contains(expected), "{name}: {message}");
    }
}

#[test]
fn a_zero_extent_is_refused_by_axis() {
    for axis in 0..3 {
        let mut sizes = [4usize, 3, 2];
        sizes[axis] = 0;
        let file = IcsFile::new(&["x", "y", "z"], &sizes, 16);
        let message = refusal_text(&file, &format!("zero_{axis}"));
        assert!(message.contains("must be positive"), "{message}");
        assert!(message.contains("is 0"), "{message}");
    }
}

#[test]
fn a_size_that_is_not_an_integer_is_refused() {
    let file = IcsFile {
        sizes: vec![4, 3],
        extra: vec!["layout\tsizes\t16\t4\tthree".to_string()],
        drop: vec![("layout", "sizes")],
        ..IcsFile::new(&["x", "y"], &[4, 3], 16)
    };
    let message = refusal_text(&file, "not_a_number");
    assert!(message.contains("not a count"), "{message}");
    assert!(message.contains("three"), "{message}");
}

#[test]
fn a_negative_size_is_refused_without_a_huge_count() {
    let file = IcsFile {
        drop: vec![("layout", "sizes")],
        extra: vec!["layout\tsizes\t16\t4\t-3".to_string()],
        ..IcsFile::new(&["x", "y"], &[4, 3], 16)
    };
    let message = refusal_text(&file, "negative_size");
    assert!(message.contains("cannot be negative"), "{message}");
    assert!(!message.contains("1844674407370955"), "{message}");
}

#[test]
fn a_layout_parameters_that_disagrees_is_refused() {
    for parameters in [1usize, 2, 4, 9] {
        let file = IcsFile {
            parameters: Some(parameters),
            ..IcsFile::new(&["x", "y", "z", "c", "t"], &[5, 3, 4, 3, 2], 16)
        };
        let message = refusal_text(&file, &format!("parameters_{parameters}"));
        assert!(
            message.contains(&format!("layout parameters is {parameters}")),
            "{message}"
        );
        // A header that disagrees with itself is a Parse refusal, not an unknown feature.
        let path = scratch(&format!("kind_{parameters}"));
        file.write(&path);
        assert!(
            matches!(IcsReader::open(&path), Err(Error::Parse(_))),
            "parameters {parameters} is a Parse refusal"
        );
    }
}

/// The header without its `end` line: what a file that stops before the end of its header looks like.
fn header_without_end(file: &IcsFile) -> Vec<u8> {
    let mut bytes = file.header();
    let end_at = bytes
        .windows(5)
        .position(|w| w == b"\nend\n")
        .expect("the header has an end line");
    bytes.truncate(end_at);
    bytes
}

#[test]
fn a_header_that_ends_without_its_end_line_is_refused() {
    // The file ends where the header does, so the read ends with it.
    let bytes = header_without_end(&IcsFile::new(&["x", "y"], &[4, 3], 16));
    let path = scratch("no_end");
    std::fs::write(&*path, &bytes).expect("write the headerless ICS");
    match IcsReader::open(&path) {
        Err(Error::Unsupported(message)) => {
            assert!(message.contains("the header has no `end`"), "{message}");
            assert!(
                !message.contains("1048576"),
                "the cap is a different refusal: {message}"
            );
        }
        Err(other) => panic!("expected Unsupported, got {other:?}"),
        Ok(_) => panic!("expected a refusal"),
    }
}

#[test]
fn a_header_past_the_cap_without_its_end_line_is_refused_by_the_cap() {
    // One MiB of header lines with no `end`: the cap stops the read, so the file is never read whole.
    let mut bytes = header_without_end(&IcsFile::new(&["x", "y"], &[4, 3], 16));
    bytes.resize(1024 * 1024 + 1, b'x');
    let path = scratch("past_the_cap");
    std::fs::write(&*path, &bytes).expect("write the over-long ICS");
    match IcsReader::open(&path) {
        Err(Error::Unsupported(message)) => {
            assert!(
                message.contains("no `end` in its first 1048576 bytes"),
                "{message}"
            );
        }
        Err(other) => panic!("expected Unsupported, got {other:?}"),
        Ok(_) => panic!("expected a refusal"),
    }
}

#[test]
fn a_missing_ids_sibling_is_refused_with_the_file_name() {
    let file = IcsFile {
        version_text: "1.0".to_string(),
        ..IcsFile::new(&["x", "y"], &[4, 3], 16)
    };
    let path = scratch("alone");
    std::fs::write(&*path, file.header()).expect("write the .ics alone");
    match IcsReader::open(&path) {
        Err(Error::Unsupported(message)) => {
            assert!(
                message.contains("alone"),
                "the message names the file: {message}"
            );
            assert!(
                message.contains(".ids"),
                "the message names the extension: {message}"
            );
        }
        Err(other) => panic!("expected Unsupported, got {other:?}"),
        Ok(_) => panic!("expected a refusal"),
    }
}

#[test]
fn an_ids_file_without_its_ics_is_refused_with_the_ics_name() {
    let dir = support::scratch::ScratchDir::new(
        std::env::temp_dir().join(format!("phasit_ics_lonely_ids_{}", std::process::id())),
    );
    let _ = std::fs::remove_dir_all(&*dir);
    std::fs::create_dir_all(&*dir).expect("create the scratch folder");
    let ids = dir.join("lonely.ids");
    std::fs::write(&*ids, vec![0u8; 32]).expect("write the .ids alone");
    match IcsReader::open(&ids) {
        Err(Error::Unsupported(message)) => assert!(message.contains(".ics"), "{message}"),
        Err(other) => panic!("expected Unsupported, got {other:?}"),
        Ok(_) => panic!("expected a refusal"),
    }
}

#[test]
fn an_unsupported_sample_type_is_refused_by_name() {
    for (format, sign, bits) in [
        ("integer", "signed", 8),
        ("integer", "unsigned", 32),
        ("integer", "signed", 32),
        ("real", "signed", 16),
        ("complex", "signed", 32),
    ] {
        let order = if bits > 16 { "1 2 3 4 5 6 7 8" } else { "1" };
        let file = IcsFile {
            format: format.to_string(),
            sign: sign.to_string(),
            byte_order: Some(order.to_string()),
            ..IcsFile::new(&["x", "y"], &[4, 3], bits)
        };
        let message = refusal_text(&file, &format!("{format}_{sign}_{bits}"));
        assert!(message.contains(&format!("{bits} bits")), "{message}");
        assert!(message.contains("cannot represent"), "{message}");
    }
}

#[test]
fn an_unknown_compression_is_refused_by_name() {
    let file = IcsFile {
        compression: "compress".to_string(),
        ..IcsFile::new(&["x", "y"], &[4, 3], 16)
    };
    let message = refusal_text(&file, "compress");
    assert!(message.contains("compress"), "{message}");
    assert!(message.contains("not one this reader decodes"), "{message}");
}

#[test]
fn a_byte_order_that_is_no_permutation_is_refused() {
    for order in ["2 2", "1 3", "0 1", "2 1 3"] {
        let file = IcsFile {
            byte_order: Some(order.to_string()),
            ..IcsFile::new(&["x", "y"], &[4, 3], 16)
        };
        let message = refusal_text(&file, &format!("order_{}", order.replace(' ', "_")));
        assert!(message.contains("neither 1..2"), "{order}: {message}");
    }
}

#[test]
fn a_wide_sample_without_a_byte_order_is_refused() {
    let file = IcsFile {
        byte_order: None,
        ..IcsFile::new(&["x", "y"], &[4, 3], 16)
    };
    let message = refusal_text(&file, "no_byte_order");
    assert!(message.contains("no byte_order"), "{message}");
    assert!(message.contains("2-byte"), "{message}");
}

#[test]
fn a_gzip_plane_that_is_not_contiguous_is_refused() {
    // `ch x y` gathers, `x ch y` has x first but not y second: both are refused, only the second is near enough to
    // be a surprise.
    for (axes, sizes, name) in [
        (
            vec!["ch", "x", "y", "z", "t"],
            vec![3, 5, 3, 4, 2],
            "ch_x_y",
        ),
        (vec!["x", "ch", "y", "z"], vec![5, 3, 3, 4], "x_ch_y"),
        (vec!["y", "x", "z"], vec![3, 5, 2], "y_x"),
    ] {
        let file = IcsFile {
            compression: "gzip".to_string(),
            ..IcsFile::new(&axes, &sizes, 16)
        };
        let message = refusal_text(&file, name);
        assert!(
            message.contains("not x then y"),
            "the sentence says what is checked: {message}"
        );
    }
}

/// The sentence a malformed header line is refused with, through the public entry point.
fn refusal_of(line: &str, drop: &[(&'static str, &'static str)]) -> String {
    let file = IcsFile {
        drop: drop.to_vec(),
        extra: vec![line.to_string()],
        ..IcsFile::new(&["x", "y"], &[4, 3], 16)
    };
    let path = scratch(&line.replace('\t', "_"));
    file.write(&path);
    match phasit_imread::open(&path) {
        Err(Error::Parse(message)) => message,
        Err(other) => panic!("expected a Parse refusal, got {other:?}"),
        Ok(_) => panic!("the file opened"),
    }
}

#[test]
fn a_layout_order_line_without_values_is_refused() {
    let message = refusal_of("layout\torder", &[("layout", "order")]);
    assert!(message.contains("layout order"), "{message}");
    assert!(message.contains("states no value"), "{message}");
}

#[test]
fn a_bare_parameter_line_is_refused() {
    let message = refusal_of("parameter", &[]);
    assert!(message.contains("parameter"), "{message}");
    assert!(message.contains("states no value"), "{message}");
}

#[test]
fn a_parameter_units_line_without_values_is_refused() {
    let message = refusal_of("parameter\tunits", &[("parameter", "units")]);
    assert!(message.contains("parameter units"), "{message}");
    assert!(message.contains("states no value"), "{message}");
}

#[test]
fn a_parameter_scale_line_without_values_is_refused() {
    let message = refusal_of("parameter\tscale", &[("parameter", "scale")]);
    assert!(message.contains("parameter scale"), "{message}");
    assert!(message.contains("states no value"), "{message}");
}

#[test]
fn data_past_the_end_of_the_file_is_refused() {
    let file = IcsFile::new(&["x", "y", "z", "c", "t"], &[5, 3, 4, 3, 2], 16);
    let path = scratch("truncated");
    let mut bytes = file.bytes();
    bytes.truncate(bytes.len() - 1);
    std::fs::write(&*path, &bytes).expect("write the truncated ICS");
    match IcsReader::open(&path) {
        Err(Error::Parse(message)) => assert!(message.contains("past the end of the"), "{message}"),
        Err(other) => panic!("expected Parse, got {other:?}"),
        Ok(_) => panic!("expected Parse, got a reader"),
    }
}

#[test]
fn a_plane_outside_the_scene_is_refused() {
    let path = scratch("coords");
    IcsFile::stack(4, 3, 2, 1, 1).write(path.deref());
    let reader = IcsReader::open(&path).expect("open the synthetic ICS");
    assert!(matches!(
        reader.plane(0, 0, 0, 2),
        Err(Error::OutOfRange { z: 2, .. })
    ));
    assert!(matches!(
        reader.plane(1, 0, 0, 0),
        Err(Error::SceneOutOfRange { scene: 1, count: 1 })
    ));
}

#[test]
fn a_plane_over_the_decode_ceiling_is_refused_before_it_is_allocated() {
    // A deflated stream has no byte length to check at open, so a header may claim a plane of 4.9 GB: `plane()` is
    // where the ceiling stops it, before the first byte is reserved.
    let file = IcsFile {
        compression: "gzip".to_string(),
        ..IcsFile::new(&["x", "y"], &[70_000, 70_000], 8)
    };
    let path = scratch("over_ceiling");
    // The header alone: the reader stops at the ceiling before it reads a byte of pixel data.
    std::fs::write(&*path, file.header()).expect("write the over-ceiling ICS");
    let reader = IcsReader::open(&path).expect("a gzip file has no length to check at open");
    let error = reader
        .plane(0, 0, 0, 0)
        .expect_err("an over-ceiling plane is refused");
    assert!(
        matches!(error, Error::OversizedAllocation { .. }),
        "expected OversizedAllocation, got {error:?}"
    );
}
