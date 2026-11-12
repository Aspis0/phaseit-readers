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
    let w = if header.num_waves == 0 {
        1
    } else {
        header.num_waves as usize
    };
    let t = if header.num_times == 0 {
        1
    } else {
        header.num_times as usize
    };
    if w > 5 {
        return Err(Error::Unsupported(format!(
            "DeltaVision NumWaves is {w}: only five wavelength slots exist"
        )));
    }
    let z = header.nz / (w * t);
    if z * w * t != header.nz {
        return Err(Error::Unsupported(format!(
            "DeltaVision NZ {} is not divisible by W * T ({} * {})",
            header.nz, w, t
        )));
    }
    let order = match header.img_sequence {
        0 => SectionOrder::Ztw,
        1 => SectionOrder::Wzt,
        2 => SectionOrder::Zwt,
        other => {
            return Err(Error::Unsupported(format!(
                "DeltaVision ImgSequence {other} is not 0, 1 or 2"
            )))
        }
    };
    let dims = Dims5 {
        t,
        c: w,
        z,
        y: header.ny,
        x: header.nx,
    };
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
            "DeltaVision data ends at byte {end} ({} section(s) of {section_bytes} bytes at offset {data_offset}), past the end of the {file_bytes}-byte file",
            header.nz
        )));
    }
    Ok(Layout {
        dims,
        x_um: px_size(header.dx),
        y_um: px_size(header.dy),
        z_um: px_size(header.dz),
        data_offset,
        section_bytes,
        section_order: order,
    })
}

fn px_size(value: f32) -> Option<f32> {
    if value.is_finite() && value > 0.0 {
        Some(value)
    } else {
        None
    }
}
