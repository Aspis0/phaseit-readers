//! The geometry of the coarse subblocks: which levels they make, and whether they make them
//! cleanly.
//!
//! A level is offered only when its tiles make a clean grid over the scene: one minification, one
//! cell size, no overlapping tiles, every plane covered, and an edge that reaches the scene. The
//! parts the grid leaves uncovered are filled from the full resolution, not left black.

use std::collections::{BTreeMap, HashMap};

use crate::types::{PixelRect, RefusedLevel, StoredLevel};

use crate::czi_reader::scene::CziScene;
use crate::czi_reader::subblock::SubBlock;

use super::attribution::{orphan_refusal, skips_of, unplaced_skips, Refusal};
use super::coarse::Skipped;
use super::coverage::{Area, Coverage};
use super::grid::{
    cell_of, covers_the_scene, extent_of, lattice_origin, tiles_do_not_overlap,
    tiles_fit_their_cells,
};
use super::PyramidBlock;

/// The most tiles one stored level may hold: a crafted file must not make the checks costly.
const MAX_STORED_TILES: usize = 1_000_000;

/// One tile of a stored level: where it sits and which subblock holds its pixels.
#[derive(Clone)]
pub(super) struct StoredTile {
    /// The tile's origin, in source pixels, relative to the scene's origin.
    pub x: i64,
    pub y: i64,
    /// The tile's extent in source pixels: its logical size.
    pub source_w: i64,
    pub source_h: i64,
    /// The tile's stored size, in level pixels.
    pub w: usize,
    pub h: usize,
    /// The acquisition coordinates of the plane the tile belongs to.
    pub plane: (i32, i32, i32),
    /// The subblock that holds the tile's pixels.
    pub block: SubBlock,
}

/// The tiles of one plane of a level, and the part of the scene they cover.
pub(super) struct LevelPlane {
    /// The acquisition coordinates of the plane.
    pub key: (i32, i32, i32),
    /// The tiles of the plane.
    pub tiles: Vec<StoredTile>,
    /// The part of the scene the tiles cover.
    pub coverage: Coverage,
    /// The level's origin, in source pixels: the lattice a rect must sit on.
    pub origin: [i64; 2],
}

/// The levels the file's coarse subblocks make over a scene.
pub(super) struct Levels {
    /// The levels that are offered, finest first.
    pub levels: Vec<StoredLevel>,
    /// The planes of each level, by its minification.
    pub planes: HashMap<usize, Vec<LevelPlane>>,
    /// The levels that are not offered, with the reason each was refused.
    pub refused: Vec<RefusedLevel>,
}

/// The levels the coarse subblocks make over one scene.
///
/// WHY a level is dropped: its pixels would be shifted, doubled or missing, so the scene keeps
/// the full-resolution path.
pub(super) fn collect_levels(
    blocks: &[PyramidBlock],
    skipped: &[Skipped],
    scene: &CziScene,
) -> Levels {
    let (tiles, cells) = grouped_tiles(blocks, skipped, scene);

    let mut levels = Vec::new();
    let mut planes = HashMap::new();
    let mut refused = Vec::new();
    for (factor, group) in tiles {
        let skips = skips_of(factor, skipped, &cells);
        match level_of(factor, &group, &skips, scene) {
            Ok(level) => {
                levels.push(level.geometry);
                planes.insert(factor, level.planes);
            }
            Err(refusal) => refused.push(RefusedLevel {
                factor,
                skipped: refusal.skipped,
                reason: refusal.reason,
            }),
        }
    }
    // A skipped tile whose size is no level's cell belongs to a level that never materialized:
    // it is a refusal of its own, not a reason to refuse the levels that exist.
    for (size, skips) in unplaced_skips(skipped, &cells) {
        refused.push(orphan_refusal(size, &skips));
    }
    Levels {
        levels,
        planes,
        refused,
    }
}

/// The tiles and the cell of each level the coarse subblocks make, by the level's minification.
///
/// A level whose every tile was skipped is a level that is refused, not a level that is absent,
/// so its empty group is kept.
fn grouped_tiles(
    blocks: &[PyramidBlock],
    skipped: &[Skipped],
    scene: &CziScene,
) -> (
    BTreeMap<usize, Vec<StoredTile>>,
    BTreeMap<usize, [usize; 2]>,
) {
    let mut groups: BTreeMap<usize, Vec<&PyramidBlock>> = BTreeMap::new();
    for block in blocks {
        groups.entry(block.factor).or_default().push(block);
    }
    for skip in skipped {
        if let Some(factor) = skip.factor {
            groups.entry(factor).or_default();
        }
    }
    let tiles: BTreeMap<usize, Vec<StoredTile>> = groups
        .iter()
        .map(|(factor, group)| {
            (
                *factor,
                group.iter().map(|block| tile_of(block, scene)).collect(),
            )
        })
        .collect();
    let cells = tiles
        .iter()
        .map(|(factor, group)| (*factor, cell_of(group)))
        .collect();
    (tiles, cells)
}

/// A level of the file's pyramid: its geometry and the tiles that fill it.
struct Level {
    geometry: StoredLevel,
    planes: Vec<LevelPlane>,
}

