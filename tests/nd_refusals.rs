// SPDX-License-Identifier: Apache-2.0
//! Every way a MetaMorph `.nd` dataset is refused: an image file the dataset directory does not hold, a name that
//! would leave that directory, an image file of another sample type or another size, a malformed line, a file
//! without its `EndFile` marker, a count of zero and a `.nd` file over the cap.
//! The datasets are written by `tests/support/nd.rs` and removed by its `Drop`.

mod support;

use phasit_imread::nd_reader::NdReader;
use phasit_imread::{detect_format, Error, Format, ImageReader};

use support::nd::{NdDataset, NdText};

/// The sentence a dataset is refused with at open, whatever the kind.
fn open_refusal(dataset: &NdDataset) -> String {
    match NdReader::open(&dataset.join("dataset.nd")) {
        Err(Error::Unsupported(message)) | Err(Error::Parse(message)) => message,
        Err(other) => panic!("expected a refusal, got {other:?}"),
        Ok(_) => panic!("the dataset opened"),
    }
}

/// The sentence a plane read is refused with (the dataset itself opens).
fn plane_refusal(reader: &NdReader, scene: usize, t: usize, c: usize, z: usize) -> String {
    match reader.plane(scene, t, c, z) {
        Err(
            error @ (Error::Unsupported(_)
            | Error::Parse(_)
            | Error::Decode(_)
            | Error::OversizedAllocation { .. }),
        ) => error.to_string(),
        Err(other) => panic!("expected a refusal, got {other:?}"),
        Ok(_) => panic!("the plane decoded"),
    }
}

/// The text of a dataset of two wavelengths, two stage positions and three time points.
fn cube_text() -> String {
    NdText::new()
        .key("DoWave", "TRUE")
        .key("NWavelengths", "2")
        .key("WaveName1", "\"DAPI\"")
        .key("WaveName2", "\"FITC\"")
        .key("WaveInFileName", "TRUE")
        .key("DoStage", "TRUE")
        .key("NStagePositions", "2")
        .key("DoTimelapse", "TRUE")
        .key("NTimePoints", "3")
        .text()
}

/// The image file of one (wave, stage, time), as the dataset names it.
fn file_name(wave: usize, name: &str, stage: usize, time: usize) -> String {
    format!("dataset_w{wave}{name}_s{stage}_t{time}.TIF")
}

#[test]
fn an_image_file_the_dataset_directory_does_not_hold_is_refused_by_its_indices() {
    let dataset = NdDataset::with_text("missing", &cube_text());
    for (wave, name) in [(1, "DAPI"), (2, "FITC")] {
        for stage in 1..=2 {
            for time in 1..=3 {
                if (wave, stage, time) == (2, 2, 3) {
                    continue;
                }
                dataset.write_tiff(&file_name(wave, name, stage, time), 3, 2, 1);
            }
        }
    }
    let reader = NdReader::open(&dataset.join("dataset.nd")).expect("the dataset opens");

    let message = plane_refusal(&reader, 1, 2, 1, 0);
    assert!(message.contains("wavelength 2"), "{message}");
    assert!(message.contains("stage position 2"), "{message}");
    assert!(message.contains("time point 3"), "{message}");
    assert!(message.contains("missing"), "{message}");
    // The files that are there are read as before.
    assert!(reader.plane(1, 1, 1, 0).is_ok());
}

#[test]
fn a_name_that_would_leave_the_dataset_directory_is_refused() {
    for name in ["../other", "a/b", "a\\b", "C:"] {
        let text = NdText::new()
            .key("DoWave", "TRUE")
            .key("NWavelengths", "1")
            .key("WaveName1", &format!("\"{name}\""))
            .text();
        let dataset = NdDataset::with_text("traversal", &text);
        let message = open_refusal(&dataset);
        assert!(message.contains("own directory"), "{name}: {message}");
    }
    let text = NdText::new()
        .key("DoStage", "TRUE")
        .key("NStagePositions", "1")
        .key("Stage1", "\"../other\"")
        .text();
    let dataset = NdDataset::with_text("traversal_stage", &text);
    assert!(open_refusal(&dataset).contains("own directory"));
}

#[test]
fn an_image_file_of_another_sample_type_is_refused() {
    let dataset = NdDataset::with_text("dtype", &cube_text());
    dataset.write_tiff("dataset_w1DAPI_s1_t1.TIF", 3, 2, 1);
    dataset.write_tiff_u16("dataset_w1DAPI_s1_t2.TIF", 3, 2);
    let reader = NdReader::open(&dataset.join("dataset.nd")).expect("the dataset opens");

    let message = plane_refusal(&reader, 0, 1, 0, 0);
    assert!(message.contains("U16"), "{message}");
    assert!(message.contains("dataset_w1DAPI_s1_t2.TIF"), "{message}");
}

#[test]
fn an_image_file_of_another_sample_count_is_refused() {
    // The first file of the dataset holds three samples per pixel, the second one a single sample.
    let dataset = NdDataset::with_text("samples", &cube_text());
    dataset.write_tiff_rgb("dataset_w1DAPI_s1_t1.TIF", 3, 2);
    dataset.write_tiff("dataset_w1DAPI_s1_t2.TIF", 3, 2, 1);
    let reader = NdReader::open(&dataset.join("dataset.nd")).expect("the dataset opens");

    let message = plane_refusal(&reader, 0, 1, 0, 0);
    assert!(message.contains("sample"), "{message}");
    assert!(message.contains("dataset_w1DAPI_s1_t2.TIF"), "{message}");
}

