// SPDX-License-Identifier: Apache-2.0
//! The section geometry of an MRC file: how ISPG splits NZ into time points and sections, and where the data starts.
//! The sizes are checked before the first byte is decoded, so a corrupt header cannot ask for an allocation.

use crate::error::{Error, Result};
use crate::types::Dims5;

use super::header::{MrcHeader, HEADER_BYTES};

/// 1 Å in µm: CELLA is stated in ångström, [`PxSizeUm`] in µm.
const ANGSTROM_UM: f32 = 1e-4;

pub(super) struct Layout {
    pub dims: Dims5,
    pub x_um: Option<f32>,
    pub y_um: Option<f32>,
    /// µm along z, `None` for a stack of images that states no volume.
    pub z_um: Option<f32>,
    /// Byte offset of section 0: after the main header and the extended header.
    pub data_offset: usize,
    /// Bytes of one section.
    pub section_bytes: usize,
}

pub(super) fn layout(header: &MrcHeader, file_bytes: usize) -> Result<Layout> {
    let (t, z, z_um) = stack(header)?;
    let dims = Dims5 {
        t,
        c: 1,
        z,
        y: header.ny,
        x: header.nx,
    };
    let section_bytes = section_bytes(header)?;
    let data_offset = HEADER_BYTES
        .checked_add(header.nsymbt)
        .ok_or_else(|| Error::Parse("MRC data offset overflows usize".to_string()))?;
    let end = section_bytes
        .checked_mul(header.nz)
        .and_then(|bytes| bytes.checked_add(data_offset))
        .ok_or_else(|| Error::Parse("MRC data size overflows usize".to_string()))?;
    if end > file_bytes {
        return Err(Error::Parse(format!(
            "MRC data ends at byte {end} ({} section(s) of {section_bytes} bytes at offset {data_offset}), \
             past the end of the {file_bytes}-byte file",
            header.nz
        )));
    }

    Ok(Layout {
        dims,
        x_um: px_size(header.cella[0], header.mx),
        y_um: px_size(header.cella[1], header.my),
        z_um,
        data_offset,
        section_bytes,
    })
}

/// `(t, z, z step)` of the one scene: ISPG 0 is a stack of images, 1-230 one volume, 401-630 a stack of volumes.
fn stack(header: &MrcHeader) -> Result<(usize, usize, Option<f32>)> {
    let nz = header.nz;
    match header.ispg {
        0 => Ok((1, nz, None)),
        1..=230 => Ok((1, nz, px_size(header.cella[2], header.mz))),
        401..=630 => {
            let mz = header.mz;
            if mz <= 0 {
                return Err(Error::Unsupported(format!(
                    "MRC ISPG {} stacks volumes but MZ is {mz}, so NZ {nz} cannot be split into volumes",
                    header.ispg
                )));
            }
            let volumes = nz / mz as usize;
            if volumes * mz as usize != nz {
                return Err(Error::Unsupported(format!(
                    "MRC ISPG {} stacks volumes of MZ {mz} sections, which does not divide NZ {nz}",
                    header.ispg
                )));
            }
            Ok((volumes, mz as usize, px_size(header.cella[2], header.mz)))
        }
        other => Err(Error::Unsupported(format!(
            "MRC ISPG {other} is not a stacking mode this reader knows (0, 1-230 or 401-630)"
        ))),
    }
}

/// Å per pixel along one axis; `None` when the file states no cell size or no sampling along it.
fn px_size(cella: f32, m: i32) -> Option<f32> {
    if cella > 0.0 && m > 0 {
        Some(cella / m as f32 * ANGSTROM_UM)
    } else {
        None
    }
}

fn section_bytes(header: &MrcHeader) -> Result<usize> {
    header
        .nx
        .checked_mul(header.ny)
        .and_then(|px| px.checked_mul(header.dtype.size_bytes()))
        .ok_or_else(|| Error::Parse("MRC section size overflows usize".to_string()))
}
