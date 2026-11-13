// SPDX-License-Identifier: Apache-2.0
//! MetaMorph STK (Molecular Devices `.stk`): one scene, a Z series of uncompressed 8- or 16-bit planes.
//! The file is a classic TIFF whose first IFD describes plane 0 and carries the private UIC tags: UIC2's COUNT is the
//! number of planes, and the planes follow plane 0 with no gap.
//! Calibration comes from UIC1's X/Y entries and only in a unit this reader converts. The UIC2 z distance is not
//! published: what it measures varies between writers. `region` stays the trait default: a plane is contiguous.

mod geometry;
mod ifd;
mod layout;
mod read;
#[cfg(test)]
mod tests;
mod uic;

use std::path::Path;

use memmap2::Mmap;

use crate::calibration::Calibration;
use crate::error::{Error, Result};
use crate::io::map_file;
use crate::types::{ChannelMeta, DType, Endian, ImageMeta, ImageReader, Plane, SceneMeta};

use self::geometry::geometry;
use self::layout::{layout, Layout};
use self::uic::Scale;

/// Plain data plus the `Mmap` (`Send + Sync`): the reader is shareable across threads.
pub struct StkReader {
    mmap: Mmap,
    meta: ImageMeta,
    layout: Layout,
    dtype: DType,
    endian: Endian,
}

impl ImageReader for StkReader {
    fn open(path: &Path) -> Result<Self> {
        let mmap = map_file(path)?;
        let endian = ifd::byte_order(&mmap)?;
        let uic2 = ifd::entry(&mmap, endian, ifd::UIC2)?
            .ok_or_else(|| Error::Unsupported(NO_UIC2.to_string()))?;
        let planes = uic::plane_count(&mmap, &uic2)?;
        let scale = uic::scale(&mmap, ifd::entry(&mmap, endian, ifd::UIC1)?, endian);
        let geometry = geometry(&mmap)?;
        let layout = layout(&geometry, planes, mmap.len())?;

        let calibration = calibrate(&scale);
        let stem = file_stem(path);
        let scene = SceneMeta {
            dims: layout.dims,
            dtype: geometry.dtype,
            samples: 1,
            px_size_um: calibration.px_size_um,
            notes: calibration.notes,
            channels: vec![ChannelMeta::default()],
            palette: None,
            name: stem.clone(),
        };

        Ok(StkReader {
            mmap,
            meta: ImageMeta {
                name: stem,
                scenes: vec![scene],
            },
            layout,
            dtype: geometry.dtype,
            endian,
        })
    }

    fn meta(&self) -> &ImageMeta {
        &self.meta
    }

    fn plane(&self, scene: usize, t: usize, c: usize, z: usize) -> Result<Plane> {
        self.read_plane(scene, t, c, z)
    }
}

/// The TIFF route asks this before it commits to [`crate::tiff_reader::TiffReader`]: a first IFD carrying UIC2 makes
/// the file an STK, whatever its name. A header this sniff cannot read is the TIFF route's own error, not this one's.
pub(crate) fn carries_uic2(path: &Path) -> Result<bool> {
    let mmap = map_file(path)?;
    let Ok(endian) = ifd::byte_order(&mmap) else {
        return Ok(false);
    };
    Ok(matches!(ifd::entry(&mmap, endian, ifd::UIC2), Ok(Some(_))))
}

/// µm per pixel from UIC1's X/Y entries, in the unit its own CalibrationUnits entry names. A file that states no usable
/// pair is published uncalibrated: a scale is never guessed.
fn calibrate(scale: &Scale) -> Calibration {
    match (scale.x_um, scale.y_um) {
        (Some(x), Some(y)) => Calibration::sized(x, y, None),
        _ => Calibration::uncalibrated(vec![NO_SCALE.to_string()]),
    }
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image")
        .to_string()
}

const NO_UIC2: &str =
    "MetaMorph STK: the first IFD carries no UIC2 (33629) plane table, so the file is not an STK";

const NO_SCALE: &str = "the file states no pixel size this reader can use: UIC1's XCalibration, YCalibration and \
                        CalibrationUnits must all be present and name a distance in a unit it converts";