#[test]
fn an_image_file_of_another_size_is_refused() {
    let dataset = NdDataset::with_text("size", &cube_text());
    dataset.write_tiff("dataset_w1DAPI_s1_t1.TIF", 3, 2, 1);
    dataset.write_tiff("dataset_w1DAPI_s1_t2.TIF", 5, 2, 1);
    let reader = NdReader::open(&dataset.join("dataset.nd")).expect("the dataset opens");

    let message = plane_refusal(&reader, 0, 1, 0, 0);
    assert!(message.contains("5x2"), "{message}");
    assert!(message.contains("3x2"), "{message}");
}

#[test]
fn a_malformed_line_is_refused() {
    for text in [
        "NDInfoFile, Version 1.0\n\"EndFile\"\n",
        "\"NDInfoFile\", Version 1.0\n\"DoWave\" TRUE\n\"EndFile\"\n",
        "\"NDInfoFile\", Version 1.0\n\"WaveName1\", \"unterminated\n\"EndFile\"\n",
    ] {
        let dataset = NdDataset::with_text("malformed", text);
        let message = open_refusal(&dataset);
        assert!(message.contains("line"), "{text:?}: {message}");
    }
}

#[test]
fn a_count_that_is_not_a_number_is_refused() {
    let dataset = NdDataset::with_text(
        "not_a_number",
        "\"NDInfoFile\", Version 1.0\n\"DoWave\", TRUE\n\"NWavelengths\", 2 extra\n\"EndFile\"\n",
    );
    let message = open_refusal(&dataset);
    assert!(message.contains("NWavelengths"), "{message}");
    assert!(message.contains("not a number"), "{message}");
}

#[test]
fn a_file_without_its_end_file_marker_is_refused() {
    let dataset = NdDataset::with_text("no_end", "\"NDInfoFile\", Version 1.0\n");
    let message = open_refusal(&dataset);
    assert!(message.contains("EndFile"), "{message}");
}

#[test]
fn a_file_that_does_not_open_with_ndinfofile_is_refused() {
    let dataset = NdDataset::with_text("no_start", "\"Something\", 1\n\"EndFile\"\n");
    let message = open_refusal(&dataset);
    assert!(message.contains("NDInfoFile"), "{message}");
}

#[test]
fn a_count_of_zero_is_refused() {
    let text = NdText::new()
        .key("DoWave", "TRUE")
        .key("NWavelengths", "0")
        .text();
    let dataset = NdDataset::with_text("zero", &text);
    let message = open_refusal(&dataset);
    assert!(message.contains("NWavelengths"), "{message}");
}

#[test]
fn an_nd_file_over_the_cap_is_refused() {
    let dataset = NdDataset::with_text("oversize", &NdText::new().text());
    let path = dataset.join("dataset.nd");
    let text = format!(
        "\"NDInfoFile\", Version 1.0\n{}\n\"EndFile\"\n",
        "x".repeat(1024 * 1024)
    );
    std::fs::write(&path, text).expect("write the .nd file");
    match NdReader::open(&path) {
        Err(Error::OversizedAllocation { requested, cap }) => {
            assert!(
                requested > cap,
                "requested {requested} bytes, cap {cap} bytes"
            );
            assert_eq!(cap, 1024 * 1024);
        }
        Err(other) => panic!("expected the cap to refuse the file, got {other:?}"),
        Ok(_) => panic!("the file opened"),
    }
}

#[test]
fn a_dataset_without_any_image_file_is_refused() {
    let dataset = NdDataset::with_text("empty", &NdText::new().text());
    let message = open_refusal(&dataset);
    assert!(message.contains("none of the image files"), "{message}");
}

#[test]
fn a_dataset_that_declares_a_million_wavelengths_is_refused() {
    // A ten-line file declares a million channels: it is refused before a million names are built.
    let text = "\"NDInfoFile\", Version 1.0\n\"DoWave\", TRUE\n\"NWavelengths\", 1000000\n\
                \"DoStage\", TRUE\n\"NStagePositions\", 1000000\n\"EndFile\"\n";
    let dataset = NdDataset::with_text("hostile", text);
    let message = open_refusal(&dataset);
    assert!(message.contains("NWavelengths"), "{message}");
    assert!(message.contains("32"), "{message}");
}

#[test]
fn a_corrupt_image_file_is_refused_by_its_name_and_the_dataset_opens() {
    let dataset = NdDataset::with_text("corrupt", &cube_text());
    std::fs::write(
        dataset.join("dataset_w1DAPI_s1_t1.TIF"),
        b"not a TIFF at all",
    )
    .expect("write the corrupt file");
    dataset.write_tiff("dataset_w1DAPI_s1_t2.TIF", 3, 2, 1);
    let reader = NdReader::open(&dataset.join("dataset.nd")).expect("the dataset opens");

    let message = plane_refusal(&reader, 0, 0, 0, 0);
    assert!(message.contains("dataset_w1DAPI_s1_t1.TIF"), "{message}");
    assert!(
        reader.plane(0, 1, 0, 0).is_ok(),
        "the file that reads is still served"
    );
}

#[test]
fn the_dispatcher_opens_a_dataset_as_one_reader() {
    let dataset = NdDataset::with_text("detected", &cube_text());
    dataset.write_tiff("dataset_w1DAPI_s1_t1.TIF", 3, 2, 1);
    let path = dataset.join("dataset.nd");

    assert_eq!(
        detect_format(&path).expect("the format"),
        Format::MetaMorphNd
    );
    let reader = phasit_imread::open(&path).expect("open through the dispatcher");
    assert_eq!(reader.meta().scene_count(), 2);
}

#[test]
fn a_text_file_that_is_not_a_dataset_is_not_detected_as_one() {
    let dataset = NdDataset::with_text("not_a_dataset", "just some notes\n");
    assert!(!matches!(
        detect_format(&dataset.join("dataset.nd")),
        Ok(Format::MetaMorphNd)
    ));
}
