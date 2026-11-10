use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use crate::calibration::Calibration;
use crate::error::{Error, Result};
use crate::io::map_file;
use crate::types::{ChannelMeta, Dims5, ImageMeta, RefusedScene, SceneMeta};

use super::codec::pixel_type_to_dtype;
use super::directory::{parse_directory, Coord, RawDirEntry};
use super::file_stem;
use super::metadata::parse_metadata;
use super::mosaic::{scene_bbox, sorted_distinct};
use super::scene::{lost_tiles_reason, CziScene, SceneSkips};
use super::segment::parse_file_header;
use super::stored::{self, collect_coarse};
use super::subblock::{subblock_pixel_span, SubBlock};
use super::CziReader;

/// Reads a CZI file's directory, metadata and scene layout; a scene that cannot be read exactly is refused by name.
pub(super) fn open_file(path: &Path) -> Result<CziReader> {
    let mmap = Arc::new(map_file(path)?);
    let name = file_stem(path);
    let buf = &mmap[..];

    let (dir_pos, meta_pos) = parse_file_header(buf)?;

    // Best-effort: the metadata XML never gates decoding.
    let (channel_names, calibration) = parse_metadata(buf, meta_pos).unwrap_or_default();

    let raw_entries = parse_directory(buf, dir_pos)?;
    let (subblocks, coords, skips) = collect_subblocks(buf, &raw_entries);
    let subblocks: Arc<[SubBlock]> = subblocks.into();
    let coarse = collect_coarse(buf, &raw_entries);
    let scene_tiles = full_res_scene_counts(&raw_entries);
    if scene_tiles.is_empty() {
        return Err(Error::Unsupported(
            "CZI has no full-resolution pixel subblocks".to_string(),
        ));
    }
    let by_scene = group_by_scene(&coords);
    let parts = FileParts {
        name: &name,
        subblocks: &subblocks,
        channel_names: &channel_names,
        calibration: &calibration,
    };
    let single_scene = scene_tiles.len() == 1;

    let mut scenes: Vec<CziScene> = Vec::new();
    let mut scene_metas: Vec<SceneMeta> = Vec::new();
    let mut refused: Vec<RefusedScene> = Vec::new();
    let mut file_indices: Vec<usize> = Vec::new();
    let mut stored_pyramids: Vec<Option<stored::CziStoredPyramid>> = Vec::new();
    for (scene_idx, (&s_val, &total_tiles)) in scene_tiles.iter().enumerate() {
        let members: &[&Coord] = by_scene
            .get(&s_val)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let scene_skips = skips.get(&s_val).copied().unwrap_or_default();
        match compose_scene(
            &parts,
            SceneSlot {
                index: scene_idx,
                s_val,
                single_scene,
                total_tiles,
            },
            members,
            scene_skips,
        )? {
            SceneOutcome::Published(published) => {
                let (meta, scene) = *published;
                file_indices.push(scene_idx);
                scene_metas.push(meta);
                stored_pyramids.push(stored::build_stored_pyramid(
                    mmap.clone(),
                    subblocks.clone(),
                    &coarse,
                    &scene,
                    scene_idx,
                    s_val,
                ));
                scenes.push(scene);
            }
            SceneOutcome::Refused(refusal) => refused.push(refusal),
        }
    }

    // Fails only when every scene was dropped; the error names each refusal with its own reason, so a lost tile
    // is not called a pixel-type clash.
    if scenes.is_empty() {
        let reasons: Vec<String> = refused.iter().map(ToString::to_string).collect();
        return Err(Error::Unsupported(format!(
            "CZI has no readable scene: {}",
            reasons.join("; ")
        )));
    }

    Ok(CziReader {
        mmap,
        meta: ImageMeta {
            name,
            scenes: scene_metas,
        },
        subblocks: subblocks.clone(),
        scenes,
        refused,
        file_indices,
        stored: stored_pyramids,
    })
}

