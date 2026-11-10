// SPDX-License-Identifier: Apache-2.0
//! The calibration tags through the reader, on a file the test builds itself: the real corpus is isotropic, so a
//! transposition could hide.

mod support;

use std::fs;
use std::path::Path;

use phasit_imread::tiff_reader::TiffReader;
use phasit_imread::ImageReader;
use support::scratch::Scratch;
use tiff::encoder::colortype::Gray8;
use tiff::encoder::{Rational, TiffEncoder};
use tiff::tags::ResolutionUnit;

/// X and Y resolutions differ (1.0 and 2.0 µm per pixel), so an X/Y transposition shows.
fn write_anisotropic_tiff(path: &Path) {
    let mut file = fs::File::create(path).expect("create the scratch TIFF");
    let mut writer = TiffEncoder::new(&mut file).expect("start the TIFF");
    let mut image = writer.new_image::<Gray8>(1, 1).expect("start the image");
    image.x_resolution(Rational { n: 10_000, d: 1 });
    image.y_resolution(Rational { n: 5_000, d: 1 });
    image.resolution_unit(ResolutionUnit::Centimeter);
    image.write_data(&[0u8]).expect("write the one pixel");
}

fn write_an_inch_tiff(path: &Path) {
    let mut file = fs::File::create(path).expect("create the scratch TIFF");
    let mut writer = TiffEncoder::new(&mut file).expect("start the TIFF");
    let mut image = writer.new_image::<Gray8>(1, 1).expect("start the image");
    image.x_resolution(Rational { n: 300, d: 1 });
    image.y_resolution(Rational { n: 300, d: 1 });
    image.resolution_unit(ResolutionUnit::Inch);
    image.write_data(&[0u8]).expect("write the one pixel");
}

#[test]
fn an_anisotropic_tiff_publishes_each_axis_on_its_own_axis() {
    let path = Scratch::new(
        std::env::temp_dir().join(format!("phasit_t1_anisotropic_{}.tif", std::process::id())),
    );
    write_anisotropic_tiff(&path);

    let reader = TiffReader::open(&path).expect("open the synthetic TIFF");
    let scene = reader.meta().scene(0).expect("the file has a scene");
    let px = scene.px_size_um.expect("the file states a pixel size");
    assert!(
        (px.x as f64 - 1.0).abs() < 1e-6,
        "XResolution 10000/cm is 1.0 µm/px on X, got {}",
        px.x
    );
    assert!(
        (px.y as f64 - 2.0).abs() < 1e-6,
        "YResolution 5000/cm is 2.0 µm/px on Y, got {}",
        px.y
    );
}

#[test]
fn an_inch_resolution_is_named_as_a_print_layout() {
    // A print resolution is not a pixel size: the file publishes none, and says so.
    let path = Scratch::new(
        std::env::temp_dir().join(format!("phasit_t1_inch_{}.tif", std::process::id())),
    );
    write_an_inch_tiff(&path);

    let reader = TiffReader::open(&path).expect("open the synthetic TIFF");
    let scene = reader.meta().scene(0).expect("the file has a scene");
    assert!(
        scene.px_size_um.is_none(),
        "a print layout is not a microscope's pixel size"
    );
    assert_eq!(
        scene.notes,
        vec![phasit_imread::tiff_calibration::print_resolution_note(300.0)],
        "the scene says the resolution is a print layout"
    );
    assert!(
        scene.notes[0].contains("300 dpi"),
        "the note names the resolution the file states: {}",
        scene.notes[0]
    );
}
