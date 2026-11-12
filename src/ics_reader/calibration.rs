// SPDX-License-Identifier: Apache-2.0
//! µm per pixel from `parameter scale` and `parameter units`: a stated spacing counts only with a unit this reader
//! converts, so an unknown unit leaves the axis uncalibrated rather than guessed.

use crate::calibration::Calibration;

use super::header::IcsHeader;

/// The unit's factor to µm; `None` for a unit this reader does not convert.
fn unit_to_um(unit: &str) -> Option<f32> {
    match unit.trim().to_ascii_lowercase().as_str() {
        "micrometers" | "micrometer" | "microns" | "um" | "\u{b5}m" => Some(1.0),
        "nanometers" | "nm" => Some(0.001),
        "millimeters" | "mm" => Some(1000.0),
        _ => None,
    }
}

/// One axis's spacing in µm: `None` when the file states no scale, no unit, an unknown unit or a non-positive one.
fn axis_um(header: &IcsHeader, axis: &str) -> Option<f32> {
    let index = header.axes.iter().position(|name| name == axis)?;
    let factor = unit_to_um(header.units.get(index)?)?;
    let stated = header.scale.get(index).copied()?;
    if !stated.is_finite() || stated <= 0.0 {
        return None;
    }
    Some(stated * factor)
}

/// µm per pixel from `parameter scale` and `parameter units`; a missing x or y size is a note, not an invented number.
pub(super) fn calibrate(header: &IcsHeader) -> Calibration {
    match (axis_um(header, "x"), axis_um(header, "y")) {
        (Some(x), Some(y)) => Calibration::sized(x, y, axis_um(header, "z")),
        _ => Calibration::uncalibrated(vec![
            "the file states no pixel size: `parameter scale` or a unit this reader converts is missing along x or y"
                .to_string(),
        ]),
    }
}
