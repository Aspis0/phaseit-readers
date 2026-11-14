// SPDX-License-Identifier: Apache-2.0
//! The image files a MetaMorph `.nd` dataset names: the files its planes are read from.
//! The datasets are written by `tests/support/nd.rs` and removed by its `Drop`.

mod support;

use phasit_imread::nd_reader::{member_files, NdReader};
use phasit_imread::ImageReader;

use support::nd::{NdDataset, NdText};

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

/// The name of the image file of one (wave, stage, time), as the dataset names it.
fn file_name(wave: usize, name: &str, stage: usize, time: usize) -> String {
    format!("dataset_w{wave}{name}_s{stage}_t{time}.TIF")
}

/// The dataset of the cube text, with every image file it names.
fn cube_dataset() -> NdDataset {
    let dataset = NdDataset::with_text("members", &cube_text());
    for (wave, name) in [(1, "DAPI"), (2, "FITC")] {
        for stage in 1..=2 {
            for time in 1..=3 {
                dataset.write_tiff(&file_name(wave, name, stage, time), 3, 2, 1);
            }
        }
    }
    dataset
}

/// The files the dataset names, as their names.
fn member_names(dataset: &NdDataset) -> Vec<String> {
    let files = member_files(&dataset.join("dataset.nd")).expect("the dataset's files");
    files
        .iter()
        .map(|file| {
            file.file_name()
                .expect("a member file has a name")
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

#[test]
fn the_dataset_names_the_files_its_planes_are_read_from() {
    let dataset = cube_dataset();

    let names = member_names(&dataset);

    assert_eq!(names.len(), 12, "one file per plane of the dataset");
    for (wave, name) in [(1, "DAPI"), (2, "FITC")] {
        for stage in 1..=2 {
            for time in 1..=3 {
                assert!(
                    names.contains(&file_name(wave, name, stage, time)),
                    "wave {wave} stage {stage} time {time} is a member"
                );
            }
        }
    }
}

#[test]
fn every_member_file_sits_in_the_dataset_directory() {
    let dataset = cube_dataset();

    let files = member_files(&dataset.join("dataset.nd")).expect("the dataset's files");
    let directory = dataset
        .join("dataset.nd")
        .parent()
        .expect("a directory")
        .to_path_buf();

    // WHY the count: the loop below says nothing when there is nothing to loop over, so the
    // boundary would be asserted by no test at all.
    assert!(!files.is_empty(), "the dataset holds member files");
    for file in &files {
        assert_eq!(
            file.parent(),
            Some(directory.as_path()),
            "{} is a file of the dataset's own directory",
            file.display()
        );
    }
}

#[test]
fn a_file_the_dataset_names_but_the_directory_does_not_hold_is_no_member() {
    let dataset = NdDataset::with_text("missing", &cube_text());
    dataset.write_tiff(&file_name(1, "DAPI", 1, 1), 3, 2, 1);

    let names = member_names(&dataset);

    assert_eq!(
        names,
        vec![file_name(1, "DAPI", 1, 1)],
        "only the file the directory holds is a member"
    );
}

#[test]
fn a_dataset_that_names_a_file_outside_its_directory_is_refused() {
    let text = NdText::new()
        .key("DoWave", "TRUE")
        .key("NWavelengths", "1")
        .key("WaveName1", "\"../outside\"")
        .text();
    let dataset = NdDataset::with_text("escape", &text);

    let error =
        member_files(&dataset.join("dataset.nd")).expect_err("a name outside the directory");

    assert!(
        error.to_string().contains("own directory"),
        "the refusal says the name would leave the directory: {error}"
    );
}

#[test]
fn a_file_outside_the_dataset_directory_is_never_opened() {
    // The dataset sits in a folder of its own, and a file of the name it expects sits beside that
    // folder: the reader looks in the dataset's own directory only.
    let dataset = NdDataset::with_text("outside", &NdText::new().text());
    let folder = dataset.join("acq");
    std::fs::create_dir_all(&folder).expect("the dataset's folder");
    let nd = dataset.write_nd("acq/dataset", &NdText::new().text());
    std::fs::write(dataset.join("dataset.TIF"), b"not an image").expect("the file beside");

    let error = match NdReader::open(&nd) {
        Ok(_) => panic!("a dataset with no image file of its own must not open"),
        Err(error) => error,
    };

    assert!(
        error.to_string().contains("none of the image files"),
        "the reader looked in the dataset's own directory: {error}"
    );
}
