// SPDX-License-Identifier: Apache-2.0
//! CZI / ZISRAW reader: framing and offsets ported from `czifile` (C. Gohlke, BSD-3-Clause; keep this attribution).
//! A scene is a mosaic blitted at `(X.Start, Y.Start)`; a plane is drawn in ascending mosaic index only when its tiles
//! carry a non-negative `M`, as `czifile` does, because directory order would put another tile on top.
//! Compression 0, 5 and 6 are read; 1 and 2 are unsupported; 4 needs the `jpegxr` feature.

mod bytes;
mod codec;
mod directory;
mod metadata;
mod mosaic;
mod open;
#[cfg(test)]
mod probe;
mod read;
mod scene;
mod segment;
mod stored;
mod subblock;
#[cfg(test)]
mod tests;

use std::path::Path;
use std::sync::Arc;

use memmap2::Mmap;

use crate::error::Result;
use crate::types::{ImageMeta, ImageReader, PixelRect, Plane, RefusedScene, StoredPyramid};

use self::scene::CziScene;
use self::stored::CziStoredPyramid;
use self::subblock::SubBlock;

/// Plain data plus the `Mmap` (`Send + Sync`): the reader is shareable across threads.
pub struct CziReader {
    mmap: Arc<Mmap>,
    meta: ImageMeta,
    subblocks: Arc<[SubBlock]>,
    scenes: Vec<CziScene>,
    refused: Vec<RefusedScene>,
    /// Parallel to `meta.scenes`: each scene's RANK among full-resolution `S` values; pyramid-only scenes hold none.
    file_indices: Vec<usize>,
    /// Parallel to `meta.scenes`: the coarse levels of the scene, when it has them.
    stored: Vec<Option<CziStoredPyramid>>,
}

impl ImageReader for CziReader {
    fn open(path: &Path) -> Result<Self> {
        open::open_file(path)
    }

    fn meta(&self) -> &ImageMeta {
        &self.meta
    }

    fn refused_scenes(&self) -> &[RefusedScene] {
        &self.refused
    }

    fn scene_file_index(&self, scene: usize) -> usize {
        self.file_indices.get(scene).copied().unwrap_or(scene)
    }

    fn plane(&self, scene: usize, t: usize, c: usize, z: usize) -> Result<Plane> {
        self.read_plane(scene, t, c, z)
    }

    /// Out-of-core: only tiles meeting `rect` decode, so a whole-slide plane over the decode cap is never materialised.
    fn supports_region(&self) -> bool {
        true
    }

    fn region(
        &self,
        scene: usize,
        t: usize,
        c: usize,
        z: usize,
        rect: PixelRect,
    ) -> Result<Plane> {
        self.read_region(scene, t, c, z, rect)
    }

    /// The coarse levels the file stores for the scene.
    fn stored_pyramid(&self, scene: usize) -> Option<&dyn StoredPyramid> {
        self.stored
            .get(scene)?
            .as_ref()
            .map(|pyramid| pyramid as &dyn StoredPyramid)
    }
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image")
        .to_string()
}
