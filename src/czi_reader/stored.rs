//! The file's own pyramid: the coarse subblocks, as the levels they make.
//!
//! A Zeiss file may store a pyramid of its own: one set of subblocks per minification. Those
//! subblocks are the coarse levels of a pyramid build, without the full-resolution pixels: a
//! whole slide can show its overview from them alone.
//!
//! The geometry of those levels is in [`levels`]; the parts they do not cover are in [`fill`].

mod attribution;
mod coarse;
mod coverage;
mod fill;
mod grid;
mod levels;
mod snap;
mod tiles;

use std::collections::HashMap;
use std::sync::Arc;

use memmap2::Mmap;

use crate::error::{Error, Result};
use crate::types::{
    Dims5, PixelRect, Plane, PlaneBuf, RefusedLevel, StoredLevel, StoredPyramid,
    StoredRegion,
};

use crate::czi_reader::directory::RawDirEntry;
use crate::czi_reader::read::{compose_region, decode_subblock, DecodeCache};
use crate::czi_reader::scene::CziScene;
use crate::czi_reader::subblock::SubBlock;

use self::coarse::{read_coarse, Skipped};
use self::coverage::{Area, Coverage};
use self::fill::{fill, LevelPixels};
use self::levels::{collect_levels, LevelPlane};
use self::snap::{snap_rect, Snap};
use self::tiles::{blit_tile, tile_window};

/// A coarse subblock: the directory entry that describes it and its pixel span.
#[derive(Clone)]
pub(super) struct PyramidBlock {
    /// The minification the tile's stored size names.
    pub factor: usize,
    pub entry: RawDirEntry,
    pub block: SubBlock,
}

/// The coarse subblocks of the file, grouped by the scene they belong to.
pub(super) struct CoarseBlocks {
    /// The coarse subblocks of each scene.
    pub scenes: HashMap<i32, SceneCoarse>,
}

/// The coarse subblocks of one scene.
#[derive(Default)]
pub(super) struct SceneCoarse {
    /// The coarse subblocks that can be read.
    pub blocks: Vec<PyramidBlock>,
    /// The coarse subblocks that cannot be read.
    pub skipped: Vec<Skipped>,
}

/// The coarse subblocks of the file, read.
pub(super) fn collect_coarse(buf: &[u8], entries: &[RawDirEntry]) -> CoarseBlocks {
    let mut scenes: HashMap<i32, SceneCoarse> = HashMap::new();
    for entry in entries.iter().filter(|entry| entry.is_coarse()) {
        let scene = scenes.entry(entry.s).or_default();
        match read_coarse(buf, entry) {
            Ok(block) => scene.blocks.push(block),
            Err(skip) => scene.skipped.push(skip),
        }
    }
    CoarseBlocks { scenes }
}

/// The coverage of a plane that has no level-0 tile.
static EMPTY_COVERAGE: Coverage = Coverage::empty();

/// The full-resolution pixels of a scene's plane: what the uncovered parts are filled from.
pub(super) struct FullRes<'a> {
    bytes: &'a [u8],
    scene: &'a CziScene,
    subblocks: &'a [SubBlock],
    /// The scene's tiles of the plane.
    tiles: &'a [usize],
    /// The decoded tiles the fill reuses across its blocks.
    cache: DecodeCache,
}

impl FullRes<'_> {
    /// The scene's pixels over `rect`, composed from the tiles that meet it.
    pub(super) fn region(&mut self, rect: PixelRect) -> Result<Vec<u8>> {
        compose_region(
            self.bytes,
            self.scene,
            self.subblocks,
            self.tiles,
            rect,
            Some(&mut self.cache),
        )
    }
}

/// The levels one scene carries, and the tiles that fill them.
pub(super) struct CziStoredPyramid {
    bytes: Arc<Mmap>,
    subblocks: Arc<[SubBlock]>,
    /// The scene the levels belong to.
    scene: CziScene,
    /// The scene's index, for the errors the reads raise.
    scene_index: usize,
    /// The levels that are offered, finest first.
    levels: Vec<StoredLevel>,
    /// The planes of each level, by its minification.
    planes: HashMap<usize, Vec<LevelPlane>>,
    /// The levels that are not offered.
    refused: Vec<RefusedLevel>,
    /// The part of the scene the level-0 tiles cover, per plane: where the fill finds pixels.
    full_coverages: HashMap<(i32, i32, i32), Coverage>,
}

/// The levels of one scene, or `None` when the file stores no coarse subblock for it.
pub(super) fn build_stored_pyramid(
    bytes: Arc<Mmap>,
    subblocks: Arc<[SubBlock]>,
    coarse: &CoarseBlocks,
    scene: &CziScene,
    scene_index: usize,
    s_val: i32,
) -> Option<CziStoredPyramid> {
    let group = coarse.scenes.get(&s_val)?;
    let levels = collect_levels(&group.blocks, &group.skipped, scene);
    if levels.levels.is_empty() && levels.refused.is_empty() {
        return None;
    }
    let full_coverages = full_coverage(scene, &subblocks);
    Some(CziStoredPyramid {
        bytes,
        subblocks,
        scene: scene.clone(),
        scene_index,
        levels: levels.levels,
        planes: levels.planes,
        refused: levels.refused,
        full_coverages,
    })
}

