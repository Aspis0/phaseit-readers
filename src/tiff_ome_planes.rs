// SPDX-License-Identifier: Apache-2.0
//! Where an OME scene's planes are: its `DimensionOrder` and the IFD of each plane, from its `<TiffData>` blocks.
//! The OME-XML shapes are ported from `tifffile` (Christoph Gohlke, BSD-3-Clause). Keep this attribution.

use crate::error::{Error, Result};
use crate::tiff_ome::TiffData;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Axis {
    T,
    C,
    Z,
}

#[derive(Debug, Clone)]
pub(crate) struct OmePlan {
    /// The inter-plane axes, fastest-varying first.
    pub order: [Axis; 3],
    pub nt: usize,
    pub nc: usize,
    pub nz: usize,
    /// The IFD of each plane, indexed by its flat `DimensionOrder` index.
    pub ifds: Vec<usize>,
}

fn number(raw: Option<&str>, default: usize) -> Result<usize> {
    match raw {
        None => Ok(default),
        Some(text) => text.trim().parse().map_err(|_| {
            Error::Parse(format!("OME <TiffData> attribute {text:?} is not a whole number"))
        }),
    }
}

pub(crate) const PLANE_ENTRY_BYTES: usize = std::mem::size_of::<Option<usize>>();

/// The table is sized from the declared `SizeT·SizeC·SizeZ`, so the cap is checked before anything is allocated.
pub(crate) const MAX_PLANE_TABLE_BYTES: usize = 16 * 1024 * 1024;

pub(crate) const MAX_PLANES_PER_SCENE: usize = MAX_PLANE_TABLE_BYTES / PLANE_ENTRY_BYTES;

fn declared_planes(nt: usize, nc: usize, nz: usize) -> Result<usize> {
    let requested = nt
        .checked_mul(nc)
        .and_then(|v| v.checked_mul(nz))
        .and_then(|planes| planes.checked_mul(PLANE_ENTRY_BYTES))
        .unwrap_or(usize::MAX);
    if requested > MAX_PLANE_TABLE_BYTES {
        return Err(Error::OversizedAllocation {
            requested,
            cap: MAX_PLANE_TABLE_BYTES,
        });
    }
    Ok(requested / PLANE_ENTRY_BYTES)
}

enum Blocks<'a> {
    Named(&'a [TiffData]),
    /// The spec's default: the file's IFDs, as one unnamed block.
    Default,
}

/// A later scene that omits `<TiffData>` does not say where its planes are, so it is refused.
fn plan_blocks<'a>(tiff_data: &'a [TiffData], first_scene: bool) -> Result<Blocks<'a>> {
    match (tiff_data.is_empty(), first_scene) {
        (false, _) => Ok(Blocks::Named(tiff_data)),
        (true, true) => Ok(Blocks::Default),
        (true, false) => Err(Error::Unsupported(
            "scene has no <TiffData>, so the IFDs of its planes are not stated".to_string(),
        )),
    }
}

/// The OME-TIFF `TiffData` rules and defaults: https://docs.openmicroscopy.org/ome-model/5/ome-tiff/specification.html
pub(crate) fn scene_plan(
    tiff_data: &[TiffData],
    root_uuid: Option<&str>,
    first_scene: bool,
    order: [Axis; 3],
    (nt, nc, nz): (usize, usize, usize),
    ifd_count: usize,
) -> Result<OmePlan> {
    let total = declared_planes(nt, nc, nz)?;
    let default_block = [TiffData::default()];
    let blocks: &[TiffData] = match plan_blocks(tiff_data, first_scene)? {
        Blocks::Named(blocks) => blocks,
        Blocks::Default => &default_block,
    };

    let mut ifds: Vec<Option<usize>> = vec![None; total];
    for block in blocks {
        fill_block(&mut ifds, block, root_uuid, order, (nt, nc, nz), ifd_count)?;
    }
    Ok(OmePlan {
        order,
        nt,
        nc,
        nz,
        ifds: finish_plan(ifds)?,
    })
}

