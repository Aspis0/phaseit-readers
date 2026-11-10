// SPDX-License-Identifier: Apache-2.0
//! The scene layout of a Zeiss LSM file: CZ-LSM-Info's position (`DimensionP`) and tile (`DimensionM`) counts multiply
//! into the number of separate acquisitions, and each owns its own pages. A file that states no counts holds one.
//! Multi-timepoint files (T > 1) are refused: the order of their time points is not established.

use crate::calibration::Calibration;
use crate::error::{Error, Result};
use crate::types::{ChannelMeta, DType, Dims5, SceneMeta};

#[derive(Debug, Clone, Copy)]
pub struct LsmLayout {
    scenes: usize,
    pages_per_scene: usize,
}

impl LsmLayout {
    /// A negative count means a malformed file: it is refused, never read as zero.
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

    pub fn scene_count(&self) -> usize {
        self.scenes
    }

    /// `None` when the product overflows.
    pub fn expected_pages(&self) -> Option<usize> {
        self.scenes.checked_mul(self.pages_per_scene)
    }

    /// Scenes own contiguous page blocks: the file stores all pages of one acquisition before the next.
    pub fn page_index(&self, scene: usize, z: usize) -> Option<usize> {
        if scene >= self.scenes {
            return None;
        }
        scene.checked_mul(self.pages_per_scene)?.checked_add(z)
    }
}

/// Several scenes are named as Bio-Formats names its series: `<file name> #1`, `#2`, and so on.
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
            samples: 1,
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
        let layout = LsmLayout::new(1, 49, 1).unwrap();
        assert_eq!(layout.scene_count(), 49);
        assert_eq!(layout.expected_pages(), Some(49));
    }

    #[test]
    fn scenes_own_contiguous_page_blocks() {
        let layout = LsmLayout::new(2, 3, 2).unwrap();
        assert_eq!(layout.scene_count(), 6);
        assert_eq!(layout.expected_pages(), Some(12));

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
        assert!(LsmLayout::new(-1, 49, 1).is_err());
        assert!(LsmLayout::new(1, -49, 1).is_err());
    }
}
