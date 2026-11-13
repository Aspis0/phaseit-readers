// SPDX-License-Identifier: Apache-2.0
//! Every way a MetaMorph STK is refused: the compression, the samples per pixel, the sample width and format, the
//! strips of plane 0 and the two places a plane count can run past the end of the file. The files are written by
//! `tests/support/stk.rs` and removed by `Scratch`, so a failing assertion leaves nothing behind.

mod support;

use phasit_imread::stk_reader::StkReader;
use phasit_imread::{Error, ImageReader};

use support::scratch::Scratch;
use support::stk::{StkFile, Uic2Type};

fn scratch(name: &str) -> Scratch {
    Scratch::new(std::env::temp_dir().join(format!("phasit_stk_{name}_{}.stk", std::process::id())))
}

/// The sentence a file is refused with, whatever the kind: an unknown feature is `Unsupported`, a header that
/// disagrees with itself is `Parse`.
fn refusal(file: &StkFile, name: &str) -> String {
    let path = scratch(name);
    file.write(&path);
    match StkReader::open(&path) {
        Err(Error::Unsupported(message)) | Err(Error::Parse(message)) => message,
        Err(other) => panic!("expected a refusal, got {other:?}"),
        Ok(_) => panic!("the file opened"),
    }
}

#[test]
fn a_compressed_file_is_refused() {
    let file = StkFile {
        compression: 8,
        ..StkFile::u16(5, 3, 2)
    };
    let message = refusal(&file, "compressed");
    assert!(message.contains("compression"), "{message}");
    assert!(message.contains('8'), "{message}");
}

#[test]
fn more_than_one_sample_per_pixel_is_refused() {
    let file = StkFile {
        samples_per_pixel: 3,
        ..StkFile::u16(5, 3, 2)
    };
    let message = refusal(&file, "samples");
    assert!(message.contains("samples per pixel"), "{message}");
    assert!(message.contains('3'), "{message}");
}

#[test]
fn a_sample_width_other_than_8_or_16_is_refused() {
    let file = StkFile {
        bits_tag: Some(12),
        ..StkFile::u16(5, 3, 2)
    };
    let message = refusal(&file, "bits");
    assert!(message.contains("12 bits"), "{message}");
}

#[test]
fn a_signed_or_float_sample_format_is_refused() {
    for format in [2, 3] {
        let file = StkFile {
            sample_format: format,
            ..StkFile::u16(5, 3, 2)
        };
        let message = refusal(&file, "sample_format");
        assert!(message.contains("sample format"), "{message}");
        assert!(message.contains("unsigned 8- and 16-bit"), "{message}");
    }
}

#[test]
fn strips_of_plane_0_that_do_not_touch_are_refused() {
    let file = StkFile {
        rows_per_strip: Some(1),
        strip_gap: 4,
        ..StkFile::u16(5, 3, 2)
    };
    let message = refusal(&file, "gapped_strips");
    assert!(message.contains("not contiguous"), "{message}");
}

#[test]
fn a_uic2_of_no_plane_is_refused() {
    let file = StkFile {
        uic2_count: Some(0),
        ..StkFile::u16(5, 3, 2)
    };
    let message = refusal(&file, "no_planes");
    assert!(message.contains("no plane"), "{message}");
}

#[test]
fn uic2_records_outside_the_file_are_refused() {
    let file = StkFile {
        uic2_at: Some(1 << 20),
        ..StkFile::u16(5, 3, 2)
    };
    let message = refusal(&file, "uic2_past_end");
    assert!(message.contains("past the end"), "{message}");
}

#[test]
fn planes_past_the_end_of_the_file_are_refused() {
    let file = StkFile {
        planes_written: Some(1),
        ..StkFile::u16(5, 3, 4)
    };
    let message = refusal(&file, "planes_past_end");
    assert!(message.contains("past the end"), "{message}");
    assert!(message.contains("plane(s)"), "{message}");
}

#[test]
fn a_uic2_table_shorter_than_its_plane_records_is_refused() {
    // Six uint32 per plane, whatever type the tag declares: 8, 16 and 32 bytes are all short of the 96 a 4-plane file
    // needs, and they sit where the file ends so nothing else can stand in for them.
    for (kind, present) in [
        (Uic2Type::Rational, 8),
        (Uic2Type::Long, 16),
        (Uic2Type::Rational, 32),
    ] {
        let mut file = StkFile {
            uic2_type: kind,
            ..StkFile::u16(5, 3, 4)
        };
        file.uic2_at = Some((file.bytes().len() - present) as u32);
        let message = refusal(&file, "short_uic2");
        assert!(message.contains("records"), "{message}");
        assert!(message.contains("past the end"), "{message}");
    }
}
