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

/// The block at the first grid position holds `header` and nothing else, with the file extended to `length`
/// bytes (a sparse file: the bytes were never written, the length still claims them).
fn sparse_block(name: &str, header: Vec<u8>, length: u64) -> N5Dataset {
    let dataset = N5Dataset::new(name, &[4, 3, 2], &[2, 2, 1], DType::U8, Some("raw"));
    dataset.write_bytes(&[0, 0, 0], &header);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(dataset.block_path(&[0, 0, 0]))
        .expect("open the block file");
    file.set_len(length).expect("extend the block file");
    dataset
}

#[test]
fn a_block_file_past_its_payload_is_refused_after_its_header_not_after_the_gigabyte() {
    // The header alone decides whether the rest of the file is worth reading, so a block file that claims a
    // gigabyte is refused in well under a second, whether its header is valid or not.
    let mut bad = vec![1u16, 3u16]
        .iter()
        .flat_map(|v| v.to_be_bytes())
        .collect::<Vec<u8>>();
    bad.extend([2u32, 2, 1].iter().flat_map(|v| v.to_be_bytes()));
    let valid = [0u16, 3u16]
        .iter()
        .flat_map(|v| v.to_be_bytes())
        .chain([2u32, 2, 1].iter().flat_map(|v| v.to_be_bytes()))
        .collect();
    for (name, header, sentence) in [
        ("guards_sparse_bad", bad, "mode is 1"),
        (
            "guards_sparse_valid",
            valid,
            "requested 1073741824 bytes, cap 20 bytes",
        ),
    ] {
        let dataset = sparse_block(name, header, 1 << 30);
        let start = std::time::Instant::now();
        let message = plane_refusal(&dataset);
        let elapsed = start.elapsed();
        eprintln!("{name}: refused in {elapsed:?}: {message}");
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "{name}: {elapsed:?}"
        );
        assert!(message.contains(sentence), "{name}: {message}");
    }
}

#[test]
fn a_compressed_stream_of_another_length_is_refused_by_the_length_guard() {
    // A stream that decodes whole but to another byte count than the block's own is refused by the length
    // guard itself, not by a broken decoder: one byte less than the block, and one KiB past it.
    for compression in ["gzip", "zstd"] {
        let dataset = N5Dataset::new(
            "stream_len",
            &[4, 3, 2],
            &[2, 2, 1],
            DType::U16,
            Some(compression),
        );
        let short = vec![7u8; 7];
        dataset.write_stream(&[0, 0, 0], &[2, 2, 1], &short);
        let message = plane_refusal(&dataset);
        assert!(message.contains("inflates to 7"), "{compression}: {message}");
        let long = vec![7u8; 8 + 1024];
        dataset.write_stream(&[0, 0, 0], &[2, 2, 1], &long);
        let message = plane_refusal(&dataset);
        assert!(message.contains("inflates to 9"), "{compression}: {message}");
    }
}
