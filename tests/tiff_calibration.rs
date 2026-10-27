// SPDX-License-Identifier: Apache-2.0
//! The TIFF calibration rules, on hand-built tag sets.
//!
//! The rules mirror what Fiji reads out of the same files: an ImageJ
//! description names the unit its resolution is counted in, an inch
//! resolution is a print resolution, and a resolution with no absolute unit
//! states no pixel size at all.

use phasit_imread::tiff_calibration::{calibration_from_tags, print_resolution_note};
use phasit_imread::PxSizeUm;

/// The pixel size the tags state, when they state one.
///
/// The calibration itself also carries the notes on a resolution the reader
/// could not use; the pixel size is what these tests are about.
fn px_size(
    description: Option<&str>,
    x_per_unit: Option<f64>,
    y_per_unit: Option<f64>,
    resolution_unit: Option<u16>,
) -> Option<PxSizeUm> {
    calibration_from_tags(description, x_per_unit, y_per_unit, resolution_unit).px_size_um
}

/// The ImageJ description of a file whose resolution counts pixels per µm.
fn description_in_microns() -> String {
    "ImageJ=1.54g\nunit=\u{b5}m\nmin=0.0\nmax=65535.0\n".to_string()
}

/// True when two micrometre values agree to a millionth, which is what the
/// published numbers are compared at.
fn close(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() <= 1e-6 * expected.abs().max(f64::MIN_POSITIVE)
}

#[test]
fn an_imagej_description_in_microns_states_the_pixel_size() {
    // The y23 file's own numbers: 13.658276 pixels per micrometre.
    let count = Some(13_658_276.0 / 1_000_000.0);
    let px = px_size(Some(&description_in_microns()), count, count, Some(1))
        .expect("a description in microns with a resolution states a pixel size");
    assert!(close(px.x as f64, 0.07321568256491522));
    assert!(close(px.y as f64, 0.07321568256491522));
}

#[test]
fn a_description_in_nanometres_scales_the_resolution() {
    // Half a pixel per nanometre is 2 nm per pixel.
    let px = px_size(Some("ImageJ=1.54g\nunit=nm\n"), Some(0.5), Some(0.5), Some(1))
        .expect("a description in nanometres states a pixel size");
    assert!(close(px.x as f64, 0.002));
    assert!(close(px.y as f64, 0.002));
}

#[test]
fn an_escaped_micro_sign_names_the_unit() {
    // A properties writer that cannot emit the micro sign leaves the unit as
    // the literal escape, and ImageJ reads it back resolved.
    let count = Some(13_658_276.0 / 1_000_000.0);
    let px = px_size(
        Some("ImageJ=1.54g\nunit=\\u00b5m\n"),
        count,
        count,
        Some(1),
    )
    .expect("the escaped unit names the micrometre scale");
    assert!(close(px.x as f64, 0.07321568256491522));
}

#[test]
fn a_description_without_a_unit_states_no_pixel_size() {
    let px = px_size(
        Some("ImageJ=1.54g\nmin=0.0\nmax=255.0\n"),
        Some(13.0),
        Some(13.0),
        Some(1),
    );
    assert!(px.is_none(), "no unit names no scale");
}

#[test]
fn a_description_in_microns_without_a_resolution_states_no_pixel_size() {
    // The description's spacing is the Z step, never a pixel size.
    let px = px_size(
        Some("ImageJ=1.54g\nunit=\u{b5}m\nspacing=1.5\n"),
        None,
        None,
        Some(1),
    );
    assert!(
        px.is_none(),
        "a unit without a resolution states no pixel size"
    );
}

#[test]
fn the_description_names_the_unit_only_where_the_tag_names_none() {
    // The tag says centimetre, so the description does not overrule it: the
    // size is the centimetre's size over the count, not the micrometre's.
    let px = px_size(
        Some("ImageJ=1.54g\nunit=\u{b5}m\n"),
        Some(12_700.0),
        Some(12_700.0),
        Some(3),
    )
    .expect("a centimetre resolution states a pixel size");
    assert!(close(px.x as f64, 10_000.0 / 12_700.0));
}

#[test]
fn an_inch_resolution_is_a_print_resolution() {
    // 72, 96 and 300 dpi are all print resolutions, none of them a pixel
    // size a microscope could have measured.
    for dpi in [72.0f64, 96.0, 300.0] {
        let px = px_size(None, Some(dpi), Some(dpi), Some(2));
        assert!(px.is_none(), "{dpi} dpi is a print resolution");
    }
    // A resolution that would LOOK like a pixel size (2000 dpi is 12.7 µm per
    // pixel) is still a print resolution: the rule is about the unit, not
    // about how plausible the number looks.
    let plausible = px_size(None, Some(2_000.0), Some(2_000.0), Some(2));
    assert!(
        plausible.is_none(),
        "an inch resolution is a print resolution whatever it says"
    );
}

#[test]
fn an_inch_resolution_is_a_print_resolution_and_says_so() {
    // The file states a print resolution: the size a picture is laid out at
    // on paper, not a distance between two pixels of a microscope image.
    // The note names the resolution and says it was not used.
    let calibration = calibration_from_tags(None, Some(300.0), Some(300.0), Some(2));
    assert!(
        calibration.px_size_um.is_none(),
        "a print resolution is no pixel size"
    );
    assert_eq!(
        calibration.notes,
        vec![print_resolution_note(300.0)],
        "the note names the print resolution the file states"
    );
}

