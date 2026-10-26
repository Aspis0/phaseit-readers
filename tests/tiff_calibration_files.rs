// SPDX-License-Identifier: Apache-2.0
//! The calibration of the real TIFF files, against Bio-Formats' numbers.
//!
//! Bio-Formats publishes "1 µm/px" for a file that states nothing; that is
//! its UNCALIBRATED DEFAULT, so the comparison treats it as "both
//! uncalibrated". The files are read from the folder named by
//! `PHASIT_REAL_DATA_DIR` (the layout is in the crate README). Skips cleanly
//! (with a printed note) when the variable is unset or a sample is absent.

mod real_data;

use phasit_imread::tiff_reader::TiffReader;
use phasit_imread::ImageReader;

use real_data::real_file;

/// A file that states its scale, with the value Bio-Formats publishes.
const CALIBRATED: [(&str, f64); 3] = [
    ("tiff/calibrated-a.tif", 0.07321568256491522),
    ("tiff/calibrated-b.tif", 0.06589411527332022),
    ("tiff/rgb.tif", 0.06589411527332022),
];

/// A file that states no scale at all.
const UNCALIBRATED: [&str; 1] = ["tiff/uncalibrated.tif"];

#[test]
fn a_calibrated_tiff_states_the_pixelsize_bioformats_publishes() {
    for (sample, expected) in CALIBRATED {
        let Some(path) = real_file(sample) else {
            continue;
        };
        let reader = TiffReader::open(&path).expect("open the TIFF");
        let scene = reader.meta().scene(0).expect("the file has a scene");
        let px = scene.px_size_um.expect("the file states a pixel size");
        assert!(
            (px.x as f64 - expected).abs() <= 1e-6 * expected,
            "{sample}: phasit-imread says {} µm/px, Bio-Formats says {expected}",
            px.x
        );
        assert!(
            (px.y as f64 - expected).abs() <= 1e-6 * expected,
            "{sample}: phasit-imread says {} µm/px on Y, Bio-Formats says {expected}",
            px.y
        );
    }
}

#[test]
fn an_uncalibrated_tiff_states_no_pixelsize() {
    for sample in UNCALIBRATED {
        let Some(path) = real_file(sample) else {
            continue;
        };
        let reader = TiffReader::open(&path).expect("open the TIFF");
        let scene = reader.meta().scene(0).expect("the file has a scene");
        assert!(
            scene.px_size_um.is_none(),
            "{sample}: the file states no scale, so phasit-imread must state none either"
        );
    }
}
