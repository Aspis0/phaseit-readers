// SPDX-License-Identifier: Apache-2.0
//! Where the pixels start: a header whose `end` line ends a read chunk must not shift the data by a byte.
//! The header is read in 32 KiB chunks, so the offset has to come from the bytes that were actually seen.
//! The files are written by `tests/support/ics.rs` and removed by `Scratch`.

mod support;

use phasit_imread::ics_reader::IcsReader;
use phasit_imread::{ImageReader, PlaneBuf};

use support::ics::{sample_at, IcsFile};
use support::scratch::Scratch;

fn scratch(name: &str) -> Scratch {
    Scratch::new(std::env::temp_dir().join(format!(
        "phasit_ics_offset_{name}_{}.ics",
        std::process::id()
    )))
}

/// The file's bytes with `end` moved so it starts at `at`, by a `history` line of filler.
fn padded_end(file: &IcsFile, at: usize) -> Vec<u8> {
    let base = file.header();
    let before = base
        .windows(5)
        .position(|w| w == b"\nend\n")
        .expect("the header has an end")
        + 1;
    let filler = at - before - b"history\t".len() - 1;
    let mut out = base[..before].to_vec();
    out.extend_from_slice(b"history\t");
    out.extend(std::iter::repeat_n(b'x', filler));
    out.push(b'\n');
    out.extend_from_slice(b"end\n");
    out.extend_from_slice(&file.pixel_bytes());
    out
}

#[test]
fn the_end_line_may_end_a_read_chunk_and_the_pixels_still_start_after_it() {
    // A 2 x 2 U8 file: the samples are 0, 1, 10, 11, so a one-byte shift shows in the plane.
    let file = IcsFile {
        bits: 8,
        byte_order: Some("1".to_string()),
        ..IcsFile::new(&["x", "y", "z"], &[2, 2, 1], 8)
    };
    let expected: Vec<u8> = (0..2)
        .flat_map(|row| (0..2).map(move |col| sample_at(0, row, col) as u8))
        .collect();
    // A chunk holds bytes 0..32767 and 32768..65535, so an `end` line starting three bytes before a bound has its
    // separator in the next chunk: the data offset must wait for that separator instead of counting it early.
    for at in [
        32763, 32764, 32765, 32766, 32767, 65531, 65532, 65533, 65534, 65535,
    ] {
        let path = scratch(&format!("boundary_{at}"));
        std::fs::write(&*path, padded_end(&file, at)).expect("write the padded ICS");
        let reader = IcsReader::open(&path).expect("open the padded ICS");
        match reader.plane(0, 0, 0, 0).expect("read the plane").data {
            PlaneBuf::U8(values) => assert_eq!(values, expected, "the end line starts at {at}"),
            other => panic!("expected U8, got {other:?}"),
        }
    }
}