#[test]
fn a_placeholder_z_step_is_dropped_and_says_so() {
    // The ImageJ description states a spacing far too small to be a distance
    // between two focal planes: the file wrote a placeholder where the step
    // would go.
    let count = Some(13_658_276.0 / 1_000_000.0);
    let calibration = calibration_from_tags(
        Some("ImageJ=1.54g\nunit=\u{b5}m\nspacing=1e-13\n"),
        count,
        count,
        Some(1),
    );
    let px = calibration.px_size_um.expect("the description states a scale");
    assert_eq!(px.z, None, "a placeholder is not a step");
    assert_eq!(
        calibration.notes,
        vec![phasit_imread::calibration::z_step_note(1e-13)
            .expect("a placeholder earns a note")],
        "the note names the placeholder the file stated"
    );
}

#[test]
fn a_centimetre_resolution_states_a_pixel_size() {
    let px = px_size(None, Some(12_700.0), Some(12_700.0), Some(3))
        .expect("a centimetre resolution states a pixel size");
    assert!(close(px.x as f64, 10_000.0 / 12_700.0));
    assert!(close(px.y as f64, 10_000.0 / 12_700.0));
}

#[test]
fn a_resolution_without_an_absolute_unit_states_no_pixel_size() {
    let px = px_size(None, Some(13.0), Some(13.0), Some(1));
    assert!(px.is_none(), "pixels per pixel is no scale");
}

#[test]
fn a_missing_resolution_unit_counts_as_no_absolute_unit() {
    let px = px_size(None, Some(13.0), Some(13.0), None);
    assert!(px.is_none(), "no ResolutionUnit tag names no absolute unit");
}

#[test]
fn an_unknown_resolution_unit_names_no_absolute_unit() {
    let px = px_size(None, Some(13.0), Some(13.0), Some(7));
    assert!(px.is_none(), "an unknown unit code names no scale");
}

#[test]
fn the_spacing_is_the_z_step() {
    let count = Some(13_658_276.0 / 1_000_000.0);
    let px = px_size(
        Some("ImageJ=1.54g\nunit=\u{b5}m\nspacing=0.9994\n"),
        count,
        count,
        Some(1),
    )
    .expect("the description states a scale");
    assert!(close(px.z.expect("the Z step") as f64, 0.9994));
}

#[test]
fn the_spacing_is_converted_from_the_descriptions_unit() {
    let px = px_size(
        Some("ImageJ=1.54g\nunit=nm\nspacing=500\n"),
        Some(0.5),
        Some(0.5),
        Some(1),
    )
    .expect("the description states a scale");
    assert!(close(px.z.expect("the Z step") as f64, 0.5));
}

#[test]
fn an_undeclared_z_step_is_published_as_not_recorded() {
    let count = Some(13_658_276.0 / 1_000_000.0);
    let px = px_size(Some(&description_in_microns()), count, count, Some(1))
        .expect("the description states a scale");
    assert_eq!(px.z, None, "no spacing means the step is not recorded");
}

#[test]
fn a_description_in_inches_states_no_pixel_size() {
    // Imperial names are outside the unit table on purpose: `unit=inch` is a
    // print layout, and gets the same refusal as the ResolutionUnit=inch tag.
    // 1000 px/inch is 25.4 µm/px, which the plausibility gate alone would
    // let through.
    let px = px_size(
        Some("ImageJ=1.54g\nunit=inch\n"),
        Some(1_000.0),
        Some(1_000.0),
        Some(1),
    );
    assert!(px.is_none(), "an inch description is a print resolution");
}

#[test]
fn an_absurd_axis_is_never_replaced_by_the_other_axis() {
    // An XResolution of 0 is a stated absurdity, not an absence: publishing
    // Y's size on X would put a scale on half the picture.
    let px = px_size(None, Some(0.0), Some(10_000.0), Some(3));
    assert!(px.is_none(), "a stated absurd axis states no pixel size");
}

#[test]
fn a_resolution_no_microscope_measured_states_no_pixel_size() {
    // Two thousand pixels per micrometre is half a nanometre per pixel; a
    // thousandth of a pixel per micrometre is a millimetre per pixel. Neither
    // is a pixel size a light microscope measured.
    let too_fine = px_size(
        Some("ImageJ=1.54g\nunit=\u{b5}m\n"),
        Some(2_000.0),
        Some(2_000.0),
        Some(1),
    );
    assert!(too_fine.is_none());

    let too_coarse = px_size(
        Some("ImageJ=1.54g\nunit=\u{b5}m\n"),
        Some(0.001),
        Some(0.001),
        Some(1),
    );
    assert!(too_coarse.is_none());
}

#[test]
fn one_axis_without_a_resolution_takes_the_other_axis_value() {
    let count = Some(13_658_276.0 / 1_000_000.0);
    let px = px_size(Some(&description_in_microns()), count, None, Some(1))
        .expect("one axis is enough to state a scale");
    assert!(close(px.x as f64, 0.07321568256491522));
    assert!(close(px.y as f64, 0.07321568256491522));
}

#[test]
fn an_unusable_resolution_count_states_no_pixel_size() {
    // A zero denominator (and any non-positive or non-finite cast) arrives
    // here only from a defensive read; none of it is a scale.
    for count in [0.0f64, -13.0, f64::NAN, f64::INFINITY] {
        let px = px_size(
            Some("ImageJ=1.54g\nunit=\u{b5}m\n"),
            Some(count),
            Some(count),
            Some(1),
        );
        assert!(px.is_none(), "{count} is no usable resolution count");
    }
}
