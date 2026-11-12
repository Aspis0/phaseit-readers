// SPDX-License-Identifier: Apache-2.0
//! MRC2014 reader (electron microscopy, tomography, some light-microscopy exports): one scene, uncompressed sections.
//! A 1024-byte main header of 256 words, an optional extended header of NSYMBT bytes, then NZ sections of NY rows of
//! NX samples, x fastest. MODE 1, 2 and 6 are read; every other sample type is refused rather than converted.
//! `region` stays the trait default: a section is contiguous and could be read by rows, but the plane is the unit the
//! decode ceiling prices.

mod header;
mod layout;
mod read;
#[cfg(test)]
mod tests;

use std::path::Path;

use memmap2::Mmap;

use crate::calibration::Calibration;
use crate::error::Result;
use crate::io::map_file;
use crate::types::{ChannelMeta, DType, Endian, ImageMeta, ImageReader, Plane, SceneMeta};

use self::header::parse;
use self::layout::{layout, Layout};

/// Plain data plus the `Mmap` (`Send + Sync`): the reader is shareable across threads.
pub struct MrcReader {
    mmap: Mmap,
    meta: ImageMeta,
    layout: Layout,
    dtype: DType,
    endian: Endian,
}

impl ImageReader for MrcReader {
    fn open(path: &Path) -> Result<Self> {
        let mmap = map_file(path)?;
        let header = parse(&mmap)?;
        let layout = layout(&header, mmap.len())?;
        // No ceiling here: an over-ceiling section is published and refused by plane(), one at a time.
        layout.dims.plane_bytes_no_ceiling(header.dtype)?;

        let calibration = calibrate(&layout);
        let stem = file_stem(path);
        let scene = SceneMeta {
            dims: layout.dims,
            dtype: header.dtype,
            samples: 1,
            px_size_um: calibration.px_size_um,
            notes: calibration.notes,
            channels: vec![ChannelMeta::default()],
            palette: None,
            name: stem.clone(),
        };

        Ok(MrcReader {
            mmap,
            meta: ImageMeta {
                name: stem,
                scenes: vec![scene],
            },
            layout,
            dtype: header.dtype,
            endian: header.endian,
        })
    }

    fn meta(&self) -> &ImageMeta {
        &self.meta
    }

    fn plane(&self, scene: usize, t: usize, c: usize, z: usize) -> Result<Plane> {
        self.read_plane(scene, t, c, z)
    }
}

/// µm per pixel from CELLA/M. A missing x or y size is a note, not an invented number.
fn calibrate(layout: &Layout) -> Calibration {
    match (layout.x_um, layout.y_um) {
        (Some(x), Some(y)) => Calibration::sized(x, y, layout.z_um),
        _ => Calibration::uncalibrated(vec![
            "the file states no pixel size: CELLA or M is 0 along x or y".to_string(),
        ]),
    }
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image")
        .to_string()
}