/// The level the tiles make, or the refusal of it.
fn level_of(
    factor: usize,
    tiles: &[StoredTile],
    skips: &[String],
    scene: &CziScene,
) -> Result<Level, Refusal> {
    refuse_skipped(skips)?;
    refuse_wrong_format(tiles, scene)?;
    if tiles.len() > MAX_STORED_TILES {
        return Err(Refusal::new(format!(
            "the level holds {} tiles",
            tiles.len()
        )));
    }

    let cell = cell_of(tiles);
    let Some(origin) = lattice_origin(tiles, &cell) else {
        return Err(Refusal::new(
            "the tiles of the level do not sit on one grid",
        ));
    };
    if !tiles_fit_their_cells(tiles, &cell, &origin) {
        return Err(Refusal::new(
            "a tile of the level does not fit the cell it sits in",
        ));
    }
    if !tiles_do_not_overlap(tiles, &cell, &origin) {
        return Err(Refusal::new("the tiles of the level overlap"));
    }
    let extent = extent_of(tiles);
    if !covers_the_scene(tiles, &extent, &cell, scene) {
        return Err(Refusal::new(
            "the tiles of the level stop more than a cell short of the scene",
        ));
    }
    let planes = planes_of(tiles, scene, origin)?;

    Ok(Level {
        geometry: StoredLevel {
            factor,
            cell,
            origin,
            extent,
            coverage_share: coverage_share(&planes, scene),
        },
        planes,
    })
}

/// The share of the scene's area the level's own tiles cover, for the plane they cover worst.
///
/// WHY the worst plane: a read of the level fills every plane's holes, so the plane with the
/// least coverage sets what a read costs.
fn coverage_share(planes: &[LevelPlane], scene: &CziScene) -> f32 {
    let scene_area = scene.dims.x as f64 * scene.dims.y as f64;
    if scene_area == 0.0 {
        return 0.0;
    }
    let scene_rect = PixelRect::new(0, 0, scene.dims.x, scene.dims.y);
    planes
        .iter()
        .map(|plane| plane.coverage.area(scene_rect) as f64 / scene_area)
        .fold(f64::MAX, f64::min) as f32
}

/// The refusal of a level that holds a tile the reader could not read.
fn refuse_skipped(skips: &[String]) -> Result<(), Refusal> {
    if skips.is_empty() {
        return Ok(());
    }
    Err(Refusal {
        skipped: skips.len(),
        reason: format!(
            "{} tile(s) of the pyramid could not be read: {}",
            skips.len(),
            skips.join(", ")
        ),
    })
}

/// The refusal of a level whose tiles do not carry the scene's pixel format.
///
/// WHY: a level of another format would be written with the wrong number of samples.
fn refuse_wrong_format(tiles: &[StoredTile], scene: &CziScene) -> Result<(), Refusal> {
    let mismatched = tiles
        .iter()
        .filter(|tile| tile.block.dtype != scene.dtype || tile.block.samples != scene.samples)
        .count();
    if mismatched == 0 {
        return Ok(());
    }
    Err(Refusal {
        skipped: mismatched,
        reason: format!(
            "{mismatched} tile(s) of the level carry a pixel format the scene does not"
        ),
    })
}

/// One tile of a level, in the scene's coordinates.
fn tile_of(block: &PyramidBlock, scene: &CziScene) -> StoredTile {
    StoredTile {
        x: i64::from(block.entry.start_x) - i64::from(scene.origin_x),
        y: i64::from(block.entry.start_y) - i64::from(scene.origin_y),
        source_w: block.entry.size_x as i64,
        source_h: block.entry.size_y as i64,
        w: block.entry.stored_x,
        h: block.entry.stored_y,
        plane: (block.entry.c, block.entry.z, block.entry.t),
        block: block.block.clone(),
    }
}

/// The planes of the level, and the coverage of each.
///
/// WHY every plane is needed: a level that holds one channel of a scene of two would show the
/// other channel as the fill, not as the level's own pixels.
fn planes_of(
    tiles: &[StoredTile],
    scene: &CziScene,
    origin: [i64; 2],
) -> Result<Vec<LevelPlane>, Refusal> {
    let mut planes: Vec<LevelPlane> = Vec::new();
    for tile in tiles {
        match planes.iter_mut().find(|plane| plane.key == tile.plane) {
            Some(plane) => plane.tiles.push(tile.clone()),
            None => planes.push(LevelPlane {
                key: tile.plane,
                tiles: vec![tile.clone()],
                coverage: Coverage::empty(),
                origin,
            }),
        }
    }
    for plane in &mut planes {
        if !scene_has_plane(scene, plane.key) {
            return Err(Refusal {
                skipped: 0,
                reason: format!(
                    "the level holds the plane (c={}, z={}, t={}) the scene does not have",
                    plane.key.0, plane.key.1, plane.key.2
                ),
            });
        }
    }
    for c in &scene.c_values {
        for z in &scene.z_values {
            for t in &scene.t_values {
                let key = (*c, *z, *t);
                if !planes.iter().any(|plane| plane.key == key) {
                    return Err(Refusal {
                        skipped: 1,
                        reason: format!(
                            "the level has no tile for the plane (c={c}, z={z}, t={t}) of the scene"
                        ),
                    });
                }
            }
        }
    }
    for plane in &mut planes {
        plane.coverage = Coverage::of(&areas_of(&plane.tiles));
    }
    Ok(planes)
}

/// The areas the tiles cover, in the source pixels of the scene.
fn areas_of(tiles: &[StoredTile]) -> Vec<Area> {
    tiles
        .iter()
        .map(|tile| Area {
            x: tile.x,
            y: tile.y,
            w: tile.source_w,
            h: tile.source_h,
        })
        .collect()
}

/// Whether the scene has the plane with these acquisition coordinates.
fn scene_has_plane(scene: &CziScene, key: (i32, i32, i32)) -> bool {
    scene.c_values.contains(&key.0)
        && scene.z_values.contains(&key.1)
        && scene.t_values.contains(&key.2)
}
