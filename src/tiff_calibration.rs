// SPDX-License-Identifier: Apache-2.0
//! A plain TIFF's pixel size, from its resolution tags or its ImageJ description (`unit=`, `spacing=`).
//! ImageJ's `spacing=` is the Z step, never a pixel size. An inch resolution (a print layout) or one with no
//! absolute unit states no pixel size. A pixel size is published only between 0.001 µm and 50 µm on BOTH axes.
//! ImageJ's resolution is pixels per the unit its description names: `unit=µm`, 13.658276 px/µm = 0.0732 µm/px.
//! A size outside the range is not published, and the note names the size the file stated and the range.

use crate::calibration::Calibration;

const MIN_PLAUSIBLE_UM: f64 = 0.001;

const MAX_PLAUSIBLE_UM: f64 = 50.0;

const RESOLUTION_UNIT_NONE: u16 = 1;
const RESOLUTION_UNIT_INCH: u16 = 2;
const RESOLUTION_UNIT_CENTIMETER: u16 = 3;

const MICROMETERS_PER_CENTIMETER: f64 = 10_000.0;

/// The scale a stated resolution is converted with: how many micrometres one unit of the file is, and the
/// name the file gives that unit.
struct Scale {
    micrometers_per_unit: f64,
    name: String,
}

/// The pixel size one stated resolution gives, and the axis that stated it.
#[derive(Clone, Copy)]
struct StatedSize {
    /// The size in micrometres per pixel.
    um: f64,
    /// The count the file states, in pixels per unit.
    count: f64,
    /// The axis the count was stated on: `X` or `Y`.
    axis: &'static str,
}

/// The pixel size in µm, plus the notes the calibration could not give: no default is invented, and
/// every count the file states is named.
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

    // A stated but unusable count (0, negative, non-finite) is absurd, not absent: it is named.
    let notes = unusable_count_notes(x_per_unit, y_per_unit);
    if !notes.is_empty() {
        return Calibration::uncalibrated(notes);
    }

    // An inch resolution is a print layout, not a pixel size.
    if resolution_unit == Some(RESOLUTION_UNIT_INCH) {
        let dpi = x_per_unit.or(y_per_unit).unwrap_or_default();
        return Calibration::uncalibrated(vec![print_resolution_note(dpi)]);
    }

    let Some(scale) = scale_of(resolution_unit, description.as_ref()) else {
        return Calibration::default();
    };

    let x = x_per_unit.map(|count| stated_size(count, &scale, "X"));
    let y = y_per_unit.map(|count| stated_size(count, &scale, "Y"));

    // Each stated count is named once: a count the file states on one axis is one fact about the file.
    let notes = range_notes([x, y].into_iter().flatten(), &scale);
    if !notes.is_empty() {
        return Calibration::uncalibrated(notes);
    }

    let (x, y) = match (x, y) {
        (Some(x), Some(y)) => (x, y),
        (Some(x), None) => (x, x),
        (None, Some(y)) => (y, y),
        (None, None) => return Calibration::default(),
    };

    let spacing = description
        .and_then(|description| description.spacing_um)
        .map(|um| um as f32);
    Calibration::sized(x.um as f32, y.um as f32, spacing)
}

/// The scale a stated resolution is converted with: how many micrometres one unit of the file is, and
/// the name the file gives that unit.
///
/// WHY the description is a fallback: the tag is the file's own statement of the unit.
fn scale_of(
    resolution_unit: Option<u16>,
    description: Option<&ImageJDescription>,
) -> Option<Scale> {
    match resolution_unit {
        Some(RESOLUTION_UNIT_CENTIMETER) => Some(Scale {
            micrometers_per_unit: MICROMETERS_PER_CENTIMETER,
            name: "centimetre".to_string(),
        }),
        Some(RESOLUTION_UNIT_NONE) | None => description.map(|description| Scale {
            micrometers_per_unit: description.micrometers_per_unit,
            name: description.unit.clone(),
        }),
        Some(_) => None,
    }
}

/// The notes for the counts a file states as unusable: a count of zero, a negative one or a
/// non-finite one is a stated absurdity, not an absence.
fn unusable_count_notes(x: Option<f64>, y: Option<f64>) -> Vec<String> {
    [("X", x), ("Y", y)]
        .into_iter()
        .filter_map(|(axis, count)| {
            let count = count?;
            if count.is_finite() && count > 0.0 {
                return None;
            }
            Some(format!(
                "the file states a resolution of {} on {axis}; a resolution must be a positive number of \
                 pixels per unit, so the image is read uncalibrated",
                short_number(count)
            ))
        })
        .collect()
}

/// The notes for the stated sizes that are outside the accepted range.
fn range_notes(sizes: impl Iterator<Item = StatedSize>, scale: &Scale) -> Vec<String> {
    sizes
        .filter(|size| !is_plausible_um(size.um))
        .map(|size| {
            pixel_size_range_note(
                size.axis,
                size.count,
                &scale.name,
                scale.micrometers_per_unit,
                size.um,
            )
        })
        .collect()
}

/// The size a stated count gives, in micrometres per pixel.
fn stated_size(count: f64, scale: &Scale, axis: &'static str) -> StatedSize {
    StatedSize {
        um: scale.micrometers_per_unit / count,
        count,
        axis,
    }
}

/// The note an out-of-range pixel size earns: the file states a size this reader cannot publish.
/// Its wording lives here, once, so the reader and the tests describe the size the same way.
fn pixel_size_range_note(
    axis: &str,
    count: f64,
    unit: &str,
    micrometers_per_unit: f64,
    um: f64,
) -> String {
    format!(
        "the file states a resolution of {} per {} on {}; {} um per {} gives {} um/px, outside the accepted \
         range of {} to {} um/px, so the image is read uncalibrated",
        short_number(count),
        unit,
        axis,
        short_number(micrometers_per_unit),
        unit,
        short_number(um),
        short_number(MIN_PLAUSIBLE_UM),
        short_number(MAX_PLAUSIBLE_UM),
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
    /// The unit as the description spells it: the note names the unit the file states.
    unit: String,
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
    // The unit is read as the file writes it: a writer that cannot emit the micro sign leaves an escape.
    let unit = unescape(unit);
    let micrometers_per_unit = micrometers_per_unit(&unit)?;
    let spacing_um = spacing
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|value| value * micrometers_per_unit);
    Some(ImageJDescription {
        micrometers_per_unit,
        unit,
        spacing_um,
    })
}

fn is_plausible_um(micrometers: f64) -> bool {
    micrometers.is_finite() && (MIN_PLAUSIBLE_UM..=MAX_PLAUSIBLE_UM).contains(&micrometers)
}

/// A number as the note states it: rounded, so a note carries the size and not its last digits. A value too
/// small for that form keeps its exponent, so a size of zero is never written where there is none.
fn short_number(value: f64) -> String {
    let text = format!("{value:.6}");
    let trimmed = text.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" || (trimmed == "0" && value != 0.0) {
        return format!("{value:e}");
    }
    trimmed.to_string()
}
