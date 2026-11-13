// SPDX-License-Identifier: Apache-2.0
//! N5 (Saalfeld lab / BigDataViewer): a dataset is a DIRECTORY holding `attributes.json` and one file per block,
//! at `g0/g1/…` (one directory per axis but the last). `region` stays the trait default: the plane is the unit the
//! decode ceiling prices.
//! A missing block is N5's zero block, not an error. v1 reads 2-D (x, y) and 3-D (x, y, z) datasets of uint8,
//! uint16, int16 or float32, raw, gzip or zstd, and maps no channel or time axis yet.

mod attributes;
mod block;
mod geometry;
mod read;
mod store;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::types::{ChannelMeta, ImageMeta, ImageReader, Plane, SceneMeta};

use self::attributes::Compression;
use self::geometry::Layout;
use self::store::Blocks;

pub struct N5Reader {
    meta: ImageMeta,
    layout: Layout,
    /// The dataset's compression: every block is decoded with the one the attributes state.
    compression: Compression,
    /// The dataset directory, under which every block file hangs.
    blocks: Blocks,
}

impl ImageReader for N5Reader {
    fn open(path: &Path) -> Result<Self> {
        let (root, file) = dataset(path)?;
        let attributes = attributes::read(&file)?;
        let layout = geometry::layout(&attributes)?;
        // Overflow only: the ceiling applies where a plane is materialised, because a dataset holds many planes.
        layout.dims().plane_bytes_no_ceiling(layout.dtype())?;
        let name = directory(&root);
        let scene = SceneMeta {
            dims: layout.dims(),
            dtype: layout.dtype(),
            samples: 1,
            px_size_um: attributes.calibration.px_size_um,
            notes: attributes.calibration.notes,
            channels: vec![ChannelMeta::default()],
            palette: None,
            name: name.clone(),
        };

        Ok(N5Reader {
            meta: ImageMeta {
                name,
                scenes: vec![scene],
            },
            layout,
            compression: attributes.compression,
            blocks: Blocks::new(&root),
        })
    }

    fn meta(&self) -> &ImageMeta {
        &self.meta
    }

    fn plane(&self, scene: usize, t: usize, c: usize, z: usize) -> Result<Plane> {
        self.read_plane(scene, t, c, z)
    }
}

/// The dataset directory and its `attributes.json`: the opened path is one or the other.
fn dataset(path: &Path) -> Result<(PathBuf, PathBuf)> {
    let attributes = match path.is_dir() {
        true => path.join("attributes.json"),
        false => path.to_path_buf(),
    };
    let root = attributes.parent().unwrap_or(Path::new("")).to_path_buf();
    Ok((root, attributes))
}

/// The dataset directory's name: a dataset opened by its `attributes.json` is named after the directory holding it.
fn directory(root: &Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("image")
        .to_string()
}
