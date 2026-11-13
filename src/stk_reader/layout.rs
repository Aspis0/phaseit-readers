// SPDX-License-Identifier: Apache-2.0
//! Where the planes are: plane 0's strips must be contiguous, and every plane follows the previous one with no gap.

use crate::error::{Error, Result};
use crate::tiff_layout::ChunkTable;
use crate::types::Dims5;

use super::geometry::Geometry;

pub(super) struct Layout {
    pub dims: Dims5,
    pub data_offset: usize,
    pub plane_bytes: usize,
}

pub(super) fn layout(geometry: &Geometry, planes: usize, file_bytes: usize) -> Result<Layout> {
    let plane_dims = Dims5 {
        t: 1,
        c: 1,
        z: 1,
        y: geometry.height,
        x: geometry.width,
    };
    // Overflow only: the decode ceiling is `plane()`'s, one plane at a time.
    let plane_bytes = plane_dims.plane_bytes_no_ceiling(geometry.dtype)?;
    let data_offset = first_strip_offset(&geometry.strips, plane_bytes)?;
    let end = planes
        .checked_mul(plane_bytes)
        .and_then(|bytes| bytes.checked_add(data_offset))
        .ok_or_else(|| Error::Parse(ADDRESS_OVERFLOW.to_string()))?;
    if end > file_bytes {
        return Err(Error::Parse(format!(
            "MetaMorph STK's {planes} plane(s) of {plane_bytes} bytes at byte {data_offset} end at byte {end}, \
             past the end of the {file_bytes}-byte file"
        )));
    }
    Ok(Layout {
        dims: Dims5 {
            t: 1,
            c: 1,
            z: planes,
            y: geometry.height,
            x: geometry.width,
        },
        data_offset,
        plane_bytes,
    })
}

impl Layout {
    /// Plane `z` follows plane 0 with no gap, so one multiplication places every plane of the Z series.
    pub(super) fn plane_offset(&self, z: usize) -> Result<usize> {
        z.checked_mul(self.plane_bytes)
            .and_then(|at| at.checked_add(self.data_offset))
            .ok_or_else(|| Error::Decode(ADDRESS_OVERFLOW.to_string()))
    }
}

/// The byte plane 0 starts at, after its strips are found to be one contiguous run that covers the whole plane: the
/// planes are addressable by the plane formula only then.
fn first_strip_offset(strips: &ChunkTable, plane_bytes: usize) -> Result<usize> {
    let mut covered = 0u64;
    for index in 0..strips.offsets.len() {
        if index > 0 {
            let end = strips.offsets[index - 1]
                .checked_add(strips.bytes[index - 1])
                .ok_or_else(|| Error::Parse(ADDRESS_OVERFLOW.to_string()))?;
            if strips.offsets[index] != end {
                return Err(Error::Unsupported(format!(
                    "MetaMorph STK plane 0's strips are not contiguous: strip {index} starts at byte {} while                      strip {} ends at byte {end}",
                    strips.offsets[index],
                    index - 1
                )));
            }
        }
        covered = covered
            .checked_add(strips.bytes[index])
            .ok_or_else(|| Error::Parse(ADDRESS_OVERFLOW.to_string()))?;
    }
    if covered != plane_bytes as u64 {
        return Err(Error::Unsupported(format!(
            "MetaMorph STK plane 0's strips cover {covered} bytes, not the {plane_bytes} one plane is"
        )));
    }
    let at = *strips
        .offsets
        .first()
        .ok_or_else(|| Error::Decode("MetaMorph STK plane 0 states no strip offset".to_string()))?;
    usize::try_from(at).map_err(|_| {
        Error::Parse(format!(
            "MetaMorph STK plane 0 starts at byte {at}, which does not fit this machine's addresses"
        ))
    })
}

const ADDRESS_OVERFLOW: &str = "MetaMorph STK: a strip or plane offset overflows the address space";
