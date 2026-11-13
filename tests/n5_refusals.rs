// SPDX-License-Identifier: Apache-2.0
//! Every way an N5 dataset is refused: a path that is no dataset, a rank v1 does not read, a sample type or
//! compression it does not decode, an attribute that is not a count, a block mode other than the default, a block
//! header that disagrees with the dataset, a payload that is not the block's own size, and a plane outside the
//! scene. Each dataset is written by `tests/support/n5.rs` and removed by its `Drop`.

mod support;

use phasit_imread::n5_reader::N5Reader;
use phasit_imread::{detect_format, DType, Error, Format, ImageReader};

use support::n5::N5Dataset;

/// The sentence a dataset is refused with at open, whatever the kind.
fn open_refusal(dataset: &N5Dataset) -> String {
    match N5Reader::open(dataset) {
        Err(Error::Unsupported(message)) | Err(Error::Parse(message)) => message,
        Err(other) => panic!("expected a refusal, got {other:?}"),
        Ok(_) => panic!("the dataset opened"),
    }
}

/// The sentence a plane read is refused with (the dataset itself opens).
fn plane_refusal(dataset: &N5Dataset) -> String {
    let reader = N5Reader::open(dataset).expect("the dataset opens");
    match reader.plane(0, 0, 0, 0) {
        Err(Error::Unsupported(message))
        | Err(Error::Parse(message))
        | Err(Error::Decode(message)) => message,
        Err(other) => panic!("expected a refusal, got {other:?}"),
        Ok(_) => panic!("the plane decoded"),
    }
}

/// A 4 x 3 x 2 uint8 dataset, block (2, 2, 1), compressed as `compression` states it.
fn dataset(name: &str, compression: &'static str) -> N5Dataset {
    N5Dataset::new(name, &[4, 3, 2], &[2, 2, 1], DType::U8, Some(compression))
}

/// A 4 x 3 uint8 dataset whose `attributes.json` holds exactly the JSON given.
fn with_attributes(name: &str, json: &str) -> N5Dataset {
    N5Dataset::at(name, json)
}