/// Keeps every full-resolution tile of a known pixel type whose span fits the file; counts the rest per scene.
fn collect_subblocks(
    buf: &[u8],
    raw_entries: &[RawDirEntry],
) -> (Vec<SubBlock>, Vec<Coord>, HashMap<i32, SceneSkips>) {
    let mut subblocks: Vec<SubBlock> = Vec::new();
    let mut coords: Vec<Coord> = Vec::new();
    let mut skips: HashMap<i32, SceneSkips> = HashMap::new();

    for de in raw_entries {
        // The coarse subblocks are the file's own pyramid, not the scene's pixels.
        // A scene of coarse tiles only is not a file scene: it is neither published nor refused.
        if de.is_coarse() {
            continue;
        }
        // An unmodelled PixelType skips its tile, not the file: the scene that holds it is refused below,
        // since publishing it without the tile would make it smaller than the file says.
        let (dtype, samples) = match pixel_type_to_dtype(de.pixel_type) {
            Ok(ds) => ds,
            Err(_) => {
                skips.entry(de.s).or_default().unmodelled_pixel_type += 1;
                continue;
            }
        };
        // A subblock whose pixel span fails bounds is skipped the same way; the file fails only if none remain.
        let (pixel_off, data_size) = match subblock_pixel_span(buf, de.file_position) {
            Ok(v) => v,
            Err(_) => {
                skips.entry(de.s).or_default().bad_span += 1;
                continue;
            }
        };

        let idx = subblocks.len();
        subblocks.push(SubBlock {
            pixel_off,
            data_size,
            compression: de.compression,
            dtype,
            samples,
            m: de.m,
            start_x: de.start_x,
            start_y: de.start_y,
            stored_x: de.stored_x,
            stored_y: de.stored_y,
        });
        coords.push(Coord {
            s: de.s,
            c: de.c,
            z: de.z,
            t: de.t,
            subblock: idx,
        });
    }
    (subblocks, coords, skips)
}

/// The file's own scene list: each full-resolution `S` value with its tile count, in `S` order.
fn full_res_scene_counts(raw_entries: &[RawDirEntry]) -> BTreeMap<i32, usize> {
    // The file's own list of scenes is taken from the raw directory, not from the subblocks read: a scene whose
    // tiles were all skipped keeps its place, or every later scene would be renumbered.
    let mut scene_tiles: BTreeMap<i32, usize> = BTreeMap::new();
    for de in raw_entries {
        if de.stored_x == de.size_x && de.stored_y == de.size_y {
            *scene_tiles.entry(de.s).or_default() += 1;
        }
    }
    scene_tiles
}

/// Groups the coordinates by scene in one pass: `S` is attacker-controlled, and a filter per scene is O(N^2).
fn group_by_scene(coords: &[Coord]) -> HashMap<i32, Vec<&Coord>> {
    let mut by_scene: HashMap<i32, Vec<&Coord>> = HashMap::new();
    for co in coords {
        by_scene.entry(co.s).or_default().push(co);
    }
    by_scene
}

/// What every scene of the file is composed from.
struct FileParts<'a> {
    name: &'a str,
    subblocks: &'a [SubBlock],
    channel_names: &'a [String],
    calibration: &'a Calibration,
}

/// Position and naming of one scene within the file.
struct SceneSlot {
    index: usize,
    s_val: i32,
    single_scene: bool,
    total_tiles: usize,
}

/// One scene read at `open()`: published with its metadata, or refused with its reason.
enum SceneOutcome {
    Published(Box<(SceneMeta, CziScene)>),
    Refused(RefusedScene),
}

