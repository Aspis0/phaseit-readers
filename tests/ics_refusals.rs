// SPDX-License-Identifier: Apache-2.0
//! Every way an ICS file is refused: the version, the axis order, the sample type, the compression, the byte order,
//! a header without `end`, a `layout parameters` that disagrees with the layout, a missing `.ids` sibling, a deflated
//! plane that is not contiguous and data that runs past the end of the file.
//! The files are written by `tests/support/ics.rs` and removed by `Scratch`, so a failing assertion leaves nothing.

mod support;

use phasit_imread::ics_reader::IcsReader;
use phasit_imread::{Error, ImageReader};

use std::ops::Deref;

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

fn refusal_kind(file: &IcsFile, name: &str) -> String {
    let path = scratch(name);
    file.write(&path);
    match IcsReader::open(&path) {
        Err(Error::Unsupported(_)) => "unsupported".to_string(),
        Err(Error::Parse(_)) => "parse".to_string(),
        Err(other) => panic!("expected a refusal, got {other:?}"),
        Ok(_) => panic!("the file opened"),
    }
}

#[test]
fn an_unknown_version_is_refused() {
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
fn an_unknown_axis_is_refused_by_name() {
    let file = IcsFile::new(&["x", "y", "q"], &[4, 3, 2], 16);
    let message = refusal_text(&file, "unknown_axis");
    assert!(message.contains("axis \"q\""), "{message}");
}

#[test]
fn a_repeated_axis_is_refused_by_name() {
    let file = IcsFile::new(&["x", "y", "x"], &[4, 3, 2], 16);
    let message = refusal_text(&file, "repeated_axis");
    assert!(message.contains("more than once"), "{message}");
}

#[test]
fn a_missing_x_or_y_is_refused_by_name() {
    for axes in [&["y", "z"][..], &["x", "z"][..]] {
        let file = IcsFile::new(axes, &[4, 3], 16);
        let name = format!("no_{}", axes[0]);
        let message = refusal_text(&file, &name);
        assert!(message.contains("does not name axis"), "{message}");
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
        assert_eq!(refusal_kind(&file, &format!("kind_{parameters}")), "parse");
    }
}

#[test]
fn a_header_without_end_inside_the_cap_is_refused() {
    let file = IcsFile::new(&["x", "y"], &[4, 3], 16);
    let mut bytes = file.header();
    // Drop the `end` line: the header ends unterminated, so no `end` can be found.
    let end_at = bytes
        .windows(5)
        .position(|w| w == b"\nend\n")
        .expect("the header has an end line");
    bytes.truncate(end_at);
    let path = scratch("no_end");
    std::fs::write(&*path, &bytes).expect("write the headerless ICS");
    match IcsReader::open(&path) {
        Err(Error::Unsupported(message)) => assert!(message.contains("`end`"), "{message}"),
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
        let file = IcsFile {
            format: format.to_string(),
            sign: sign.to_string(),
            byte_order: if bits > 16 {
                Some("1 2 3 4 5 6 7 8".to_string())
            } else {
                Some("1".to_string())
            },
            ..IcsFile::new(&["x", "y"], &[4, 3], bits)
        };
        let name = format!("{format}_{sign}_{bits}");
        let message = refusal_text(&file, &name);
        assert!(
            message.contains(&format!("{bits} bits per sample")),
            "{name}: {message}"
        );
        assert!(message.contains("cannot represent"), "{name}: {message}");
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
    let file = IcsFile {
        compression: "gzip".to_string(),
        ..IcsFile::new(&["ch", "x", "y", "z", "t"], &[3, 5, 3, 4, 2], 16)
    };
    let message = refusal_text(&file, "gz_gather");
    assert!(message.contains("not contiguous"), "{message}");
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
