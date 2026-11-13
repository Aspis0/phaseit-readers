// SPDX-License-Identifier: Apache-2.0
//! The MetaMorph `.nd` reader on synthetic datasets: one wave, stage position and time point; two wavelengths x two
//! stage positions x three time points; a z-series of four planes for one wavelength and one plane for the other; a
//! wavelength name that is not part of the file name; file names in another case; the calibration of the first file
//! and the z step of the `.nd` file.
//! The datasets are written by `tests/support/nd.rs` and removed by its `Drop`.

mod support;

use phasit_imread::nd_reader::NdReader;
use phasit_imread::types::Dims5;
use phasit_imread::{DType, Error, ImageReader, PlaneBuf};

use support::nd::{plane_samples, NdDataset, NdText};
use support::tiff_page::Pattern;

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
        .key("Stage1", "\"first\"")
        .key("Stage2", "\"second\"")
        .key("DoTimelapse", "TRUE")
        .key("NTimePoints", "3")
        .text()
}

/// The value of the image file of one (wave, stage, time): every file of the dataset holds its own.
fn file_value(wave: usize, stage: usize, time: usize) -> u8 {
    (wave as u8) * 100 + (stage as u8) * 10 + time as u8
}

/// The name of the image file of one (wave, stage, time), as the dataset names it.
fn file_name(wave: usize, name: &str, stage: usize, time: usize) -> String {
    format!("dataset_w{wave}{name}_s{stage}_t{time}.TIF")
}

#[test]
fn a_dataset_of_one_wave_one_stage_and_one_time_holds_one_plane() {
    let dataset = NdDataset::with_text("simple", &NdText::new().text());
    dataset.write_tiff("dataset.TIF", 4, 3, 7);
    let reader = NdReader::open(&dataset.join("dataset.nd")).expect("open the dataset");

    let scene = reader.meta().scene(0).expect("the scene");
    assert_eq!(
        scene.dims,
        Dims5 {
            t: 1,
            c: 1,
            z: 1,
            y: 3,
            x: 4
        }
    );
    assert_eq!(scene.dtype, DType::U8);
    assert_eq!(
        reader.plane(0, 0, 0, 0).expect("the plane").data,
        PlaneBuf::U8(vec![7; 12])
    );
}

#[test]
fn every_plane_comes_from_the_file_of_its_wavelength_stage_and_time() {
    let dataset = NdDataset::with_text("cube", &cube_text());
    for (wave, name) in [(1, "DAPI"), (2, "FITC")] {
        for stage in 1..=2 {
            for time in 1..=3 {
                let value = file_value(wave, stage, time);
                dataset.write_tiff(&file_name(wave, name, stage, time), 3, 2, value);
            }
        }
    }
    let reader = NdReader::open(&dataset.join("dataset.nd")).expect("open the dataset");

    let scene = reader.meta().scene(0).expect("the scene");
    assert_eq!(
        scene.dims,
        Dims5 {
            t: 3,
            c: 2,
            z: 1,
            y: 2,
            x: 3
        }
    );
    assert_eq!(
        reader.meta().scene_count(),
        2,
        "one scene per stage position"
    );
    for wave in 1..=2 {
        for stage in 1..=2 {
            for time in 1..=3 {
                let plane = reader
                    .plane(stage - 1, time - 1, wave - 1, 0)
                    .expect("the plane");
                let value = file_value(wave, stage, time);
                assert_eq!(
                    plane.data,
                    PlaneBuf::U8(vec![value; 6]),
                    "wave {wave} stage {stage} time {time}"
                );
            }
        }
    }
}

#[test]
fn a_z_series_is_read_plane_by_plane_and_a_wavelength_without_one_holds_one_plane() {
    // The first wavelength holds a z-series of four planes; the second holds one plane, as its own file says.
    let text = NdText::new()
        .key("DoWave", "TRUE")
        .key("NWavelengths", "2")
        .key("WaveName1", "\"DAPI\"")
        .key("WaveName2", "\"FITC\"")
        .key("WaveInFileName", "TRUE")
        .key("DoZSeries", "TRUE")
        .key("NZSteps", "4")
        .key("WaveDoZ2", "FALSE")
        .text();
    let dataset = NdDataset::with_text("zseries", &text);
    dataset.write_stk("dataset_w1DAPI.STK", 3, 2, 4, 0);
    dataset.write_tiff_u16("dataset_w2FITC.TIF", 3, 2);
    let reader = NdReader::open(&dataset.join("dataset.nd")).expect("open the dataset");

    let scene = reader.meta().scene(0).expect("the scene");
    assert_eq!(
        scene.dims,
        Dims5 {
            t: 1,
            c: 2,
            z: 4,
            y: 2,
            x: 3
        }
    );
    for z in 0..4 {
        let plane = reader.plane(0, 0, 0, z).expect("the plane");
        assert_eq!(
            plane.data,
            PlaneBuf::U16(plane_samples(z, 3, 2, 0)),
            "z = {z}"
        );
    }
    // The second wavelength has one plane: the file holds it, and no plane of it is the first one again.
    let single = reader.plane(0, 0, 1, 0).expect("the one plane");
    let expected: Vec<u16> = (0..2)
        .flat_map(|y| (0..3).map(move |x| u16::at(x, y, 0)))
        .collect();
    assert_eq!(single.data, PlaneBuf::U16(expected));
    let error = reader.plane(0, 0, 1, 1).expect_err("the second plane");
    assert!(error.to_string().contains("one plane"), "{error}");
}

