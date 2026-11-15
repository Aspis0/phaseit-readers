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
use tiff::tags::{ResolutionUnit, Tag};

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

/// A TIFF whose X and Y resolutions are these many pixels per unit, with this resolution unit and
/// this description.
fn write_a_tiff_with_resolution(
    path: &Path,
    x: u32,
    y: u32,
    unit: ResolutionUnit,
    description: Option<&str>,
) {
    let mut file = fs::File::create(path).expect("create the scratch TIFF");
    let mut writer = TiffEncoder::new(&mut file).expect("start the TIFF");
    let mut image = writer.new_image::<Gray8>(1, 1).expect("start the image");
    image.x_resolution(Rational { n: x, d: 1 });
    image.y_resolution(Rational { n: y, d: 1 });
    image.resolution_unit(unit);
    if let Some(text) = description {
        image
            .encoder()
            .write_tag(Tag::ImageDescription, text)
            .expect("write the description");
    }
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

/// A pixel size outside the accepted range reaches the scene as a note: the reader publishes none,
/// and says which size the file stated.
#[test]
fn an_out_of_range_resolution_is_named_in_the_scene() {
    // 100 pixels per centimetre is 100 µm per pixel: no light microscope measures that.
    let path = Scratch::new(
        std::env::temp_dir().join(format!("phasit_t1_out_of_range_{}.tif", std::process::id())),
    );
    write_a_tiff_with_resolution(&path, 100, 5_000, ResolutionUnit::Centimeter, None);

    let reader = TiffReader::open(&path).expect("open the synthetic TIFF");
    let scene = reader.meta().scene(0).expect("the file has a scene");
    assert!(
        scene.px_size_um.is_none(),
        "a size outside the range is not published"
    );
    assert_eq!(scene.notes.len(), 1, "the size the file states is named");
    assert!(scene.notes[0].contains("on X"), "{}", scene.notes[0]);
    assert!(scene.notes[0].contains("100"), "{}", scene.notes[0]);
    assert!(
        scene.notes[0].contains("uncalibrated"),
        "{}",
        scene.notes[0]
    );
}

/// A unit the file writes as an escape is read as the character it names: the note shows the unit,
/// not the escape.
#[test]
fn an_escaped_unit_is_read_as_the_character_it_names() {
    // A writer that cannot emit the micro sign leaves the literal escape in the description.
    let path = Scratch::new(
        std::env::temp_dir().join(format!("phasit_t1_escaped_unit_{}.tif", std::process::id())),
    );
    write_a_tiff_with_resolution(
        &path,
        2_000,
        13,
        ResolutionUnit::None,
        Some("ImageJ=1.54g\nunit=\\u00b5m\n"),
    );

    let reader = TiffReader::open(&path).expect("open the synthetic TIFF");
    let scene = reader.meta().scene(0).expect("the file has a scene");
    assert_eq!(scene.notes.len(), 1, "the size the file states is named");
    assert!(
        scene.notes[0].contains("per \u{b5}m"),
        "the note names the unit the file states: {}",
        scene.notes[0]
    );
    assert!(
        !scene.notes[0].contains("\\u00b5"),
        "the escape is not shown raw: {}",
        scene.notes[0]
    );
}
