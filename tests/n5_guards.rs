// SPDX-License-Identifier: Apache-2.0
//! The N5 guards that need a file of their own on disk: detection agrees with the reader about how much of
//! `attributes.json` may be read, a block file is read no further than its header allows, and a compressed
//! stream is held to the block's own byte count.

mod support;

use phasit_imread::n5_reader::N5Reader;
use phasit_imread::{detect_format, open, DType, Error, Format, ImageReader};

use support::n5::N5Dataset;

/// The sentence a plane read is refused with, whatever its kind.
fn plane_refusal(dataset: &N5Dataset) -> String {
    match N5Reader::open(dataset)
        .expect("the dataset opens")
        .plane(0, 0, 0, 0)
    {
        Err(error) => error.to_string(),
        Ok(_) => panic!("the plane decoded"),
    }
}

/// A 4 x 3 uint8 dataset whose `attributes.json` carries one ignored member of `length` bytes.
fn padded(name: &str, length: usize) -> N5Dataset {
    N5Dataset::at(
        name,
        &format!(
            r#"{{"dimensions":[4,3],"blockSize":[2,2],"dataType":"uint8","notes":"{}"}}"#,
            "a".repeat(length)
        ),
    )
}

#[test]
fn a_large_but_valid_attributes_json_is_detected_and_opens() {
    // An annotated or pretty-printed attributes.json is a dataset too: the probe reads as far as the reader
    // itself does, and a file past the reader's cap is refused by the probe exactly as by the reader.
    let six_kb = padded("guards_detect_big", 6 * 1024);
    assert_eq!(
        detect_format(&six_kb).expect("detect the padded dataset"),
        Format::N5
    );
    assert_eq!(
        open(&six_kb).expect("open the padded dataset").meta().scene_count(),
        1
    );
    let huge = padded("guards_detect_huge", 1_048_578);
    assert!(matches!(
        detect_format(&huge),
        Err(Error::OversizedAllocation { .. })
    ));
    assert!(matches!(
        N5Reader::open(&huge),
        Err(Error::OversizedAllocation { .. })
    ));
}
