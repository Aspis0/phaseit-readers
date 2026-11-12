// SPDX-License-Identifier: Apache-2.0
//! ICS/IDS (Image Cytometry Standard) reader: one scene, uncompressed or gzip pixel data.
//! The header is text whose first two bytes are the field and the line separator; version 1.0 keeps the pixels in a
//! separate `.ids` file, 2.0 keeps them after `end`. The axes are the file's own `order`, fastest first, so a plane is
//! contiguous only when x and y come first and is gathered by strides otherwise.
//! U8, U16, I16 and F32 samples are read; every other sample type, compression and byte order is refused, never
//! converted. `region` stays the trait default: the plane is the unit the decode ceiling prices.

mod calibration;
mod header;
mod layout;
mod read;
#[cfg(test)]
mod tests;

use std::path::Path;

use memmap2::Mmap;

use crate::error::Result;
use crate::io::map_file;
use crate::types::{ChannelMeta, ImageMeta, ImageReader, Plane, SceneMeta};

use self::header::{header_path, load};
use self::layout::{layout, Layout};

/// Plain data plus the `Mmap` (`Send + Sync`): the reader is shareable across threads.
pub struct IcsReader {
    mmap: Mmap,
    meta: ImageMeta,
    layout: Layout,
}

impl ImageReader for IcsReader {
    fn open(path: &Path) -> Result<Self> {
        let header_file = header_path(path)?;
        let (header, data_file) = load(&header_file)?;
        let mmap = map_file(&data_file)?;
        // A deflated stream has no byte length to check: only the data itself knows where it ends.
        let file_bytes = match header.compression.as_str() {
            "gzip" => None,
            _ => Some(mmap.len()),
        };
        let layout = layout(&header, file_bytes)?;

        let calibration = calibration::calibrate(&header);
        let stem = file_stem(&header_file);
        let scene = SceneMeta {
            dims: layout.dims,
            dtype: layout.dtype,
            samples: 1,
            px_size_um: calibration.px_size_um,
            notes: calibration.notes,
            channels: vec![ChannelMeta::default(); layout.dims.c],
            palette: None,
            name: stem.clone(),
        };

        Ok(IcsReader {
            mmap,
            meta: ImageMeta {
                name: stem,
                scenes: vec![scene],
            },
            layout,
        })
    }

    fn meta(&self) -> &ImageMeta {
        &self.meta
    }

    fn plane(&self, scene: usize, t: usize, c: usize, z: usize) -> Result<Plane> {
        self.read_plane(scene, t, c, z)
    }
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image")
        .to_string()
}
