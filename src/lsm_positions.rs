// SPDX-License-Identifier: Apache-2.0
//! The scene layout of a Zeiss LSM file.
//!
//! One confocal acquisition can be split into several separate shots: a
//! mosaic shot as a grid of tiles, or a multi-position experiment where the
//! stage visits several places. CZ-LSM-Info records both counts —
//! `DimensionP` (stage positions) and `DimensionM` (tiles) — and their
//! product is the number of separate acquisitions the file holds. Each
//! acquisition owns its own pages in the file, so the product is also the
//! number of scenes the file is published as.
//!
//! A file that states no counts holds one acquisition and is published as one
//! scene. The pages of a scene run Z-ascending: the reader refuses a
//! multi-timepoint file (T > 1), because the order of its time points is not
//! established, so a scene's pages are exactly its Z planes.
//!
//! This module also builds the scene vector the reader publishes: one scene
//! per acquisition, each with the file's own geometry and channels.

use crate::calibration::Calibration;
use crate::error::{Error, Result};
use crate::types::{ChannelMeta, DType, Dims5, SceneMeta};

/// The tile/position layout of one LSM file.
///
/// Built from the counts CZ-LSM-Info states and the file's `Z`; it answers how
/// many scenes the file holds and which real page each scene's planes live
/// on.
#[derive(Debug, Clone, Copy)]
pub struct LsmLayout {
    /// How many separate acquisitions (scenes) the file holds.
    scenes: usize,
    /// The real pages one scene holds: its Z planes.
    pages_per_scene: usize,
}

impl LsmLayout {
    /// The layout of a file with the given counts and `Z` dimension.
    ///
    /// `positions` and `tiles` are the raw CZ-LSM-Info counts. A negative one
    /// means the file is malformed and is refused rather than read as zero.
    ///
    /// A file that states no counts at all holds one acquisition.
    pub fn new(positions: i32, tiles: i32, z: usize) -> Result<Self> {
        let positions = usize::try_from(positions).map_err(|_| {
            Error::Decode("LSM CZ-LSM-Info has a negative DimensionP (stage positions)".to_string())
        })?;
        let tiles = usize::try_from(tiles)
            .map_err(|_| Error::Decode("LSM CZ-LSM-Info has a negative DimensionM (tiles)".to_string()))?;
        Ok(Self {
            scenes: positions.saturating_mul(tiles).max(1),
            pages_per_scene: z,
        })
    }

    /// How many separate acquisitions (scenes) the file holds: one per
    /// position/tile pair.
    pub fn scene_count(&self) -> usize {
        self.scenes
    }

    /// The real pages the whole file must hold: `Z * P * M`.
    ///
    /// `None` when the product overflows.
    pub fn expected_pages(&self) -> Option<usize> {
        self.scenes.checked_mul(self.pages_per_scene)
    }

    /// The real page a scene's Z plane lives on.
    ///
    /// Scenes own contiguous blocks of pages: the file stores every page of
    /// the first acquisition, then every page of the second, and so on.
    ///
    /// `None` when `scene` is outside the file, or when the arithmetic
    /// overflows.
    pub fn page_index(&self, scene: usize, z: usize) -> Option<usize> {
        if scene >= self.scenes {
            return None;
        }
        scene.checked_mul(self.pages_per_scene)?.checked_add(z)
    }
}

/// The scene vector an LSM file publishes: one scene per acquisition.
///
/// A file with several scenes names them the way Bio-Formats names its
/// series: `<file name> #1`, `<file name> #2`, … A file with one scene keeps
/// the file's own name.
pub fn build_scenes(
    file_name: &str,
    layout: &LsmLayout,
    dims: Dims5,
    dtype: DType,
    calibration: Calibration,
    channels: Vec<ChannelMeta>,
) -> Vec<SceneMeta> {
    let scene_count = layout.scene_count();
    (0..scene_count)
        .map(|scene| SceneMeta {
            dims,
            dtype,
            samples: 1, // LSM planes are grayscale (one sample per pixel).
            px_size_um: calibration.px_size_um,
            notes: calibration.notes.clone(),
            channels: channels.clone(),
            palette: None,
            name: if scene_count == 1 {
                file_name.to_string()
            } else {
                format!("{file_name} #{}", scene + 1)
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_without_counts_holds_one_scene() {
        let layout = LsmLayout::new(0, 0, 5).unwrap();
        assert_eq!(layout.scene_count(), 1);
        assert_eq!(layout.expected_pages(), Some(5));
    }

    #[test]
    fn positions_and_tiles_multiply_into_scenes() {
        // A 7x7 tile mosaic: one position, 49 tiles, one page each.
        let layout = LsmLayout::new(1, 49, 1).unwrap();
        assert_eq!(layout.scene_count(), 49);
        assert_eq!(layout.expected_pages(), Some(49));
    }

    #[test]
    fn scenes_own_contiguous_page_blocks() {
        // Two positions, three tiles, two Z planes: 6 scenes of 2 pages each
        // = 12 pages.
        let layout = LsmLayout::new(2, 3, 2).unwrap();
        assert_eq!(layout.scene_count(), 6);
        assert_eq!(layout.expected_pages(), Some(12));

        // Scene 0 owns pages 0..2, scene 1 owns 2..4, and so on.
        assert_eq!(layout.page_index(0, 0), Some(0));
        assert_eq!(layout.page_index(0, 1), Some(1));
        assert_eq!(layout.page_index(1, 0), Some(2));
        assert_eq!(layout.page_index(5, 1), Some(11));
    }

    #[test]
    fn a_scene_outside_the_file_has_no_page() {
        let layout = LsmLayout::new(1, 49, 1).unwrap();
        assert_eq!(layout.page_index(48, 0), Some(48));
        assert_eq!(layout.page_index(49, 0), None);
    }

    #[test]
    fn negative_counts_are_refused() {
        // A negative count is a malformed file: the reader refuses it rather
        // than reading it as zero and publishing a scene the file never had.
        assert!(LsmLayout::new(-1, 49, 1).is_err());
        assert!(LsmLayout::new(1, -49, 1).is_err());
    }
}
