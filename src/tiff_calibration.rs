// SPDX-License-Identifier: Apache-2.0
//! A plain TIFF's pixel size, from its resolution tags or its ImageJ description (`unit=`, `spacing=`).
//! ImageJ's `spacing=` is the Z step, never a pixel size. An inch resolution (a print layout) or one with no
//! absolute unit states no pixel size. A pixel size is published only between 0.001 µm and 50 µm on BOTH axes.
//! ImageJ's resolution is pixels per the unit its description names: `unit=µm`, 13.658276 px/µm = 0.0732 µm/px.

use crate::calibration::Calibration;

const MIN_PLAUSIBLE_UM: f64 = 0.001;

const MAX_PLAUSIBLE_UM: f64 = 50.0;

const RESOLUTION_UNIT_NONE: u16 = 1;
const RESOLUTION_UNIT_INCH: u16 = 2;
const RESOLUTION_UNIT_CENTIMETER: u16 = 3;

const MICROMETERS_PER_CENTIMETER: f64 = 10_000.0;

/// The pixel size in µm, plus the notes the calibration could not give; no default is invented, only INCH earns a note.
pub fn calibration_from_tags(
    description: Option<&str>,
    x_per_unit: Option<f64>,
    y_per_unit: Option<f64>,
    resolution_unit: Option<u16>,
) -> Calibration {
    let description = description.and_then(imagej_description);
    if x_per_unit.is_none() && y_per_unit.is_none() {
        return Calibration::default();
    }

    // A stated but unusable count (0, negative, non-finite) is absurd, not absent: never replaced by the other axis.
    let unusable = |count: Option<f64>| matches!(count, Some(v) if !(v.is_finite() && v > 0.0));
    if unusable(x_per_unit) || unusable(y_per_unit) {
        return Calibration::default();
    }

    // The description names the unit only where the ResolutionUnit tag names none: the tag is the file's own statement.
    let micrometers_per_unit = match resolution_unit {
        Some(RESOLUTION_UNIT_INCH) => {
            let dpi = x_per_unit.or(y_per_unit).unwrap_or_default();
            return Calibration::uncalibrated(vec![print_resolution_note(dpi)]);
        }
        Some(RESOLUTION_UNIT_CENTIMETER) => MICROMETERS_PER_CENTIMETER,
        Some(RESOLUTION_UNIT_NONE) | None => {
            match description.as_ref() {
                Some(description) => description.micrometers_per_unit,
                None => return Calibration::default(),
            }
        }
        Some(_) => return Calibration::default(),
    };

    let x = x_per_unit.map(|count| micrometers_per_unit / count);
    let y = y_per_unit.map(|count| micrometers_per_unit / count);
    let (x, y) = match (x, y) {
        (Some(x), Some(y)) => (x, y),
        (Some(x), None) => (x, x),
        (None, Some(y)) => (y, y),
        (None, None) => return Calibration::default(),
    };

    if !is_plausible_um(x) || !is_plausible_um(y) {
        return Calibration::default();
    }

    Calibration::sized(
        x as f32,
        y as f32,
        description
            .and_then(|description| description.spacing_um)
            .map(|um| um as f32),
    )
}

/// The note an INCH resolution earns: a print layout, not a pixel size. Its wording lives here, once.
pub fn print_resolution_note(dpi: f64) -> String {
    format!(
        "the file states a print resolution of {dpi} dpi; it is not a microscope pixel size and was not used"
    )
}

/// Imperial names are left out on purpose: `unit=inch` is a print layout and gets the same refusal as the tag.
/// An unknown name yields `None`, so no scale is published. OME `PhysicalSize*Unit` also converts through here.
pub(crate) fn micrometers_per_unit(unit: &str) -> Option<f64> {
    // The micro sign and Greek mu are the same unit: normalize the spelling.
    let key = unescape(unit).trim().to_lowercase().replace('μ', "µ");
    match key.as_str() {
        "µm" | "um" | "micron" | "microns" | "micrometer" | "micrometers" => Some(1.0),
        "nm" | "nanometer" | "nanometers" => Some(1e-3),
        "mm" | "millimeter" | "millimeters" => Some(1e3),
        "cm" | "centimeter" | "centimeters" => Some(1e4),
        _ => None,
    }
}

/// Resolves Java-style `\uXXXX` escapes: ImageJ writes `unit=µm` where it cannot emit the micro sign.
/// An escaped unit left as is would name no scale.
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

struct ImageJDescription {
    micrometers_per_unit: f64,
    spacing_um: Option<f64>,
}

/// A unit this reader cannot convert names no scale.
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

fn is_plausible_um(micrometers: f64) -> bool {
    micrometers.is_finite() && (MIN_PLAUSIBLE_UM..=MAX_PLAUSIBLE_UM).contains(&micrometers)
}
