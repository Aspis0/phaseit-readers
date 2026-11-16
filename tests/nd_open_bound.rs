// SPDX-License-Identifier: Apache-2.0
//! A MetaMorph `.nd` dataset of more image files than stay open at once: the scene says so by number, and every
//! plane is still read correctly across the files that were closed and opened again.
//! The datasets are written by `tests/support/nd.rs` and removed by its `Drop`.

mod support;

use phasit_imread::nd_reader::NdReader;
use phasit_imread::types::PlaneBuf;
use phasit_imread::ImageReader;

use support::nd::{NdDataset, NdText};

/// One more image file than the most that stay open at once.
const FILES: usize = 257;

/// The grey value of the image file of one stage position: neighbours differ, so a file read in place of another
/// shows.
fn value(stage: usize) -> u8 {
    (stage % 251) as u8
}

/// A dataset of `stages` stage positions: one 2x2 grey file per position, each holding its own value.
fn dataset(stages: usize) -> NdDataset {
    let text = NdText::new()
        .key("DoStage", "TRUE")
        .key("NStagePositions", &stages.to_string())
        .text();
    let dataset = NdDataset::with_text("open_bound", &text);
    for stage in 1..=stages {
        dataset.write_tiff(&format!("dataset_s{stage}.TIF"), 2, 2, value(stage));
    }
    dataset
}

#[test]
fn a_dataset_of_more_files_than_stay_open_says_so_and_reads_every_plane() {
    let dataset = dataset(FILES);
    let reader = NdReader::open(&dataset.join("dataset.nd")).expect("the dataset opens");

    let notes = &reader.meta().scenes[0].notes;
    assert!(
        notes
            .iter()
            .any(|note| note.contains("257 image files, at most 256 stay open")),
        "{notes:?}"
    );
    // The second round reopens the files the first round closed.
    for _ in 0..2 {
        for scene in 0..FILES {
            let plane = reader.plane(scene, 0, 0, 0).expect("the plane is read");
            assert_eq!(
                plane.data,
                PlaneBuf::U8(vec![value(scene + 1); 4]),
                "scene {scene}"
            );
        }
    }
}

#[test]
fn a_dataset_within_the_bound_states_no_open_files_note() {
    let dataset = dataset(3);
    let reader = NdReader::open(&dataset.join("dataset.nd")).expect("the dataset opens");

    for scene in &reader.meta().scenes {
        assert!(
            scene.notes.iter().all(|note| !note.contains("stay open")),
            "{:?}",
            scene.notes
        );
    }
}