/// Composes one scene from its tiles, or refuses it; only an overflowing size or a failed bounds read is an error.
fn compose_scene(
    parts: &FileParts,
    slot: SceneSlot,
    members: &[&Coord],
    skips: SceneSkips,
) -> Result<SceneOutcome> {
    let scene_name = if slot.single_scene {
        parts.name.to_string()
    } else {
        format!("{} scene {}", parts.name, slot.index)
    };

    // A scene that lost full-resolution tiles is refused by name, never published smaller than the file says.
    // Each declared tile is either a member or counted as skipped, so the two add up.
    let skipped = skips.total();
    debug_assert_eq!(members.len() + skipped, slot.total_tiles);
    if skipped > 0 || members.is_empty() {
        return Ok(SceneOutcome::Refused(RefusedScene {
            image_index: slot.index,
            name: scene_name,
            reason: lost_tiles_reason(slot.index, slot.s_val, slot.total_tiles, &skips),
        }));
    }

    let c_values = sorted_distinct(members.iter().map(|co| co.c));
    let z_values = sorted_distinct(members.iter().map(|co| co.z));
    let t_values = sorted_distinct(members.iter().map(|co| co.t));

    let (origin_x, origin_y, width, height) = scene_bbox(members, parts.subblocks)?;

    // Every tile of a scene shares one dtype and samples-per-pixel (Gray mixed with Bgr is not modelled).
    // A heterogeneous scene is refused; open() fails only if no scene remains.
    let dtype = parts.subblocks[members[0].subblock].dtype;
    let samples = parts.subblocks[members[0].subblock].samples;
    if members.iter().any(|co| {
        let sb = &parts.subblocks[co.subblock];
        sb.dtype != dtype || sb.samples != samples
    }) {
        return Ok(SceneOutcome::Refused(RefusedScene {
            image_index: slot.index,
            name: scene_name,
            reason: format!(
                "scene {} of the file (S={}): the tiles of this scene do not share one pixel type",
                slot.index, slot.s_val
            ),
        }));
    }

    let dims = Dims5 {
        t: t_values.len(),
        c: c_values.len(),
        z: z_values.len(),
        y: height,
        x: width,
    };
    // Only usize overflow is rejected here: the 4 GiB cap is not enforced at open(), so a whole-slide scene
    // still opens and region() streams it. plane() re-checks the cap before it materialises a plane.
    dims.x
        .checked_mul(dims.y)
        .and_then(|e| e.checked_mul(samples as usize))
        .and_then(|e| e.checked_mul(dtype.size_bytes()))
        .ok_or(Error::OversizedAllocation {
            requested: usize::MAX,
            cap: crate::guard::MAX_DECODED_PLANE_BYTES,
        })?;

    let tiles = plane_tiles(members, parts.subblocks);
    let channels: Vec<ChannelMeta> = (0..dims.c)
        .map(|c| ChannelMeta {
            name: parts.channel_names.get(c).cloned(),
            color: None,
        })
        .collect();

    let meta = SceneMeta {
        dims,
        dtype,
        samples,
        px_size_um: parts.calibration.px_size_um,
        notes: parts.calibration.notes.clone(),
        channels,
        palette: None,
        name: scene_name,
    };
    let scene = CziScene {
        dims,
        dtype,
        samples,
        c_values,
        z_values,
        t_values,
        origin_x,
        origin_y,
        tiles,
    };
    Ok(SceneOutcome::Published(Box::new((meta, scene))))
}

/// The subblocks of each plane, in the order they are composed. Directory order is NOT mosaic order: in the
/// Axioscan sample 993 of 7350 overlapping pairs pick another winner. Planes are sorted by mosaic index, stably,
/// and only when one of their tiles has a non-negative `M` (czifile.py:1258-1259, 1308).
fn plane_tiles(members: &[&Coord], subblocks: &[SubBlock]) -> HashMap<(i32, i32, i32), Vec<usize>> {
    let mut tiles: HashMap<(i32, i32, i32), Vec<usize>> = HashMap::new();
    for co in members {
        tiles.entry((co.c, co.z, co.t)).or_default().push(co.subblock);
    }
    for plane_tiles in tiles.values_mut() {
        // czifile tests "any M non-negative" over the whole file, this per plane: they differ only where one plane
        // has a non-negative M and another plane's M values are all negative and distinct: czifile sorts that plane.
        if !plane_tiles.iter().any(|&sb_idx| subblocks[sb_idx].m >= 0) {
            continue;
        }
        plane_tiles.sort_by_key(|&sb_idx| subblocks[sb_idx].m);
    }
    tiles
}
