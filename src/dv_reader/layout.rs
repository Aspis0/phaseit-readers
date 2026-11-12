// SPDX-License-Identifier: Apache-2.0

use crate::error::{Error, Result};
use crate::types::Dims5;

use super::header::{DvHeader, HEADER_BYTES};

pub(super) struct Layout {
    pub dims: Dims5,
    pub x_um: Option<f32>,
    pub y_um: Option<f32>,
    pub z_um: Option<f32>,
    pub data_offset: usize,
    pub section_bytes: usize,
    pub section_order: SectionOrder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SectionOrder {
    Ztw,
    Wzt,
    Zwt,
}

pub(super) fn layout(header: &DvHeader, file_bytes: usize) -> Result<Layout> {
    let (dims, section_order) = geometry(header)?;
    let (data_offset, section_bytes) = window(header, file_bytes)?;
    Ok(Layout {
        dims,
        x_um: px_size(header.dx),
        y_um: px_size(header.dy),
        z_um: px_size(header.dz),
        data_offset,
        section_bytes,
        section_order,
    })
}

/// The one scene's axes: `W = NumWaves` channels, `T = NumTimes` time points, and `Z = NZ / (W·T)` sections.
fn geometry(header: &DvHeader) -> Result<(Dims5, SectionOrder)> {
    let c = count("NumWaves", header.num_waves)?;
    let t = count("NumTimes", header.num_times)?;
    if c > WAVELENGTH_SLOTS {
        return Err(Error::Unsupported(format!(
            "DeltaVision NumWaves is {c}: only five wavelength slots exist"
        )));
    }
    let sections = c
        .checked_mul(t)
        .ok_or_else(|| Error::Parse("DeltaVision W * T overflows usize".to_string()))?;
    let z = header.nz / sections;
    if z * sections != header.nz {
        return Err(Error::Unsupported(format!(
            "DeltaVision NZ {} is not divisible by W * T ({} * {})",
            header.nz, c, t
        )));
    }
    let section_order = match header.img_sequence {
        0 => SectionOrder::Ztw,
        1 => SectionOrder::Wzt,
        2 => SectionOrder::Zwt,
        other => {
            return Err(Error::Unsupported(format!(
                "DeltaVision ImgSequence {other} is not 0, 1 or 2"
            )))
        }
    };
    Ok((
        Dims5 {
            t,
            c,
            z,
            y: header.ny,
            x: header.nx,
        },
        section_order,
    ))
}

/// `(byte offset of section 0, bytes of one section)`, checked against the end of the file.
fn window(header: &DvHeader, file_bytes: usize) -> Result<(usize, usize)> {
    let section_bytes = header
        .nx
        .checked_mul(header.ny)
        .and_then(|px| px.checked_mul(header.dtype.size_bytes()))
        .ok_or_else(|| Error::Parse("DeltaVision section size overflows usize".to_string()))?;
    let data_offset = HEADER_BYTES
        .checked_add(header.next)
        .ok_or_else(|| Error::Parse("DeltaVision data offset overflows usize".to_string()))?;
    let end = section_bytes
        .checked_mul(header.nz)
        .and_then(|bytes| bytes.checked_add(data_offset))
        .ok_or_else(|| Error::Parse("DeltaVision data size overflows usize".to_string()))?;
    if end > file_bytes {
        return Err(Error::Parse(format!(
            "DeltaVision data ends at byte {end} ({} section(s) of {section_bytes} bytes at offset {data_offset}), \
             past the end of the {file_bytes}-byte file",
            header.nz
        )));
    }
    Ok((data_offset, section_bytes))
}

/// The five wavelength slots DeltaVision defines.
const WAVELENGTH_SLOTS: usize = 5;

/// A count of time points or channels: 0 means 1, a negative value is a corrupt header rather than a huge count.
fn count(name: &str, value: i16) -> Result<usize> {
    match value {
        0 => Ok(1),
        value if value > 0 => Ok(value as usize),
        value => Err(Error::Unsupported(format!(
            "DeltaVision {name} is {value}: a count cannot be negative"
        ))),
    }
}

/// µm per pixel along one axis; `None` when the file states no usable distance.
fn px_size(value: f32) -> Option<f32> {
    if value.is_finite() && value > 0.0 {
        Some(value)
    } else {
        None
    }
}
