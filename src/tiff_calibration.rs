// SPDX-License-Identifier: Apache-2.0
//! TIFF calibration: the pixel size a plain TIFF states about itself.
//!
//! A TIFF can carry its scale in two families of tags, and they answer
//! different questions:
//!
//! - the standard resolution tags: XResolution/YResolution (282/283) with
//!   ResolutionUnit (296) = 2 (inch) or 3 (centimetre);
//! - the ImageJ description (270): `unit=<u>` beside `spacing=<v>`, which is
//!   what ImageJ writes when a user sets the image scale.
//!
//! ImageJ counts its resolution in the unit its description names: `unit=µm`
//! with 13.658276 pixels per micrometre is 0.0732 µm per pixel. Its
//! `spacing=` is the Z STEP — the distance between two planes — and is never
//! a pixel size.
//!
//! Two kinds of resolution are NOT a pixel size, and are published as
//! uncalibrated (no pixel size at all) rather than as a micrometre number:
//!
//! - an INCH resolution: a picture laid out for a printer;
//! - a resolution with no absolute unit: the file counts pixels in pixels.
//!
//! A pixel size is published only when light microscopy could have measured
//! it: between 0.001 µm and 50 µm per pixel, on BOTH axes. A resolution that
//! is plausible on one axis and absurd on the other states a number no
//! microscope measured.

use crate::types::PxSizeUm;

/// The smallest pixel size light microscopy could have measured, in µm.
const MIN_PLAUSIBLE_UM: f64 = 0.001;

/// The largest pixel size light microscopy could have measured, in µm.
const MAX_PLAUSIBLE_UM: f64 = 50.0;

/// ResolutionUnit (tag 296): the resolution counts pixels per pixel.
const RESOLUTION_UNIT_NONE: u16 = 1;
/// ResolutionUnit (tag 296): the resolution counts pixels per inch.
const RESOLUTION_UNIT_INCH: u16 = 2;
/// ResolutionUnit (tag 296): the resolution counts pixels per centimetre.
const RESOLUTION_UNIT_CENTIMETER: u16 = 3;

/// The micrometres one centimetre is worth.
const MICROMETERS_PER_CENTIMETER: f64 = 10_000.0;

/// The pixel size a plain TIFF states about itself, in micrometres.
///
/// `description` is the ImageJ description (tag 270); `x_per_unit` and
/// `y_per_unit` are the pixels-per-unit counts the XResolution/YResolution
/// rationals (tags 282/283) state; `resolution_unit` is the ResolutionUnit
/// tag (296).
///
/// `None` means the file states no pixel size: an uncalibrated file stays
/// uncalibrated, and no default micrometre value is invented for it.
pub fn calibration_from_tags(
    description: Option<&str>,
    x_per_unit: Option<f64>,
    y_per_unit: Option<f64>,
    resolution_unit: Option<u16>,
) -> Option<PxSizeUm> {
    let description = description.and_then(imagej_description);
    if x_per_unit.is_none() && y_per_unit.is_none() {
        return None;
    }

    // A stated-but-unusable count (0, negative, non-finite) is an ABSURD
    // resolution, not an absent one: it states a number no microscope
    // measured, so the file publishes nothing — the count is never replaced
    // by the other axis' value.
    let unusable = |count: Option<f64>| matches!(count, Some(v) if !(v.is_finite() && v > 0.0));
    if unusable(x_per_unit) || unusable(y_per_unit) {
        return None;
    }

    // The unit the resolution is counted in. The description names it ONLY
    // where the ResolutionUnit tag names none: a tag that names an absolute
    // unit is the file's own statement of what the resolution counts, and
    // the description does not overrule it.
    let micrometers_per_unit = match resolution_unit {
        // An inch resolution is the size a picture is laid out at on paper.
        Some(RESOLUTION_UNIT_INCH) => return None,
        Some(RESOLUTION_UNIT_CENTIMETER) => MICROMETERS_PER_CENTIMETER,
        Some(RESOLUTION_UNIT_NONE) | None => {
            // No absolute unit anywhere: the file states no pixel size.
            description.as_ref()?.micrometers_per_unit
        }
        // An unknown unit code names no absolute unit either.
        Some(_) => return None,
    };

    // The resolution counts pixels per unit, so the pixel size is the unit's
    // size over the count.
    let x = x_per_unit.map(|count| micrometers_per_unit / count);
    let y = y_per_unit.map(|count| micrometers_per_unit / count);
    let (x, y) = match (x, y) {
        (Some(x), Some(y)) => (x, y),
        // One axis without a resolution takes the other axis' value.
        (Some(x), None) => (x, x),
        (None, Some(y)) => (y, y),
        (None, None) => return None,
    };

    if !is_plausible_um(x) || !is_plausible_um(y) {
        return None;
    }

    Some(PxSizeUm {
        x: x as f32,
        y: y as f32,
        // The description's spacing is the Z step. Without one the step is
        // unknown: 0.0 is this crate's "not recorded" marker (the ND2/CZI/LIF
        // readers use it, and the OME-N layer treats <= 0 as unrecorded), so
        // a step the file never declared is never published.
        z: description
            .and_then(|description| description.spacing_um)
            .unwrap_or(0.0) as f32,
    })
}