/// The part of the scene the level-0 tiles cover, per plane: where the fill finds pixels.
fn full_coverage(
    scene: &CziScene,
    subblocks: &[SubBlock],
) -> HashMap<(i32, i32, i32), Coverage> {
    let mut out = HashMap::new();
    for (key, tiles) in &scene.tiles {
        let areas: Vec<Area> = tiles
            .iter()
            .map(|&index| {
                let tile = &subblocks[index];
                Area {
                    x: i64::from(tile.start_x) - i64::from(scene.origin_x),
                    y: i64::from(tile.start_y) - i64::from(scene.origin_y),
                    w: tile.stored_x as i64,
                    h: tile.stored_y as i64,
                }
            })
            .collect();
        out.insert(*key, Coverage::of(&areas));
    }
    out
}

impl StoredPyramid for CziStoredPyramid {
    fn levels(&self) -> &[StoredLevel] {
        &self.levels
    }

    fn refused_levels(&self) -> &[RefusedLevel] {
        &self.refused
    }

    fn region(
        &self,
        factor: usize,
        t: usize,
        c: usize,
        z: usize,
        rect: PixelRect,
    ) -> Result<StoredRegion> {
        let (plane, key) = self.plane_of(factor, t, c, z)?;
        let snap = snap_rect(plane.origin, factor, rect, (self.scene.dims.x, self.scene.dims.y))?;
        let dtype = self.scene.dtype;
        let samples = self.scene.samples;
        let dims = Dims5 {
            t: 1,
            c: 1,
            z: 1,
            y: snap.rows,
            x: snap.cols,
        };
        let bytes = dims.checked_plane_bytes_samples(dtype, samples)?;
        let mut composed = LevelPixels {
            pixels: vec![0u8; bytes],
            width: snap.cols,
            height: snap.rows,
            dtype,
            samples,
        };

        // The parts the level's tiles do not cover are filled from the full resolution first:
        // the tiles of the level own their own pixels and are written over them.
        let (mut full_res, full) = self.full_res(&key, &snap);
        let level = plane.coverage.shifted(-snap.x, -snap.y);
        fill(&level, &full, &snap, factor, &mut composed, &mut full_res)?;
        self.blit_tiles(plane, &snap, factor, &mut composed)?;

        // The buffer is taken over, not copied: a whole-scene strip read is tens of megabytes.
        let data = PlaneBuf::from_vec(dtype, composed.pixels)?;
        let plane = Plane::new_samples(data, [snap.rows, snap.cols], samples)?;
        Ok(StoredRegion {
            plane,
            origin: [snap.x, snap.y],
        })
    }
}

impl CziStoredPyramid {
    /// The scene plane's full-resolution pixels and where its tiles cover them, in the read
    /// frame's terms: what the fill of a read composes the level's holes from.
    fn full_res<'a>(&'a self, key: &(i32, i32, i32), snap: &Snap) -> (FullRes<'a>, Coverage) {
        let tiles = self
            .scene
            .tiles
            .get(key)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let full_res = FullRes {
            bytes: &self.bytes,
            scene: &self.scene,
            subblocks: &self.subblocks,
            tiles,
            cache: DecodeCache::default(),
        };
        let coverage = self
            .full_coverages
            .get(key)
            .unwrap_or(&EMPTY_COVERAGE)
            .shifted(-snap.x, -snap.y);
        (full_res, coverage)
    }
}

impl CziStoredPyramid {
    /// The plane of the level with this minification, and the acquisition coordinates it holds.
    fn plane_of(
        &self,
        factor: usize,
        t: usize,
        c: usize,
        z: usize,
    ) -> Result<(&LevelPlane, (i32, i32, i32))> {
        let Some(planes) = self.planes.get(&factor) else {
            return Err(Error::Unsupported(format!(
                "CZI has no stored level with minification {factor}"
            )));
        };
        let key = self.plane_key(t, c, z)?;
        let plane = planes
            .iter()
            .find(|plane| plane.key == key)
            .ok_or(Error::OutOfRange {
                scene: self.scene_index,
                t,
                c,
                z,
            })?;
        Ok((plane, key))
    }

    /// Writes the tiles of the level over the composed plane.
    fn blit_tiles(
        &self,
        plane: &LevelPlane,
        snap: &Snap,
        factor: usize,
        composed: &mut LevelPixels,
    ) -> Result<()> {
        let pixel_bytes = self.scene.dtype.size_bytes() * self.scene.samples as usize;
        for tile in &plane.tiles {
            // Decoding is the cost: a tile outside the read is skipped before it is decoded.
            let Some(window) = tile_window(tile, snap.x, snap.y, factor, composed.width, composed.height)
            else {
                continue;
            };
            let decoded = decode_subblock(&self.bytes, &tile.block)?;
            blit_tile(
                &mut composed.pixels,
                tile,
                &decoded,
                &window,
                composed.width,
                pixel_bytes,
            )?;
        }
        Ok(())
    }
}

impl CziStoredPyramid {
    /// The acquisition coordinates of a plane of the scene.
    fn plane_key(&self, t: usize, c: usize, z: usize) -> Result<(i32, i32, i32)> {
        let t_value = *self.scene.t_values.get(t).ok_or(Error::OutOfRange {
            scene: self.scene_index,
            t,
            c,
            z,
        })?;
        let c_value = *self.scene.c_values.get(c).ok_or(Error::OutOfRange {
            scene: self.scene_index,
            t,
            c,
            z,
        })?;
        let z_value = *self.scene.z_values.get(z).ok_or(Error::OutOfRange {
            scene: self.scene_index,
            t,
            c,
            z,
        })?;
        Ok((c_value, z_value, t_value))
    }
}
