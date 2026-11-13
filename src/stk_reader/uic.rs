// SPDX-License-Identifier: Apache-2.0
//! UIC2's plane table and the UIC1 entries that state the pixel size.
//! A UIC1 entry names a file offset, and a file may name one that is not there: such an entry is ignored, never read
//! past the end of the file.

use crate::error::{Error, Result};
use crate::types::Endian;

use super::ifd::{self, Entry};

const ADDRESS_OVERFLOW: &str = "MetaMorph UIC2's value starts past the end of the address space";

/// The UIC1 entry ids this reader reads.
const X_CALIBRATION: u32 = 4;
const Y_CALIBRATION: u32 = 5;
const CALIBRATION_UNITS: u32 = 6;

/// The pixel size in µm, each axis `None` when the file states no distance this reader can use.
pub(super) struct Scale {
    pub x_um: Option<f32>,
    pub y_um: Option<f32>,
}

/// The number of planes UIC2 declares, once the value it declares is found inside the file.
pub(super) fn plane_count(buf: &[u8], uic2: &Entry) -> Result<usize> {
    if uic2.count == 0 {
        return Err(Error::Unsupported(
            "MetaMorph UIC2 states no plane: its count is 0".to_string(),
        ));
    }
    let size = value_size(uic2.kind)
        .and_then(|unit| uic2.count.checked_mul(unit))
        .ok_or_else(|| {
            Error::Unsupported(format!(
                "MetaMorph UIC2 declares the TIFF type {} this reader does not know",
                uic2.kind
            ))
        })?;
    // A value that fits in the entry's own four bytes is inside the file by construction.
    if size <= 4 {
        return Ok(uic2.count);
    }
    let end = uic2
        .at
        .checked_add(size)
        .ok_or_else(|| Error::Parse(ADDRESS_OVERFLOW.to_string()))?;
    if end > buf.len() {
        return Err(Error::Parse(format!(
            "MetaMorph UIC2's {size}-byte value at byte {} ends at byte {end}, past the end of the {}-byte file",
            uic2.at,
            buf.len()
        )));
    }
    Ok(uic2.count)
}

/// Bytes of one value of a TIFF field type; `None` for a type outside TIFF 6.0 and BigTIFF.
fn value_size(kind: u16) -> Option<usize> {
    match kind {
        1 | 2 | 6 | 7 => Some(1),
        3 | 8 => Some(2),
        4 | 5 | 9 | 11 => Some(4),
        10 | 12 | 16 | 17 | 18 => Some(8),
        _ => None,
    }
}

/// The pixel size the UIC1 entries state, in the unit its own CalibrationUnits entry names.
pub(super) fn scale(buf: &[u8], uic1: Option<Entry>, endian: Endian) -> Scale {
    let Some(uic1) = uic1 else {
        return Scale::none();
    };
    let mut x = None;
    let mut y = None;
    let mut units = None;
    // A count past the end of the file names pairs that are not there: the table is as long as the file can hold.
    for pair in 0..uic1.count.min(buf.len().saturating_sub(uic1.at) / 8) {
        let Some((id, value)) = pair_at(buf, uic1.at, pair, endian) else {
            break;
        };
        match id {
            X_CALIBRATION => x = rational(buf, value, endian),
            Y_CALIBRATION => y = rational(buf, value, endian),
            CALIBRATION_UNITS => units = unit_factor(buf, value, endian),
            _ => {}
        }
        // These three are the only values the scale needs: the rest of the table is not worth the walk.
        if x.is_some() && y.is_some() && units.is_some() {
            break;
        }
    }
    let Some(um_per_unit) = units else {
        return Scale::none();
    };
    Scale {
        x_um: x.and_then(|value| micrometers(value, um_per_unit)),
        y_um: y.and_then(|value| micrometers(value, um_per_unit)),
    }
}

impl Scale {
    fn none() -> Scale {
        Scale {
            x_um: None,
            y_um: None,
        }
    }
}

/// A rational in units per pixel as µm per pixel; `None` when the result is not a distance.
fn micrometers(value: f64, um_per_unit: f64) -> Option<f32> {
    let scaled = (value * um_per_unit) as f32;
    if scaled.is_finite() && scaled > 0.0 {
        Some(scaled)
    } else {
        None
    }
}

/// One `(id, value)` pair of the UIC1 table, `None` when it is not inside the file.
fn pair_at(buf: &[u8], at: usize, pair: usize, endian: Endian) -> Option<(u32, usize)> {
    let at = at.checked_add(pair.checked_mul(8)?)?;
    let id = ifd::u32_at(buf, at, endian).ok()?;
    let value = ifd::u32_at(buf, at.checked_add(4)?, endian).ok()? as usize;
    Some((id, value))
}

/// The rational at byte `at`, `None` when it is absent or its denominator is 0.
fn rational(buf: &[u8], at: usize, endian: Endian) -> Option<f64> {
    let numerator = ifd::u32_at(buf, at, endian).ok()? as f64;
    let denominator = ifd::u32_at(buf, at.checked_add(4)?, endian).ok()? as f64;
    if denominator == 0.0 {
        None
    } else {
        Some(numerator / denominator)
    }
}

/// µm per unit for the name at byte `at`: a uint32 length, then that many ASCII bytes.
/// The name is read in place and capped: no unit is this long, and copying a crafted one would price the open.
fn unit_factor(buf: &[u8], at: usize, endian: Endian) -> Option<f64> {
    let len = ifd::u32_at(buf, at, endian).ok()? as usize;
    if len > MAX_UNIT_BYTES {
        return None;
    }
    let start = at.checked_add(4)?;
    let raw = std::str::from_utf8(buf.get(start..start.checked_add(len)?)?).ok()?;
    um_per_unit(raw.trim_matches('\0').trim())
}

/// The longest CalibrationUnits name this reader reads; a longer one is no unit it knows.
const MAX_UNIT_BYTES: usize = 64;

/// µm per CalibrationUnits name. A name outside this table states no scale: MetaMorph writes several (inches, feet,
/// angstroms, ...) and a guessed factor would publish a wrong distance instead of none.
fn um_per_unit(unit: &str) -> Option<f64> {
    // The Greek mu and the micro sign are the same unit: normalize the spelling.
    match unit.trim().to_lowercase().replace('μ', "µ").as_str() {
        "µm" | "um" | "micron" | "microns" | "micrometer" | "micrometers" => Some(1.0),
        "nm" => Some(1e-3),
        _ => None,
    }
}
