use crate::error::{Error, Result};

use super::dims::Dims5;
use super::pixel::DType;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChannelMeta {
    pub name: Option<String>,
    pub color: Option<[u8; 3]>,
}

/// Pixel size in µm as a named struct, so Z, Y and X cannot be transposed by a caller.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PxSizeUm {
    /// Z step in µm; `None` when the file states none or a placeholder.
    pub z: Option<f32>,
    pub y: f32,
    pub x: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SceneMeta {
    pub dims: Dims5,
    pub dtype: DType,
    /// Samples per pixel (`1` grey, `3` RGB). CZI and TIFF interleave colour samples; LSM and LIF map them to `dims.c`.
    pub samples: u32,
    pub px_size_um: Option<PxSizeUm>,
    /// Calibration the file stated but the reader could not use: no file path, no pixel value.
    pub notes: Vec<String>,
    pub channels: Vec<ChannelMeta>,
    /// The palette of an indexed scene: `palette[i]` is the RGB of index `i`; `None` otherwise.
    pub palette: Option<Vec<[u8; 3]>>,
    pub name: String,
}

/// Image-level metadata cached at `open()`; multi-scene formats build one entry per scene.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageMeta {
    pub name: String,
    pub scenes: Vec<SceneMeta>,
}

impl ImageMeta {
    pub fn scene_count(&self) -> usize {
        self.scenes.len()
    }

    pub fn scene(&self, scene: usize) -> Result<&SceneMeta> {
        self.scenes.get(scene).ok_or(Error::SceneOutOfRange {
            scene,
            count: self.scenes.len(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusedScene {
    pub image_index: usize,
    pub name: String,
    pub reason: String,
}

impl std::fmt::Display for RefusedScene {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<Image> {} \"{}\": {}", self.image_index, self.name, self.reason)
    }
}