#[test]
fn a_wavelength_name_that_is_not_part_of_the_file_name_is_found_by_its_index() {
    let text = NdText::new()
        .key("DoWave", "TRUE")
        .key("NWavelengths", "2")
        .key("WaveName1", "\"DAPI\"")
        .key("WaveName2", "\"FITC\"")
        .key("WaveInFileName", "FALSE")
        .text();
    let dataset = NdDataset::with_text("no_name", &text);
    dataset.write_tiff("dataset_w1.TIF", 2, 2, 11);
    dataset.write_tiff("dataset_w2.TIF", 2, 2, 22);
    let reader = NdReader::open(&dataset.join("dataset.nd")).expect("open the dataset");

    assert_eq!(
        reader.plane(0, 0, 0, 0).expect("the first plane").data,
        PlaneBuf::U8(vec![11; 4])
    );
    assert_eq!(
        reader.plane(0, 0, 1, 0).expect("the second plane").data,
        PlaneBuf::U8(vec![22; 4])
    );
}

#[test]
fn an_image_file_is_found_whatever_the_case_of_its_name() {
    let dataset = NdDataset::with_text("case", &cube_text());
    for (wave, name) in [(1, "DAPI"), (2, "FITC")] {
        for stage in 1..=2 {
            for time in 1..=3 {
                let value = file_value(wave, stage, time);
                let spelled = file_name(wave, name, stage, time).to_uppercase();
                dataset.write_tiff(&spelled, 3, 2, value);
            }
        }
    }
    let reader = NdReader::open(&dataset.join("dataset.nd")).expect("open the dataset");

    let plane = reader.plane(1, 2, 1, 0).expect("the plane");
    assert_eq!(plane.data, PlaneBuf::U8(vec![file_value(2, 2, 3); 6]));
}

#[test]
fn the_calibration_comes_from_the_first_file_and_the_z_step_from_the_nd_file() {
    // The first file is an STK, which states a pixel size of its own; the z step is in the `.nd` file.
    let text = NdText::new()
        .key("DoZSeries", "TRUE")
        .key("NZSteps", "2")
        .key("ZStepSize", "0.25")
        .text();
    let dataset = NdDataset::with_text("calibrated", &text);
    dataset.write_stk("dataset.STK", 3, 2, 2, 0);
    let reader = NdReader::open(&dataset.join("dataset.nd")).expect("open the dataset");

    let scene = reader.meta().scene(0).expect("the scene");
    let px = scene.px_size_um.expect("the pixel size is published");
    assert_eq!(px.x, 0.5, "the first file states its own pixel size");
    assert_eq!(px.y, 0.25);
    assert_eq!(
        px.z,
        Some(0.25),
        "the z step is the one the .nd file states"
    );
}

#[test]
fn the_stage_positions_are_the_scenes_and_the_wavelengths_the_channels() {
    let dataset = NdDataset::with_text("scenes", &cube_text());
    for (wave, name) in [(1, "DAPI"), (2, "FITC")] {
        for stage in 1..=2 {
            for time in 1..=3 {
                dataset.write_tiff(&file_name(wave, name, stage, time), 3, 2, 1);
            }
        }
    }
    let reader = NdReader::open(&dataset.join("dataset.nd")).expect("open the dataset");

    assert_eq!(reader.meta().scene(0).expect("the scene").name, "first");
    assert_eq!(reader.meta().scene(1).expect("the scene").name, "second");
    let channels = &reader.meta().scene(0).expect("the scene").channels;
    assert_eq!(channels.len(), 2);
    assert_eq!(channels[0].name.as_deref(), Some("DAPI"));
    assert_eq!(channels[1].name.as_deref(), Some("FITC"));
}

#[test]
fn a_plane_outside_the_scene_is_refused_without_a_panic() {
    let dataset = NdDataset::with_text("coords", &cube_text());
    for (wave, name) in [(1, "DAPI"), (2, "FITC")] {
        for stage in 1..=2 {
            for time in 1..=3 {
                dataset.write_tiff(&file_name(wave, name, stage, time), 3, 2, 1);
            }
        }
    }
    let reader = NdReader::open(&dataset.join("dataset.nd")).expect("open the dataset");

    assert!(matches!(
        reader.plane(2, 0, 0, 0),
        Err(Error::SceneOutOfRange { scene: 2, count: 2 })
    ));
    for (t, c) in [(3, 0), (0, 2)] {
        assert!(
            matches!(reader.plane(0, t, c, 0), Err(Error::OutOfRange { .. })),
            "t={t} c={c}"
        );
    }
}