/// Without a `PlaneCount`, a block names one plane if it has an IFD, else the file's IFDs; the run is cut at the
/// scene's end.
/// Overlaps, another file, or an IFD past the pages refuse the scene: the reader must not guess an IFD.
fn fill_block(
    ifds: &mut [Option<usize>],
    block: &TiffData,
    root_uuid: Option<&str>,
    order: [Axis; 3],
    (nt, nc, nz): (usize, usize, usize),
    ifd_count: usize,
) -> Result<()> {
    let total = ifds.len();
    if let (Some(uuid), Some(root)) = (block.uuid.as_deref(), root_uuid) {
        if uuid != root {
            return Err(Error::Unsupported(format!(
                "<TiffData> names another file ({uuid}); its planes are not read from this one"
            )));
        }
    }
    let first_t = number(block.first_t.as_deref(), 0)?;
    let first_c = number(block.first_c.as_deref(), 0)?;
    let first_z = number(block.first_z.as_deref(), 0)?;
    if first_t >= nt || first_c >= nc || first_z >= nz {
        return Err(Error::Parse(
            "OME <TiffData> starts outside the scene's planes".to_string(),
        ));
    }
    let ifd = number(block.ifd.as_deref(), 0)?;
    let start = ome_ifd_index(&order, first_t, first_c, first_z, nt, nc, nz)?;
    let count = match (block.plane_count.as_deref(), block.ifd.is_some()) {
        (Some(raw), _) => number(Some(raw), 0)?,
        (None, true) => 1,
        (None, false) => ifd_count.min(total - start),
    };
    let end = start
        .checked_add(count)
        .filter(|end| *end <= total)
        .ok_or_else(|| Error::Parse("OME <TiffData> runs past the scene's planes".to_string()))?;
    for (k, slot) in ifds[start..end].iter_mut().enumerate() {
        if slot.is_some() {
            return Err(Error::Parse(
                "two OME <TiffData> blocks state the same plane".to_string(),
            ));
        }
        let plane_ifd = ifd
            .checked_add(k)
            .ok_or_else(|| Error::Parse("OME <TiffData> IFD overflows".to_string()))?;
        if plane_ifd >= ifd_count {
            return Err(Error::Parse(format!(
                "OME <TiffData> names IFD {plane_ifd}, but the file has {ifd_count} pages"
            )));
        }
        *slot = Some(plane_ifd);
    }
    Ok(())
}

fn finish_plan(ifds: Vec<Option<usize>>) -> Result<Vec<usize>> {
    ifds.into_iter()
        .enumerate()
        .map(|(flat, ifd)| {
            ifd.ok_or_else(|| {
                Error::Parse(format!("no OME <TiffData> gives the IFD of plane {flat}"))
            })
        })
        .collect::<Result<Vec<usize>>>()
}

pub(crate) fn parse_dim_order(s: &str) -> Result<[Axis; 3]> {
    let mut axes: Vec<Axis> = Vec::with_capacity(3);
    for ch in s.trim().chars() {
        match ch.to_ascii_uppercase() {
            'X' | 'Y' => {}
            'T' => axes.push(Axis::T),
            'C' => axes.push(Axis::C),
            'Z' => axes.push(Axis::Z),
            other => {
                return Err(Error::Parse(format!(
                    "OME DimensionOrder {s:?} has unexpected axis {other:?}"
                )))
            }
        }
    }
    match axes.as_slice() {
        [a, b, c] => {
            // Must be a true permutation of {T, C, Z}: three tokens are not enough (`XYZZT` collects [Z, Z, T]),
            // and a duplicate would silently alias a whole axis onto one IFD.
            let (mut has_t, mut has_c, mut has_z) = (false, false, false);
            for axis in [a, b, c] {
                match axis {
                    Axis::T => has_t = true,
                    Axis::C => has_c = true,
                    Axis::Z => has_z = true,
                }
            }
            if has_t && has_c && has_z {
                Ok([*a, *b, *c])
            } else {
                Err(Error::Parse(format!(
                    "OME DimensionOrder {s:?} must be a permutation of T, C and Z"
                )))
            }
        }
        _ => Err(Error::Parse(format!(
            "OME DimensionOrder {s:?} must list exactly T, C and Z after X, Y"
        ))),
    }
}

pub(crate) fn ome_ifd_index(
    order: &[Axis; 3],
    t: usize,
    c: usize,
    z: usize,
    nt: usize,
    nc: usize,
    nz: usize,
) -> Result<usize> {
    let idx = |a: Axis| match a {
        Axis::T => t,
        Axis::C => c,
        Axis::Z => z,
    };
    let sz = |a: Axis| match a {
        Axis::T => nt,
        Axis::C => nc,
        Axis::Z => nz,
    };
    // Checked arithmetic: malformed OME-XML errors here instead of panicking (debug) or wrapping (release).
    let overflow = || Error::Decode("OME dimension index overflow".to_string());
    let inner = sz(order[1])
        .checked_mul(idx(order[2]))
        .and_then(|v| v.checked_add(idx(order[1])))
        .ok_or_else(overflow)?;
    sz(order[0])
        .checked_mul(inner)
        .and_then(|v| v.checked_add(idx(order[0])))
        .ok_or_else(overflow)
}

#[cfg(test)]
#[path = "tiff_ome_planes_tests.rs"]
mod tests;