/// A dataset whose `attributes.json` states exactly what the case varies: dimensions and block as given, the rest
/// the 4 x 3 uint8 raw dataset's own.
fn varying(name: &str, dimensions: &str, block: &str, extra: &str) -> N5Dataset {
    with_attributes(
        name,
        &format!(r#"{{"dimensions":{dimensions},"blockSize":{block},"dataType":"uint8"{extra}}}"#),
    )
}

#[test]
fn a_path_that_is_not_a_dataset_is_refused() {
    let dir = std::env::temp_dir().join(format!("phasit_n5_refuse_path_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the scratch folder");
    let image = dir.join("image.tif");
    std::fs::write(&image, b"II\x2A\x00").expect("write the file");
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).expect("create the folder");
    for path in [&image, &empty] {
        assert!(
            !matches!(detect_format(path), Ok(Format::N5)),
            "{} is not detected as N5",
            path.display()
        );
        match N5Reader::open(path) {
            Err(Error::Io(_)) | Err(Error::Parse(_)) => {}
            Err(Error::Unsupported(message)) => {
                assert!(message.contains("attributes.json"), "{message}")
            }
            Err(other) => panic!("{}: {other}", path.display()),
            Ok(_) => panic!("{} opened", path.display()),
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_dispatcher_routes_an_n5_dataset_to_the_n5_reader() {
    let dataset = dataset("detected", "raw");
    assert_eq!(detect_format(&dataset).expect("the directory"), Format::N5);
    assert_eq!(
        detect_format(&dataset.attributes_path()).expect("the file"),
        Format::N5
    );
    assert_eq!(
        phasit_imread::open(&dataset)
            .expect("open through the dispatcher")
            .meta()
            .scene_count(),
        1
    );
}

#[test]
fn a_rank_other_than_two_or_three_is_refused_by_name() {
    for (dimensions, block) in [
        ("[]", "[]"),
        ("[4]", "[2]"),
        ("[4,3,2,1]", "[2,2,2,2]"),
        ("[4,3,2,1,1]", "[2,2,2,2,2]"),
    ] {
        let dataset = varying(
            "rank",
            dimensions,
            block,
            r#","compression":{"type":"raw"}"#,
        );
        let message = open_refusal(&dataset);
        assert!(
            message.contains("2 dimensions") && message.contains("3 dimensions"),
            "{dimensions}: {message}"
        );
        let axes = dimensions.matches(',').count() + usize::from(dimensions != "[]");
        assert!(
            message.contains(&format!("this one has {axes}")),
            "{dimensions}: {message}"
        );
    }
}

#[test]
fn a_sample_type_this_reader_does_not_model_is_refused_by_name() {
    for data_type in ["\"uint32\"", "\"int8\"", "\"float64\"", "\"complex\"", "8"] {
        let dataset = varying(
            "dtype",
            "[4,3]",
            "[2,2]",
            &format!(r#","dataType":{data_type}"#),
        );
        let message = open_refusal(&dataset);
        assert!(message.contains("dataType"), "{data_type}: {message}");
        assert!(
            message.contains("uint8, uint16, int16 and float32"),
            "{data_type}: {message}"
        );
    }
    let missing = with_attributes(
        "no_dtype",
        r#"{"dimensions":[4,3],"blockSize":[2,2],"compression":{"type":"raw"}}"#,
    );
    assert!(open_refusal(&missing).contains("no dataType"));
}

#[test]
fn a_compression_this_reader_does_not_decode_is_refused_by_name() {
    for compression in ["blosc", "lz4", "xz", "bzip2"] {
        let dataset = dataset("compression", compression);
        let message = open_refusal(&dataset);
        assert!(message.contains(compression), "{compression}: {message}");
        assert!(
            message.contains("not one this reader decodes"),
            "{compression}: {message}"
        );
    }
    let mut legacy = N5Dataset::new("legacy_blosc", &[4, 3, 2], &[2, 2, 1], DType::U8, None);
    legacy.compress_legacy("blosc");
    assert!(open_refusal(&legacy).contains("blosc"));
    // A dataset that states no compression holds raw blocks: N5's own default.
    let unstated = with_attributes(
        "raw_default",
        r#"{"dimensions":[4,3],"blockSize":[2,2],"dataType":"uint8"}"#,
    );
    let reader = N5Reader::open(&unstated).expect("a dataset that states no compression is raw");
    assert_eq!(reader.plane(0, 0, 0, 0).expect("the plane").shape, [3, 4]);
}

#[test]
fn an_attribute_that_is_not_a_whole_positive_count_is_refused() {
    for (name, dimensions, block, reason) in [
        ("zero_extent", "[4,0]", "[2,2]", "dimensions[1]"),
        ("negative", "[4,-2]", "[2,2]", "dimensions[1]"),
        ("fractional", "[4,1.5]", "[2,2]", "dimensions[1]"),
        ("zero_block", "[4,3]", "[0,2]", "blockSize[0]"),
        ("block_fraction", "[4,3]", "[2,1.5]", "blockSize[1]"),
    ] {
        let dataset = varying(name, dimensions, block, r#","compression":{"type":"raw"}"#);
        let message = open_refusal(&dataset);
        assert!(message.contains(reason), "{name}: {message}");
        assert!(
            message.contains("whole number from 1 to"),
            "{name}: {message}"
        );
    }
    let text = varying(
        "text",
        "[4,\"3\"]",
        "[2,2]",
        r#","compression":{"type":"raw"}"#,
    );
    assert!(open_refusal(&text).contains("dimensions[1] is not a number"));
    let huge = varying(
        "over_the_scene_bound",
        "[4,1000001]",
        "[2,2]",
        r#","compression":{"type":"raw"}"#,
    );
    let message = open_refusal(&huge);
    assert!(
        message.contains("dimensions[1]") && message.contains("1000001"),
        "{message}"
    );
}

#[test]
fn attributes_without_their_dimensions_or_block_size_are_refused() {
    for (name, attributes) in [
        (
            "no_dimensions",
            r#"{"blockSize":[2,2],"dataType":"uint8","compression":{"type":"raw"}}"#,
        ),
        (
            "no_block",
            r#"{"dimensions":[4,3],"dataType":"uint8","compression":{"type":"raw"}}"#,
        ),
        (
            "dimensions_not_an_array",
            r#"{"dimensions":4,"blockSize":[2,2],"dataType":"uint8","compression":{"type":"raw"}}"#,
        ),
    ] {
        let dataset = with_attributes(name, attributes);
        let message = open_refusal(&dataset);
        assert!(message.contains("state no"), "{name}: {message}");
    }
}

#[test]
fn attributes_that_are_not_a_json_object_are_refused_as_malformed() {
    for (name, attributes) in [
        ("empty", ""),
        ("truncated", r#"{"dimensions":[4,3]"#),
        ("array", "[]"),
    ] {
        let dataset = with_attributes(name, attributes);
        let message = open_refusal(&dataset);
        assert!(message.contains("attributes.json"), "{name}: {message}");
        assert!(
            message.contains("not valid JSON") || message.contains("JSON object"),
            "{name}: {message}"
        );
    }
}

#[test]
fn a_block_mode_other_than_the_default_is_refused_by_mode() {
    for mode in [1u16, 2, 7] {
        let dataset = dataset("mode", "raw");
        let mut block: Vec<u8> = mode.to_be_bytes().to_vec();
        block.extend(3u16.to_be_bytes());
        block.extend([2u32, 2, 2].iter().flat_map(|v| v.to_be_bytes()));
        dataset.write_bytes(&[0, 0, 0], &block);
        let message = plane_refusal(&dataset);
        assert!(
            message.contains(&format!("mode is {mode}")),
            "mode {mode}: {message}"
        );
        assert!(
            message.contains("not the default (0)"),
            "mode {mode}: {message}"
        );
    }
}

#[test]
fn a_block_header_that_disagrees_with_the_dataset_is_refused() {
    // Another rank, a zero extent, one above the block size and one past the dataset edge.
    for (name, ndim, size, kind) in [
        ("rank", 2u16, vec![2u32, 1], "dimension(s)"),
        ("zero", 3, vec![0, 2, 1], "axis 0"),
        ("over_block", 3, vec![3, 2, 1], "axis 0"),
    ] {
        let dataset = dataset(name, "raw");
        let message = header_refusal(&dataset, ndim, &size);
        assert!(message.contains(kind), "{name}: {message}");
    }
    // A block side may exceed the dataset edge only where the edge is the smaller of the two.
    let dataset = varying(
        "edge",
        "[1,3,2]",
        "[2,2,1]",
        r#","compression":{"type":"raw"}"#,
    );
    let message = header_refusal(&dataset, 3, &[2, 2, 1]);
    assert!(message.contains("axis 0"), "{message}");
}

/// A block at the first grid position whose header states `ndim` and `size`, with no payload of its own.
fn header_refusal(dataset: &N5Dataset, ndim: u16, size: &[u32]) -> String {
    let mut block: Vec<u8> = [0u16, ndim].iter().flat_map(|v| v.to_be_bytes()).collect();
    block.extend(size.iter().flat_map(|v| v.to_be_bytes()));
    dataset.write_bytes(&[0, 0, 0], &block);
    plane_refusal(dataset)
}

#[test]
fn a_block_header_shorter_than_its_own_rank_is_refused() {
    let dataset = dataset("short_header", "raw");
    for header in [
        &[][..],
        &[0, 0][..],
        &[0, 0, 0][..],
        &[0, 0, 0, 3][..],
        &[0, 0, 0, 3, 0, 0][..],
    ] {
        dataset.write_bytes(&[0, 0, 0], header);
        let message = plane_refusal(&dataset);
        assert!(message.contains("stops before"), "{header:?}: {message}");
    }
}

#[test]
fn a_raw_payload_that_is_not_the_blocks_own_size_is_refused() {
    // A raw block of 2x1 u8 holds 2 bytes: none, one too few and one too many are all refused.
    let dataset = dataset("payload", "raw");
    for payload in [&[][..], &[1][..], &[1, 2, 3][..]] {
        dataset.write_raw(&[0, 0, 0], &[2, 1, 1], payload);
        let message = plane_refusal(&dataset);
        assert!(message.contains("not the 2"), "{payload:?}: {message}");
        assert!(message.contains("raw"), "{payload:?}: {message}");
    }
}

#[test]
fn a_gzip_or_zstd_payload_of_another_length_is_refused() {
    for compression in ["gzip", "zstd"] {
        let dataset = N5Dataset::new(
            &format!("{compression}_len"),
            &[4, 3, 2],
            &[2, 2, 1],
            DType::U16,
            Some(compression),
        );
        dataset.write_block(&[0, 0, 0], &[2, 2, 1], DType::U16);
        let path = dataset.block_path(&[0, 0, 0]);
        let mut bytes = std::fs::read(&path).expect("read the block");
        bytes.truncate(bytes.len() - 1);
        std::fs::write(&path, &bytes).expect("shorten the block");
        let message = plane_refusal(&dataset);
        assert!(
            message.contains("not the 8") || message.contains("broken"),
            "{compression}: {message}"
        );
    }
}

#[test]
fn a_block_whose_payload_stops_inside_it_is_refused_not_decoded_short() {
    // The header says 2x2 u8 (4 bytes) but the file holds three: a short payload, never a short plane.
    let dataset = dataset("truncated", "raw");
    let mut block: Vec<u8> = [0u16, 3].iter().flat_map(|v| v.to_be_bytes()).collect();
    block.extend([2u32, 2, 1].iter().flat_map(|v| v.to_be_bytes()));
    block.extend_from_slice(&[1, 2, 3]);
    dataset.write_bytes(&[0, 0, 0], &block);
    let message = plane_refusal(&dataset);
    assert!(message.contains("not the 4"), "{message}");
}

#[test]
fn a_plane_outside_the_scene_is_refused_without_a_panic() {
    let dataset = dataset("coords", "raw");
    let reader = N5Reader::open(&dataset).expect("open the dataset");
    assert!(matches!(
        reader.plane(1, 0, 0, 0),
        Err(Error::SceneOutOfRange { scene: 1, count: 1 })
    ));
    for (t, c, z) in [(1, 0, 0), (0, 1, 0), (0, 0, 2)] {
        assert!(
            matches!(reader.plane(0, t, c, z), Err(Error::OutOfRange { .. })),
            "t={t} c={c} z={z}"
        );
    }
    assert!(
        reader.meta().scene(1).is_err(),
        "the one scene is the only one"
    );
}
