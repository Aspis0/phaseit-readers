// SPDX-License-Identifier: Apache-2.0
//! The resolution counts a file states that the reader cannot use: a count of zero, a negative one
//! or a non-finite one is a stated absurdity, and the image is read uncalibrated and says so.

use phasit_imread::tiff_calibration::calibration_from_tags;

/// The calibration of a file that states these resolutions, with a resolution unit of micrometres.
fn calibration_of(x: Option<f64>, y: Option<f64>) -> phasit_imread::calibration::Calibration {
    calibration_from_tags(
        Some("ImageJ=1.54g\nunit=\u{b5}m\n"),
        x,
        y,
        Some(1),
    )
}

/// A count of zero on one axis is named, and the image is read uncalibrated.
#[test]
fn a_resolution_of_zero_is_named() {
    let calibration = calibration_of(Some(0.0), Some(13.0));
    assert!(
        calibration.px_size_um.is_none(),
        "a count of zero states no pixel size"
    );
    assert_eq!(calibration.notes.len(), 1, "the count the file states is named");
    let note = &calibration.notes[0];
    assert!(note.contains("on X"), "the note names the axis: {note}");
    assert!(note.contains("0"), "the note names the count: {note}");
    assert!(
        note.contains("uncalibrated"),
        "the note says the image is read uncalibrated: {note}"
    );
}

/// A negative count and a non-finite one are named as they are.
#[test]
fn a_negative_and_a_non_finite_resolution_are_named() {
    for (count, stated) in [(-13.0f64, "-13"), (f64::NAN, "NaN"), (f64::INFINITY, "inf")] {
        let calibration = calibration_of(Some(count), Some(13.0));
        assert!(
            calibration.px_size_um.is_none(),
            "{count} states no pixel size"
        );
        assert_eq!(calibration.notes.len(), 1, "{count} is named");
        assert!(
            calibration.notes[0].contains(stated),
            "the note names the count: {}",
            calibration.notes[0]
        );
    }
}

/// Both axes are named when both are unusable: the two counts are two facts about the file.
#[test]
fn both_unusable_resolutions_are_named_separately() {
    let calibration = calibration_of(Some(0.0), Some(-1.0));
    assert_eq!(calibration.notes.len(), 2, "both counts are named");
    assert!(calibration.notes[0].contains("on X"), "{}", calibration.notes[0]);
    assert!(calibration.notes[1].contains("on Y"), "{}", calibration.notes[1]);
}

/// A resolution the reader can use earns no note.
#[test]
fn a_usable_resolution_earns_no_note() {
    let calibration = calibration_of(Some(13.0), Some(13.0));
    assert!(
        calibration.px_size_um.is_some(),
        "a usable count states a pixel size"
    );
    assert!(calibration.notes.is_empty(), "{:?}", calibration.notes);
}