/// The micrometres one unit of a resolution is worth, per the unit's name.
///
/// The names are the ones a microscope file uses; a unit this table does not
/// know names no scale, so the size is not published. Imperial names are left
/// out on purpose: `unit=inch` is a print layout, and gets the same refusal
/// as the ResolutionUnit=inch tag. The OME `PhysicalSize*Unit` attributes
/// convert through this table too.
pub(crate) fn micrometers_per_unit(unit: &str) -> Option<f64> {
    // The micro sign and the Greek letter mu are the same unit to a reader
    // that cares about the size, not the spelling.
    let key = unescape(unit).trim().to_lowercase().replace('μ', "µ");
    match key.as_str() {
        "µm" | "um" | "micron" | "microns" | "micrometer" | "micrometers" => Some(1.0),
        "nm" | "nanometer" | "nanometers" => Some(1e-3),
        "mm" | "millimeter" | "millimeters" => Some(1e3),
        "cm" | "centimeter" | "centimeters" => Some(1e4),
        _ => None,
    }
}

/// The text with its Java-style `\uXXXX` escapes resolved.
///
/// ImageJ reads its description back through `java.util.Properties`, which
/// resolves those escapes; a writer that cannot emit the micro sign leaves
/// the unit as the literal escape `unit=\u00B5m`, and an escaped unit that
/// stayed escaped would name no scale.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let escaped = bytes[index] == b'\\'
            && index + 6 <= bytes.len()
            && (bytes[index + 1] == b'u' || bytes[index + 1] == b'U');
        if escaped {
            if let Some(code) = text
                .get(index + 2..index + 6)
                .and_then(|digits| u32::from_str_radix(digits, 16).ok())
                .and_then(char::from_u32)
            {
                out.push(code);
                index += 6;
                continue;
            }
        }
        let character = text[index..].chars().next().unwrap_or('\u{fffd}');
        out.push(character);
        index += character.len_utf8();
    }
    out
}

/// What the ImageJ description states: the unit its resolution is counted in
/// and the Z step it names.
struct ImageJDescription {
    /// The micrometres one unit of the description's unit is worth.
    micrometers_per_unit: f64,
    /// The Z step the description states, in micrometres.
    spacing_um: Option<f64>,
}

/// Read the ImageJ description: the unit it names and the Z step it states.
///
/// A description that names no unit this reader can convert names no scale.
fn imagej_description(description: &str) -> Option<ImageJDescription> {
    let mut unit = None;
    let mut spacing = None;
    for line in description.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("unit=") {
            if unit.is_none() {
                unit = Some(value.trim());
            }
        } else if let Some(value) = line.strip_prefix("spacing=") {
            if spacing.is_none() {
                spacing = value.trim().parse::<f64>().ok();
            }
        }
    }
    let unit = unit?;
    let micrometers_per_unit = micrometers_per_unit(unit)?;
    let spacing_um = spacing
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|value| value * micrometers_per_unit);
    Some(ImageJDescription {
        micrometers_per_unit,
        spacing_um,
    })
}

/// True for a pixel size light microscopy could have measured.
fn is_plausible_um(micrometers: f64) -> bool {
    micrometers.is_finite() && (MIN_PLAUSIBLE_UM..=MAX_PLAUSIBLE_UM).contains(&micrometers)
}
