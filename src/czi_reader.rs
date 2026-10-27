// SPDX-License-Identifier: Apache-2.0
//! Zeiss CZI / ZISRAW reader.
//!
//! The CZI container and its segment/subblock byte layout are ported from the
//! behaviour of the reference reader `czifile` (Christoph Gohlke, BSD-3-Clause,
//! <https://github.com/cgohlke/czifile>) and the openly-documented ZISRAW
//! specification. Only the container *facts* and offset arithmetic are
//! reproduced; no source was copied. Keep this attribution if you touch the
//! framing or the subblock offset math.
//!
//! # Container shape
//!
//! A `.czi` is a flat sequence of **segments**. Each segment is a 32-byte header
//! (`SID[16]` ASCII null-padded, `AllocatedSize` i64 LE, `UsedSize` i64 LE)
//! followed by `AllocatedSize` data bytes. Every integer is little-endian.
//!
//! We do not scan the whole file: the first `ZISRAWFILE` segment
//! (`FileHeaderSegment`) gives the absolute offsets of the `ZISRAWDIRECTORY`
//! (a table of every pixel subblock) and the `ZISRAWMETADATA` (XML). We seek to
//! those two, build the plane model from the directory, and read pixels lazily.
//!
//! **Size**: this file is longer than the 400-line target of the coder
//! standard. The excess is pre-existing and DECLARED here, not refactored by
//! the change that touched it.
//!
//! # Subblock pixel-data offset (byte-exact)
//!
//! A subblock's data (at `DirectoryEntryDV.FilePosition`, i.e. the
//! `ZISRAWSUBBLOCK` segment) begins with a fixed 16-byte header
//! (`MetadataSize` i32, `AttachmentSize` i32, `DataSize` i64), then the
//! subblock's own `DirectoryEntryDV` (`32 + 20*DimensionCount` bytes), then a
//! fill so the whole fixed part is `max(256, 16 + storage_size)` bytes, then
//! `MetadataSize` XML bytes, then `DataSize` pixel bytes, then the attachment.
//! So, relative to the segment's data start `sd = FilePosition + 32`:
//!
//! ```text
//! storage_size    = 32 + 20 * DimensionCount
//! metadata_offset = max(256, 16 + storage_size)
//! pixel_offset    = sd + metadata_offset + MetadataSize
//! pixel_bytes     = DataSize
//! ```
//!
//! # Canonical axis order
//!
//! Dims are built in the crate-canonical **TCZYX** order (`Dims5{t,c,z,y,x}`),
//! matching `types.rs` and every other reader. C/Z/T come from the enumerated
//! subblock dimensions; Y/X are the composed scene-plane size.
//!
//! # Scenes and mosaics
//!
//! The `S` dimension enumerates scenes (default: one scene 0 when `S` is
//! absent). Within a scene, the `M` (mosaic) dimension carries multiple tiles
//! per `(C,Z,T)`; the scene plane is the **bounding box** of its tiles, each
//! blitted at `(X.Start - origin_x, Y.Start - origin_y)`. A non-mosaic scene is
//! the degenerate one-tile case of the same code path.
//!
//! A plane is drawn in **ascending mosaic index**, so the tile with the higher
//! `M` ends up on top — but only when the plane is sorted at all: it is
//! sorted only when at least one of its own tiles carries a non-negative `M`,
//! and a plane whose `M` values are all negative keeps its subblock-directory
//! order instead.
//!
//! The ascending-mosaic-index order is the order `czifile` composes a mosaic
//! in (`czifile.py:1258-1259`: *"Entries with a mosaic index are sorted by
//! mosaic index so that tiles with higher M-index are composited on top"*;
//! the sort at `czifile.py:1308-1309`, applied at `czifile.py:2810-2811`).
//! libCZI is **not verified** — no libCZI source is available on this machine,
//! so the only claim made here is the one about `czifile`.
//!
//! The rule, precisely:
//!
//! * A subblock that carries **no** `M` dimension has mosaic index `-1`
//!   (`czifile.py:4598`), and a subblock that carries **several** `M`
//!   dimension entries takes the **first** one in file order (`czifile.py:4609`
//!   walks the dimension entries in reverse, so the first `M` in the file is
//!   the last one assigned).
//! * A plane is sorted by mosaic index only when **at least one of its own
//!   tiles carries a non-negative `M`** (`czifile.py:1308`); otherwise the
//!   plane keeps its subblock-directory order. A file whose `M` values are all
//!   negative is therefore composed in directory order.
//! * The sort is **stable**: tiles that share a mosaic index keep their
//!   subblock-directory order.
//! * A **sorted** plane draws its tiles in ascending mosaic index, and a tile
//!   that carries **no** `M` has index `-1`: it is drawn after every tile with
//!   a **negative** `M` and before every tile with a non-negative one. So the
//!   tile without an `M` is not necessarily below the tiles that carry one —
//!   it is below the ones with a non-negative `M` and above the ones with a
//!   negative `M`. In the sub-case where every `M` of the plane is **negative**
//!   the plane is not sorted at all, so no tile is drawn first: the directory
//!   order stands, and a tile without an `M` (index `-1`) is not necessarily
//!   below the others either.
//! * A **packed** `M` (`M.Size > 1`) is accepted, not refused: the `Start` is
//!   the mosaic index and the `Size` is ignored, like `czifile` does
//!   (`czifile.py:4616-4617` — no size check at all). The `M` axis carries no
//!   pixels, so a packed `M` cannot change the geometry of the composed
//!   plane. The one gap is the JPEG-XR path: a JPEG-XR subblock that really
//!   holds several packed frames would be decoded as one frame, because that
//!   path has no frame-count check (the uncompressed and zstd paths refuse a
//!   mismatched length). No such sample exists, so this is declared, not
//!   tested.
//!
//! `czifile` evaluates "is any `M` non-negative" over the WHOLE file's
//! filtered subblock directory (`czifile.py:1308`), while this reader
//! evaluates it per plane. The two agree for every file whose planes are
//! either all-mosaic or all-non-mosaic; they differ only for a file where one
//! plane carries a non-negative `M` and another plane's `M` values are all
//! negative *and distinct* — there `czifile` sorts that second plane by its
//! negative indices, and this reader keeps its directory order.
//!
//! `czifile`'s threaded path is **non-deterministic** for overlapping tiles:
//! *"the final value of any pixel covered by more than one tile is
//! non-deterministic under multi-threading"* (`czifile.py:2793-2799`); only
//! `maxworkers=1` forces *"sequential, fully deterministic compositing"*
//! (`czifile.py:2800-2801`, the path at `czifile.py:2810-2811`). Every
//! reference in this repository is therefore taken with `maxworkers=1`.
//!
//! The file's list of scenes is the sorted distinct `S` values of the
//! **full-resolution** subblocks. A scene that holds only sub-sampled
//! (pyramid) tiles is therefore not a scene of the file this reader composes:
//! it is neither published nor refused. A scene that LOSES full-resolution
//! tiles (an unmodelled pixel type, a tile whose pixel span is out of bounds)
//! is refused by name, with the count of the tiles that were skipped and why
//! they were skipped — it is never published smaller than the file says.
//! When EVERY scene of the file is refused there is nothing left to open, so
//! the open fails — and the error then names each refusal with its own reason
//! instead of guessing a cause.
//!
//! # Compression
//!
//! * `0` Uncompressed  — raw little-endian samples (the acceptance case).
//! * `5` Zstd0         — plain zstd stream (pure-Rust `ruzstd`).
//! * `6` Zstd1         — small header + optional Hi/Lo byte unpack, then zstd.
//! * `1` JpgFile / `2` LZW — `Error::Unsupported`.
//! * `4` JpegXR — `Error::Unsupported` unless the `jpegxr` feature is on.
//!
//! The Zstd1 header/Hi-Lo handling is implemented to the documented czifile
//! layout (libCZI not verified) and is **validated against imagecodecs `zstd1`
//! fixtures** (the
//! same codec czifile uses to decode real Zeiss zstd subblocks — see
//! `tests/fixtures/zstd/` and the `zstd_fixtures_match_ground_truth` test); see
//! `zstd1_decode` for the exact bytes.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::path::Path;

use memmap2::Mmap;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use ruzstd::decoding::StreamingDecoder;

use crate::calibration::Calibration;
use crate::error::{Error, Result};
use crate::io::map_file;
use crate::types::{
    ChannelMeta, DType, Dims5, Endian, ImageMeta, ImageReader, PixelRect, Plane, PlaneBuf,
    RefusedScene, SceneMeta,
};

/// Length of a ZISRAW segment header (`SID[16]` + `AllocatedSize` + `UsedSize`).
const SEGMENT_HEADER_LEN: usize = 32;

/// Fixed lead-in of a `SubBlockSegment` before its `DirectoryEntryDV`
/// (`MetadataSize` i32 + `AttachmentSize` i32 + `DataSize` i64).
const SUBBLOCK_FIXED_HEADER: usize = 16;

/// Minimum size of a subblock's fixed part (header + directory entry + fill).
/// The ZISRAW spec pads the fixed part to `max(256, 16 + storage_size)`.
const SUBBLOCK_MIN_FIXED: usize = 256;

/// Fixed part of a `DirectoryEntryDV` before its dimension entries.
const DIR_ENTRY_FIXED: usize = 32;

/// Size of one `DimensionEntryDV1` (Dimension char[4], Start i32, Size i32,
/// StartCoordinate f32, StoredSize i32).
const DIM_ENTRY_LEN: usize = 20;

/// Fixed lead-in of the `MetadataSegment` before its XML (`XmlSize` i32 +
/// `AttachmentSize` i32 + `Spare[248]`).
const METADATA_FIXED: usize = 256;

/// Upper bound on subblock directory entries (guards a corrupt `EntryCount`).
/// Real files have hundreds to a few thousand (an Axioscan whole-slide has
/// 4885); a still-generous 2M cap keeps a small crafted file from allocating
/// multi-GB of bookkeeping `Vec`s. The per-entry byte-floor check in
/// `parse_directory` and the mmap bounds-checks also terminate the walk.
const MAX_SUBBLOCKS: usize = 2_000_000;

/// Upper bound on dimension entries in one subblock (real CZIs have < 16).
const MAX_DIMS_PER_ENTRY: usize = 64;

/// Upper bound on the metadata XML we will decode (best-effort metadata only).
const MAX_XML_BYTES: usize = 256 * 1024 * 1024;

/// One decodable pixel subblock (a full-resolution tile).
struct SubBlock {
    /// Absolute file offset of the pixel bytes.
    pixel_off: usize,
    /// Length of the pixel bytes (`DataSize`; compressed length when compressed).
    data_size: usize,
    /// CZI compression id (0 = uncompressed, 5/6 = zstd, …).
    compression: i32,
    /// Per-sample type of this tile.
    dtype: DType,
    /// Samples per pixel (1 = grayscale, 3 = RGB/color).
    samples: u32,
    /// Mosaic index: the `M` dimension's `Start`. `-1` when the tile carries no
    /// `M` dimension (czifile's own convention, `czifile.py:4598`).
    m: i32,
    /// Tile X position within the scene pixel grid (`X.Start`, may be negative).
    start_x: i32,
    /// Tile Y position within the scene pixel grid (`Y.Start`, may be negative).
    start_y: i32,
    /// Stored tile width in pixels (`X.StoredSize` == `X.Size` for full-res).
    stored_x: usize,
    /// Stored tile height in pixels (`Y.StoredSize` == `Y.Size` for full-res).
    stored_y: usize,
}

/// A composed scene: its dims, dtype, and the tile lookup for `plane()`.
struct CziScene {
    dims: Dims5,
    dtype: DType,
    /// Samples per pixel shared by every tile in this scene (1 = grayscale).
    samples: u32,
    /// Sorted distinct `C`/`Z`/`T` coordinate values; canonical index `i` maps
    /// to the value at position `i` (robust to non-zero / non-contiguous bases).
    c_values: Vec<i32>,
    z_values: Vec<i32>,
    t_values: Vec<i32>,
    /// Top-left origin of the scene bounding box (`min X.Start`, `min Y.Start`).
    origin_x: i32,
    origin_y: i32,
    /// `(c_val, z_val, t_val)` -> indices into [`CziReader::subblocks`] (tiles).
    tiles: HashMap<(i32, i32, i32), Vec<usize>>,
}

/// The full-resolution tiles of ONE scene that the reader skipped, and why.
#[derive(Debug, Clone, Copy, Default)]
struct SceneSkips {
    /// Tiles whose `PixelType` the reader does not model.
    unmodelled_pixel_type: usize,
    /// Tiles whose pixel span does not fit the file.
    bad_span: usize,
}

impl SceneSkips {
    /// How many tiles of the scene were skipped in total.
    fn total(&self) -> usize {
        self.unmodelled_pixel_type + self.bad_span
    }
}

/// Zeiss CZI reader. `Mmap` is `Send + Sync` and every other field is plain
/// data, so the reader is `Send + Sync` and shareable across threads.
pub struct CziReader {
    mmap: Mmap,
    meta: ImageMeta,
    /// Every decodable subblock, referenced by index from each scene's `tiles`.
    subblocks: Vec<SubBlock>,
    /// Parallel to `meta.scenes`: `scenes[s]` describes `meta.scenes[s]`.
    scenes: Vec<CziScene>,
    /// The scenes of the file this reader refused, in file order, each with its
    /// reason. The readable scenes are in `meta`, in file order.
    refused: Vec<RefusedScene>,
    /// Parallel to `meta.scenes`: the RANK of each readable scene among the
    /// sorted distinct `S` values of the file's FULL-RESOLUTION subblocks. It
    /// differs from the scene's own index when an earlier scene of the file
    /// was refused. A scene that holds only sub-sampled (pyramid) tiles is not
    /// one of the file's scenes and holds no rank.
    file_indices: Vec<usize>,
}

impl ImageReader for CziReader {
    fn open(path: &Path) -> Result<Self> {
        let mmap = map_file(path)?;
        let name = file_stem(path);
        let buf = &mmap[..];

        // --- FileHeader: locate the directory and metadata segments ---
        let (dir_pos, meta_pos) = parse_file_header(buf)?;

        // --- best-effort metadata XML (never gates decoding) ---
        let (channel_names, calibration) = parse_metadata(buf, meta_pos).unwrap_or_default();

        // --- subblock directory: enumerate every full-res tile ---
        let raw_entries = parse_directory(buf, dir_pos)?;
        let mut subblocks: Vec<SubBlock> = Vec::new();
        // Per subblock (index-aligned with `subblocks`): its scene/coordinate key.
        let mut coords: Vec<Coord> = Vec::new();
        // Per scene (`S`): the full-resolution tiles that were skipped, and why.
        let mut skips: HashMap<i32, SceneSkips> = HashMap::new();

        for de in &raw_entries {
            // Skip pyramid / sub-sampled tiles (StoredSize != Size): only the
            // full-resolution level is composed. For the reference files this
            // drops nothing (every tile is full-res). A scene that holds ONLY
            // sub-sampled tiles is not a scene of the file we compose: the
            // scene list below is built from the full-resolution subblocks, so
            // such a scene is neither published nor refused.
            if de.stored_x != de.size_x || de.stored_y != de.size_y {
                continue;
            }
            // Skip (don't abort the file on) a subblock whose PixelType we do
            // not model — e.g. a label/overview tile or a Bgr96Float tile mixed
            // into the main directory alongside hundreds of valid planes. The
            // scene that holds it is REFUSED below (publishing it would give a
            // scene smaller than the file says), while the other scenes still
            // open. (Bgr24/Bgr48 ARE modelled now, so they are kept.)
            let (dtype, samples) = match pixel_type_to_dtype(de.pixel_type) {
                Ok(ds) => ds,
                Err(_) => {
                    skips.entry(de.s).or_default().unmodelled_pixel_type += 1;
                    continue;
                }
            };
            // Skip (don't abort the file on) a single subblock whose pixel span
            // fails bounds — same policy as the pixel-type skip above: the
            // scene that holds it is refused by name. The `subblocks.is_empty()`
            // check below still fails the file when NO usable subblock remains.
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

        // --- group into scenes by the S coordinate (default: single scene 0) ---
        // Group in ONE pass: `co.s` is attacker-controlled, so the old
        // `coords.iter().filter(|co| co.s == s_val)` per scene was O(scenes x
        // subblocks) == O(N^2) and a file with N distinct S values pinned a CPU
        // for minutes inside `open()`. Build the scene -> members map once
        // (O(N) amortized), then look each scene up in O(1).
        let mut by_scene: HashMap<i32, Vec<&Coord>> = HashMap::new();
        for co in &coords {
            by_scene.entry(co.s).or_default().push(co);
        }
        // The FILE's own list of scenes: the sorted distinct `S` values of the
        // FULL-RESOLUTION subblocks, and how many tiles each of them declares.
        // It is taken from the raw directory, not from the subblocks we could
        // read, because a scene whose every tile was skipped (an unmodelled
        // pixel type, a tile whose pixel span is out of bounds) still holds its
        // place in the file: dropping it from this list would renumber every
        // scene after it. A scene that holds only sub-sampled (pyramid) tiles
        // is NOT in this list: it is not a scene of the file we compose, so it
        // is neither published nor refused.
        let mut scene_tiles: BTreeMap<i32, usize> = BTreeMap::new();
        for de in &raw_entries {
            if de.stored_x == de.size_x && de.stored_y == de.size_y {
                *scene_tiles.entry(de.s).or_default() += 1;
            }
        }
        if scene_tiles.is_empty() {
            return Err(Error::Unsupported(
                "CZI has no full-resolution pixel subblocks".to_string(),
            ));
        }
        let scene_s_values: Vec<i32> = scene_tiles.keys().copied().collect();

        let mut scenes: Vec<CziScene> = Vec::new();
        let mut scene_metas: Vec<SceneMeta> = Vec::new();
        let mut refused: Vec<RefusedScene> = Vec::new();
        let mut file_indices: Vec<usize> = Vec::new();

        for (scene_idx, &s_val) in scene_s_values.iter().enumerate() {
            let members: &[&Coord] = by_scene
                .get(&s_val)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let scene_skips = skips.get(&s_val).copied().unwrap_or_default();
            let total_tiles = scene_tiles[&s_val];

            let scene_name = if scene_s_values.len() == 1 {
                name.clone()
            } else {
                format!("{name} scene {scene_idx}")
            };

            // A scene that LOST full-resolution tiles is not published smaller
            // than the file says: it is refused by name, with the count of the
            // tiles that were skipped and why they were skipped. A scene with
            // no readable tile at all is refused the same way. The scenes
            // after it keep their own file positions.
            //
            // Every full-resolution tile of the scene is either readable (a
            // member) or skipped (counted), so the two counts add up to the
            // number of tiles the file declares for the scene.
            let skipped = scene_skips.total();
            debug_assert_eq!(members.len() + skipped, total_tiles);
            if skipped > 0 || members.is_empty() {
                refused.push(RefusedScene {
                    image_index: scene_idx,
                    name: scene_name,
                    reason: lost_tiles_reason(scene_idx, s_val, total_tiles, &scene_skips),
                });
                continue;
            }

            let c_values = sorted_distinct(members.iter().map(|co| co.c));
            let z_values = sorted_distinct(members.iter().map(|co| co.z));
            let t_values = sorted_distinct(members.iter().map(|co| co.t));

            // Bounding box of this scene's tiles (mosaic composition extent).
            let (origin_x, origin_y, width, height) = scene_bbox(members, &subblocks)?;

            // Every tile in a scene must share one pixel type: the same
            // per-sample dtype AND samples-per-pixel (heterogeneous mosaics,
            // e.g. Gray mixed with Bgr, are not modelled). Policy is
            // skip-not-abort: a heterogeneous scene is dropped, but OTHER valid
            // scenes still open. `open()` fails only if this leaves ZERO scenes
            // (checked after the loop).
            let dtype = subblocks[members[0].subblock].dtype;
            let samples = subblocks[members[0].subblock].samples;
            if members
                .iter()
                .any(|co| {
                    let sb = &subblocks[co.subblock];
                    sb.dtype != dtype || sb.samples != samples
                })
            {
                refused.push(RefusedScene {
                    image_index: scene_idx,
                    name: scene_name,
                    reason: format!(
                        "scene {scene_idx} of the file (S={s_val}): the tiles of this scene \
                         do not share one pixel type"
                    ),
                });
                continue;
            }

            let dims = Dims5 {
                t: t_values.len(),
                c: c_values.len(),
                z: z_values.len(),
                y: height,
                x: width,
            };
            // Guard the composed-plane size against usize OVERFLOW only — a
            // corrupt dims whose `x * y * samples * itemsize` wraps `usize` is
            // rejected here, before any bookkeeping is built. The 4 GiB CAP is
            // deliberately NOT enforced at open(): open() allocates nothing
            // plane-sized — only the tile `HashMap` + per-scene metadata — so a
            // whole-slide Axioscan scene (whose fully-composed plane is tens of
            // GB) MUST still OPEN, letting `region()` stream bounded windows
            // out-of-core. The cap still protects the one place that actually
            // materializes the whole plane: `plane()` re-checks
            // `checked_plane_bytes_samples` and still refuses an oversize scene
            // with `OversizedAllocation`.
            dims.x
                .checked_mul(dims.y)
                .and_then(|e| e.checked_mul(samples as usize))
                .and_then(|e| e.checked_mul(dtype.size_bytes()))
                .ok_or(Error::OversizedAllocation {
                    requested: usize::MAX,
                    cap: crate::guard::MAX_DECODED_PLANE_BYTES,
                })?;

            // Tile lookup keyed by the actual (c,z,t) coordinate values.
            let mut tiles: HashMap<(i32, i32, i32), Vec<usize>> = HashMap::new();
            for co in members {
                tiles.entry((co.c, co.z, co.t)).or_default().push(co.subblock);
            }

            // Draw the tiles of a plane in ASCENDING MOSAIC INDEX, so the tile
            // with the higher M ends up on top. This is the order `czifile`
            // composes a mosaic in ("Entries with a mosaic index are sorted by
            // mosaic index so that tiles with higher M-index are composited on
            // top", `czifile.py:1258-1259`; the sort at `czifile.py:1308-1309`,
            // applied at `czifile.py:2810-2811`). libCZI is NOT verified here:
            // no libCZI source is available on this machine, so the claim this
            // reader makes is about `czifile` only.
            //
            // WHY: the subblock directory is NOT in mosaic order — in the
            // Axioscan sample 993 of the 7350 overlapping pairs pick a
            // different winner under the two orders, so the directory order
            // would draw a different tile on top wherever two tiles overlap.
            //
            // The sort is done ONCE, here, where the per-plane tile lists are
            // built — not per `region()` call. It is stable, so tiles that
            // share a mosaic index keep their subblock-directory order, and
            // the tiles that carry no M dimension at all (index -1) keep
            // theirs too: they are drawn after the tiles with a negative M and
            // before the ones with a non-negative M.
            //
            // A plane is sorted only when at least one of THAT plane's tiles
            // carries a non-negative M. `czifile` evaluates that condition over
            // the WHOLE file's filtered subblock directory
            // (`czifile.py:1308`); this reader evaluates it per PLANE, which
            // is a documented divergence (see the module doc). A plane whose
            // tiles all carry a negative M keeps its subblock-directory
            // order, which is what `czifile` does for a file that carries no
            // mosaic at all.
            for plane_tiles in tiles.values_mut() {
                if !plane_tiles.iter().any(|&sb_idx| subblocks[sb_idx].m >= 0) {
                    continue;
                }
                plane_tiles.sort_by_key(|&sb_idx| subblocks[sb_idx].m);
            }

            // Channel metadata: names from the XML in document order, best-effort.
            let channels: Vec<ChannelMeta> = (0..dims.c)
                .map(|c| ChannelMeta {
                    name: channel_names.get(c).cloned(),
                    color: None,
                })
                .collect();

            file_indices.push(scene_idx);
            scene_metas.push(SceneMeta {
                dims,
                dtype,
                samples,
                px_size_um: calibration.px_size_um,
                notes: calibration.notes.clone(),
                channels,
                palette: None,
                name: scene_name,
            });
            scenes.push(CziScene {
                dims,
                dtype,
                samples,
                c_values,
                z_values,
                t_values,
                origin_x,
                origin_y,
                tiles,
            });
        }

        // Fail closed only when EVERY scene was dropped (all heterogeneous /
        // unsupported); a file with at least one pure, supported scene opens.
        // The error names EVERY refusal with its own reason, so a scene that
        // lost a tile is not misdiagnosed as a pixel-type clash.
        if scenes.is_empty() {
            let reasons: Vec<String> = refused.iter().map(ToString::to_string).collect();
            return Err(Error::Unsupported(format!(
                "CZI has no readable scene: {}",
                reasons.join("; ")
            )));
        }

        let meta = ImageMeta {
            name,
            scenes: scene_metas,
        };

        Ok(CziReader {
            mmap,
            meta,
            subblocks,
            scenes,
            refused,
            file_indices,
        })
    }

    fn meta(&self) -> &ImageMeta {
        &self.meta
    }

    /// The scenes of the file this reader refused, each with its reason.
    fn refused_scenes(&self) -> &[RefusedScene] {
        &self.refused
    }

    /// The rank of `scene` among the sorted distinct `S` values of the file's
    /// FULL-RESOLUTION subblocks. It differs from `scene` when an earlier
    /// scene of the file was refused. It is a RANK, not the `S` value itself:
    /// a file whose scenes are numbered from 1 (or with gaps) still numbers
    /// its ranks from 0.
    fn scene_file_index(&self, scene: usize) -> usize {
        self.file_indices.get(scene).copied().unwrap_or(scene)
    }

    fn plane(&self, scene: usize, t: usize, c: usize, z: usize) -> Result<Plane> {
        let sc = self.scene_checked(scene, t, c, z)?;
        let d = sc.dims;

        // Size the composed OUTPUT plane BEFORE reserving/reading anything.
        // `pixel_bytes` is the per-pixel byte size: per-sample size x samples
        // per pixel (color is carried as interleaved samples, so the blit works
        // on whole pixels regardless of grayscale vs RGB).
        let pixel_bytes = sc
            .dtype
            .size_bytes()
            .checked_mul(sc.samples as usize)
            .ok_or_else(|| Error::Decode("CZI per-pixel byte size overflow".to_string()))?;
        let plane_bytes = d.checked_plane_bytes_samples(sc.dtype, sc.samples)?;

        // Canonical index -> actual CZI coordinate value.
        let key = (sc.c_values[c], sc.z_values[z], sc.t_values[t]);
        let tiles = sc.tiles.get(&key).ok_or(Error::OutOfRange { scene, t, c, z })?;

        // Compose the scene plane from its tiles at the little-endian byte level,
        // then convert once to native endianness. Uncovered pixels stay zero.
        let mut composed = vec![0u8; plane_bytes];
        // Samples-aware row stride: `width * samples * itemsize` bytes per row.
        let row_stride = d
            .x
            .checked_mul(pixel_bytes)
            .ok_or_else(|| Error::Decode("CZI composed row stride overflow".to_string()))?;

        for &sb_idx in tiles {
            let sb = &self.subblocks[sb_idx];
            let tile = self.decode_tile(sb)?;
            blit_tile(
                &mut composed,
                d.x,
                d.y,
                row_stride,
                // Treat a whole (possibly multi-sample) pixel as the blit unit,
                // so the per-row stride and column offsets are samples-aware.
                pixel_bytes,
                &tile,
                sb.stored_x,
                sb.stored_y,
                // Compute placement offsets in i64: `X.Start`/`Y.Start` are
                // attacker-controlled i32 and `origin` is their min, so the
                // difference is always >= 0 but can exceed i32::MAX. Doing the
                // subtraction on plain i32 would overflow-panic on a crafted
                // mosaic (e.g. tiles at i32::MIN and 0); i64 cannot.
                i64::from(sb.start_x) - i64::from(sc.origin_x),
                i64::from(sb.start_y) - i64::from(sc.origin_y),
            )?;
        }

        // `composed` is little-endian, interleaved (R,G,B for color) sample
        // bytes; `from_bytes` byteswaps per-sample to native as needed (CZI is
        // little-endian, so on a LE host it is a copy). For color, the buffer is
        // already RGB — `decode_tile` swapped B<->R per pixel.
        let data = PlaneBuf::from_bytes(sc.dtype, Endian::Little, &composed)?;
        Plane::new_samples(data, [d.y, d.x], sc.samples)
    }

    /// CZI implements a genuine out-of-core [`region`](Self::region): it decodes
    /// only the tiles intersecting `rect`, never the whole plane. A caller doing
    /// many sub-region reads of one plane must keep streaming per tile here (a
    /// whole-slide plane may exceed the decode cap and must never be
    /// materialized), so this returns `true`.
    fn supports_region(&self) -> bool {
        true
    }

    /// Out-of-core region read: decode ONLY the sub-window `rect` of the plane
    /// at `(scene, t, c, z)`, never composing the whole plane.
    ///
    /// The result is **byte-identical** to `self.plane(scene, t, c, z)?.crop(rect)`
    /// but the output buffer is sized to `rect` (not the scene plane) and only
    /// the tiles that actually intersect `rect` are decoded — so a 512-px window
    /// over a whole-slide mosaic touches ~1–4 tiles instead of thousands, and a
    /// scene whose full plane exceeds [`crate::guard::MAX_DECODED_PLANE_BYTES`]
    /// (refused by `plane()`) can still be read one window at a time. This is
    /// what lets a consumer that builds pyramids stream the base level of a
    /// multi-gigabyte whole-slide scene.
    fn region(
        &self,
        scene: usize,
        t: usize,
        c: usize,
        z: usize,
        rect: PixelRect,
    ) -> Result<Plane> {
        // Validate scene/t/c/z exactly like `plane()` (shared helper).
        let sc = self.scene_checked(scene, t, c, z)?;
        let d = sc.dims;

        // Validate `rect` against the scene dims with CHECKED adds: an
        // overflowing `x + w` / `y + h`, or a rect past `dims.x`/`dims.y`, is a
        // clean `Decode` error (mirrors `Plane::crop` / the provided `region`
        // default), never a wrap or panic.
        let rect_x1 = rect.x.checked_add(rect.w).ok_or_else(|| {
            Error::Decode(format!(
                "CZI region rect x+w overflows usize (x={}, w={})",
                rect.x, rect.w
            ))
        })?;
        let rect_y1 = rect.y.checked_add(rect.h).ok_or_else(|| {
            Error::Decode(format!(
                "CZI region rect y+h overflows usize (y={}, h={})",
                rect.y, rect.h
            ))
        })?;
        if rect_x1 > d.x || rect_y1 > d.y {
            return Err(Error::Decode(format!(
                "CZI region rect {{x:{}, y:{}, w:{}, h:{}}} out of range for scene {} dims (x:{}, y:{})",
                rect.x, rect.y, rect.w, rect.h, scene, d.x, d.y
            )));
        }

        // Per-pixel byte size: per-sample size x samples per pixel (color is
        // carried as interleaved samples, so the blit works on whole pixels).
        let pixel_bytes = sc
            .dtype
            .size_bytes()
            .checked_mul(sc.samples as usize)
            .ok_or_else(|| Error::Decode("CZI per-pixel byte size overflow".to_string()))?;

        // Canonical index -> actual CZI coordinate value. Look up the tiles for
        // this coordinate BEFORE sizing/allocating the output, so a sparse
        // (c,z,t) with no subblock fails cheaply instead of first reserving an
        // up-to-cap buffer (matches `plane()`'s validate-before-allocate order).
        let key = (sc.c_values[c], sc.z_values[z], sc.t_values[t]);
        let tiles = sc.tiles.get(&key).ok_or(Error::OutOfRange { scene, t, c, z })?;

        // Size the OUTPUT to the RECT (never the plane) and enforce the cap on
        // THIS: a pathological huge rect is refused with `OversizedAllocation`
        // even though the enclosing (whole-slide) scene was allowed to open.
        let rect_dims = Dims5 { t: 1, c: 1, z: 1, y: rect.h, x: rect.w };
        let region_bytes = rect_dims.checked_plane_bytes_samples(sc.dtype, sc.samples)?;
        // Destination row stride is the RECT width in bytes.
        let dst_row_stride = rect
            .w
            .checked_mul(pixel_bytes)
            .ok_or_else(|| Error::Decode("CZI region row stride overflow".to_string()))?;

        // Compose the requested window at the little-endian byte level, then
        // convert once to native endianness (identical to `plane()`). Uncovered
        // pixels stay zero.
        let mut composed = vec![0u8; region_bytes];

        for &sb_idx in tiles {
            let sb = &self.subblocks[sb_idx];

            // Tile placement on the scene grid. `X.Start`/`Y.Start` are
            // attacker-controlled i32 and `origin` is their min, so the
            // difference is >= 0 by construction but can exceed i32::MAX — do it
            // in i64 (like `plane()`'s blit) so a crafted mosaic cannot overflow.
            let tile_x0_i = i64::from(sb.start_x) - i64::from(sc.origin_x);
            let tile_y0_i = i64::from(sb.start_y) - i64::from(sc.origin_y);
            if tile_x0_i < 0 || tile_y0_i < 0 {
                return Err(Error::Decode(
                    "CZI tile placed before scene origin".to_string(),
                ));
            }
            let tile_x0 = usize::try_from(tile_x0_i)
                .map_err(|_| Error::Decode("CZI tile column offset out of range".to_string()))?;
            let tile_y0 = usize::try_from(tile_y0_i)
                .map_err(|_| Error::Decode("CZI tile row offset out of range".to_string()))?;
            // Tile extent `[tile_x0, tile_x1) x [tile_y0, tile_y1)` (checked).
            let tile_x1 = tile_x0
                .checked_add(sb.stored_x)
                .ok_or_else(|| Error::Decode("CZI tile x-extent overflow".to_string()))?;
            let tile_y1 = tile_y0
                .checked_add(sb.stored_y)
                .ok_or_else(|| Error::Decode("CZI tile y-extent overflow".to_string()))?;

            // Intersect the tile extent with the requested rect.
            let inter_x0 = tile_x0.max(rect.x);
            let inter_y0 = tile_y0.max(rect.y);
            let inter_x1 = tile_x1.min(rect_x1);
            let inter_y1 = tile_y1.min(rect_y1);
            // Empty intersection -> skip WITHOUT decoding (the out-of-core win).
            // Also covers a zero-size rect (rect_x1 == rect.x => inter_x0 >= inter_x1).
            if inter_x0 >= inter_x1 || inter_y0 >= inter_y1 {
                continue;
            }
            let inter_w = inter_x1 - inter_x0;
            let inter_h = inter_y1 - inter_y0;

            // A compressed codec cannot decode a sub-rectangle, so the FULL tile
            // is decoded — but only for the (few) tiles that actually intersect.
            let tile = self.decode_tile(sb)?;
            blit_tile_clipped(
                &mut composed,
                dst_row_stride,
                pixel_bytes,
                &tile,
                sb.stored_x,
                sb.stored_y,
                // Source offset within the decoded tile.
                inter_y0 - tile_y0,
                inter_x0 - tile_x0,
                // Destination offset within `composed` (rect-local).
                inter_y0 - rect.y,
                inter_x0 - rect.x,
                inter_w,
                inter_h,
            )?;
        }

        // `composed` is little-endian interleaved sample bytes (RGB for color,
        // already swapped by `decode_tile`); convert once to native endianness.
        let data = PlaneBuf::from_bytes(sc.dtype, Endian::Little, &composed)?;
        Plane::new_samples(data, [rect.h, rect.w], sc.samples)
    }
}

impl CziReader {
    /// Validate a `(scene, t, c, z)` coordinate and return its scene. Shared by
    /// `plane()` and `region()` so the two entry points can never drift on the
    /// bounds check (a fix to one but not the other would be a silent bug).
    fn scene_checked(&self, scene: usize, t: usize, c: usize, z: usize) -> Result<&CziScene> {
        let sc = self.scenes.get(scene).ok_or(Error::SceneOutOfRange {
            scene,
            count: self.scenes.len(),
        })?;
        let d = sc.dims;
        if t >= d.t || c >= d.c || z >= d.z {
            return Err(Error::OutOfRange { scene, t, c, z });
        }
        Ok(sc)
    }

    /// Decode one subblock's tile into little-endian sample bytes
    /// (`stored_x * stored_y * samples * itemsize` bytes). For a color tile the
    /// samples are interleaved and already swapped from BGR to RGB order.
    fn decode_tile(&self, sb: &SubBlock) -> Result<Vec<u8>> {
        // Guard the decoded tile size BEFORE any allocation (reuses the plane
        // cap; samples-aware so a color tile is sized at `samples`x grayscale).
        let tile_dims = Dims5 {
            t: 1,
            c: 1,
            z: 1,
            y: sb.stored_y,
            x: sb.stored_x,
        };
        let expected = tile_dims.checked_plane_bytes_samples(sb.dtype, sb.samples)?;

        // Pixel bytes were bounds-checked at open(); re-check defensively.
        let end = sb
            .pixel_off
            .checked_add(sb.data_size)
            .filter(|&e| e <= self.mmap.len())
            .ok_or_else(|| Error::Decode("CZI subblock pixel span out of bounds".to_string()))?;
        let raw = &self.mmap[sb.pixel_off..end];

        // Little-endian sample bytes for this tile. Color tiles are still in the
        // container's native BGR order at this point; the swap to RGB happens
        // once below, after decompression, so it is correct for every codec path.
        let le_bytes = match sb.compression {
            // Uncompressed: raw is already the little-endian sample bytes.
            0 => {
                if raw.len() != expected {
                    return Err(Error::Decode(format!(
                        "CZI uncompressed subblock has {} bytes, expected {expected}",
                        raw.len()
                    )));
                }
                raw.to_vec()
            }
            // Zstd0: a plain zstd stream. CZI-writer-bug fast path: some real
            // Zeiss files store raw uncompressed bytes even with Compression=5/6
            // when `data_size == expected pixel bytes`; treat those as raw (they
            // are already little-endian samples) rather than feeding non-zstd
            // bytes to the decoder. Matches the reference reader.
            5 if raw.len() == expected => raw.to_vec(),
            5 => zstd_raw_decode(raw, expected)?,
            // Zstd1: header + optional Hi/Lo unpack, then a zstd stream.
            6 if raw.len() == expected => raw.to_vec(),
            6 => zstd1_decode(raw, expected, sb.dtype.size_bytes())?,
            1 => {
                return Err(Error::Unsupported(
                    "CZI JpgFile (compression 1) is not supported".to_string(),
                ))
            }
            2 => {
                return Err(Error::Unsupported(
                    "CZI LZW (compression 2) is not supported".to_string(),
                ))
            }
            // JpegXR: decode the codestream via vendored jxrlib (opt-in feature).
            // Returns native-order LE sample bytes (color tiles stay BGR here;
            // the swap to RGB happens once below, uniform with every codec path).
            #[cfg(feature = "jpegxr")]
            4 => crate::jpegxr_ffi::decode_jpegxr(
                raw,
                sb.stored_x,
                sb.stored_y,
                sb.dtype,
                sb.samples,
            )?,
            #[cfg(not(feature = "jpegxr"))]
            4 => {
                return Err(Error::Unsupported(
                    "CZI JpegXR (compression 4) requires building phasit-imread with \
                     --features jpegxr"
                        .to_string(),
                ))
            }
            other => {
                return Err(Error::Unsupported(format!(
                    "CZI compression id {other} is not supported"
                )))
            }
        };

        // Color: swap B<->R per pixel so callers always receive interleaved RGB.
        // A no-op for grayscale (samples == 1). The swap operates on whole
        // samples (`dtype.size_bytes()` each), correct before the later
        // native-endian byteswap in `PlaneBuf::from_bytes`.
        if sb.samples == 3 {
            Ok(bgr_to_rgb(le_bytes, sb.dtype.size_bytes()))
        } else {
            Ok(le_bytes)
        }
    }
}

/// Blit `tile` (`stored_x * stored_y` pixels, little-endian bytes) into the
/// composed plane at pixel offset `(col_off, row_off)`.
///
/// `pixel_bytes` is the byte size of ONE whole pixel — for color that is
/// `samples * per_sample_bytes` (interleaved samples), so the per-row stride and
/// column offsets are samples-aware and multi-sample tiles compose correctly.
#[allow(clippy::too_many_arguments)]
fn blit_tile(
    composed: &mut [u8],
    scene_x: usize,
    scene_y: usize,
    row_stride: usize,
    pixel_bytes: usize,
    tile: &[u8],
    stored_x: usize,
    stored_y: usize,
    col_off_i: i64,
    row_off_i: i64,
) -> Result<()> {
    // A tile must land fully inside the bounding box (it defines the box, so
    // this holds; validated defensively so a stray coordinate can never write
    // out of bounds). Offsets arrive as i64 so the caller's `Start - origin`
    // subtraction cannot overflow; convert to usize here, rejecting a negative
    // or absurd value rather than relying on any wraparound.
    if col_off_i < 0 || row_off_i < 0 {
        return Err(Error::Decode("CZI tile placed before scene origin".to_string()));
    }
    let col_off = usize::try_from(col_off_i)
        .map_err(|_| Error::Decode("CZI tile column offset out of range".to_string()))?;
    let row_off = usize::try_from(row_off_i)
        .map_err(|_| Error::Decode("CZI tile row offset out of range".to_string()))?;
    let right = col_off.checked_add(stored_x);
    let bottom = row_off.checked_add(stored_y);
    match (right, bottom) {
        (Some(r), Some(b)) if r <= scene_x && b <= scene_y => {}
        _ => {
            return Err(Error::Decode(
                "CZI tile extends past the composed scene bounds".to_string(),
            ))
        }
    }

    let tile_row_bytes = stored_x
        .checked_mul(pixel_bytes)
        .ok_or_else(|| Error::Decode("CZI tile row stride overflow".to_string()))?;
    if tile.len() != tile_row_bytes.saturating_mul(stored_y) {
        return Err(Error::Decode(format!(
            "CZI tile has {} bytes, expected {}",
            tile.len(),
            tile_row_bytes.saturating_mul(stored_y)
        )));
    }
    let col_off_bytes = col_off
        .checked_mul(pixel_bytes)
        .ok_or_else(|| Error::Decode("CZI tile column offset overflow".to_string()))?;

    for ty in 0..stored_y {
        let dst_start = (row_off + ty)
            .checked_mul(row_stride)
            .and_then(|v| v.checked_add(col_off_bytes))
            .ok_or_else(|| Error::Decode("CZI blit destination overflow".to_string()))?;
        let src_start = ty * tile_row_bytes;
        composed[dst_start..dst_start + tile_row_bytes]
            .copy_from_slice(&tile[src_start..src_start + tile_row_bytes]);
    }
    Ok(())
}

/// Blit ONLY the `inter_w x inter_h` sub-rectangle of a decoded `tile` into a
/// rect-sized destination buffer, used by the out-of-core [`CziReader::region`].
///
/// Unlike [`blit_tile`] (which copies a whole tile into a full-plane buffer),
/// this copies just the window where the tile and the requested rect overlap:
///
/// * `dst_row_stride` — bytes per destination row (`rect.w * pixel_bytes`).
/// * `pixel_bytes`    — byte size of ONE whole pixel (`samples * per_sample`),
///   so the copy is samples-aware for color tiles.
/// * `(src_row_off, src_col_off)` — top-left of the overlap WITHIN the tile
///   (`inter_y0 - tile_y0`, `inter_x0 - tile_x0`).
/// * `(dst_row_off, dst_col_off)` — top-left of the overlap WITHIN the rect
///   (`inter_y0 - rect.y`, `inter_x0 - rect.x`).
///
/// Every offset/stride multiply is checked and every row's source/destination
/// span is bounds-checked against the actual slice lengths, so a crafted tile or
/// rect can never index out of bounds (clean `Decode` error instead of a panic).
#[allow(clippy::too_many_arguments)]
fn blit_tile_clipped(
    composed: &mut [u8],
    dst_row_stride: usize,
    pixel_bytes: usize,
    tile: &[u8],
    stored_x: usize,
    stored_y: usize,
    src_row_off: usize,
    src_col_off: usize,
    dst_row_off: usize,
    dst_col_off: usize,
    inter_w: usize,
    inter_h: usize,
) -> Result<()> {
    // Validate the decoded tile is exactly `stored_x * stored_y` pixels — the
    // same invariant `blit_tile` enforces, so a short/long tile is rejected
    // before any copy indexes into it.
    let src_row_stride = stored_x
        .checked_mul(pixel_bytes)
        .ok_or_else(|| Error::Decode("CZI tile row stride overflow".to_string()))?;
    let expected_tile = src_row_stride
        .checked_mul(stored_y)
        .ok_or_else(|| Error::Decode("CZI tile size overflow".to_string()))?;
    if tile.len() != expected_tile {
        return Err(Error::Decode(format!(
            "CZI tile has {} bytes, expected {expected_tile}",
            tile.len()
        )));
    }

    // Bytes copied per row, and the (samples-aware) column byte offsets.
    let copy_bytes = inter_w
        .checked_mul(pixel_bytes)
        .ok_or_else(|| Error::Decode("CZI region copy width overflow".to_string()))?;
    let src_col_bytes = src_col_off
        .checked_mul(pixel_bytes)
        .ok_or_else(|| Error::Decode("CZI region source column overflow".to_string()))?;
    let dst_col_bytes = dst_col_off
        .checked_mul(pixel_bytes)
        .ok_or_else(|| Error::Decode("CZI region dest column overflow".to_string()))?;

    for row in 0..inter_h {
        let src_start = (src_row_off + row)
            .checked_mul(src_row_stride)
            .and_then(|v| v.checked_add(src_col_bytes))
            .ok_or_else(|| Error::Decode("CZI region source offset overflow".to_string()))?;
        let src_end = src_start
            .checked_add(copy_bytes)
            .ok_or_else(|| Error::Decode("CZI region source span overflow".to_string()))?;
        let dst_start = (dst_row_off + row)
            .checked_mul(dst_row_stride)
            .and_then(|v| v.checked_add(dst_col_bytes))
            .ok_or_else(|| Error::Decode("CZI region dest offset overflow".to_string()))?;
        let dst_end = dst_start
            .checked_add(copy_bytes)
            .ok_or_else(|| Error::Decode("CZI region dest span overflow".to_string()))?;
        // Defensive bounds check (the intersection math keeps these in range;
        // verified explicitly so no slice index can ever panic).
        if src_end > tile.len() || dst_end > composed.len() {
            return Err(Error::Decode(
                "CZI region blit span out of bounds".to_string(),
            ));
        }
        composed[dst_start..dst_end].copy_from_slice(&tile[src_start..src_end]);
    }
    Ok(())
}

/// Decode a plain zstd stream, refusing output larger than `expected` (a
/// decompression bomb) and requiring it to be exactly `expected` bytes.
fn zstd_raw_decode(input: &[u8], expected: usize) -> Result<Vec<u8>> {
    let mut decoder = StreamingDecoder::new(input)
        .map_err(|e| Error::Decode(format!("CZI zstd: {e}")))?;
    // Reserve the known output size up front (the exact-length check below
    // rejects anything else), avoiding `read_to_end`'s doubling realloc churn.
    let mut out = Vec::with_capacity(expected);
    // Read at most `expected + 1` bytes: one extra proves the stream is not
    // longer than the declared uncompressed size (bomb guard).
    let cap = expected as u64 + 1;
    decoder
        .by_ref()
        .take(cap)
        .read_to_end(&mut out)
        .map_err(|e| Error::Decode(format!("CZI zstd read: {e}")))?;
    if out.len() != expected {
        return Err(Error::Decode(format!(
            "CZI zstd produced {} bytes, expected {expected}",
            out.len()
        )));
    }
    Ok(out)
}

/// Decode a CZI **Zstd1** subblock.
///
/// Validated against imagecodecs `zstd1_encode` output (byte-identical to what
/// real Zeiss ZSTDHDR subblocks store — see `zstd_fixtures_match_ground_truth`).
/// The header layout observed in every fixture, matching czifile (libCZI not
/// verified):
///
/// ```text
/// byte 0            : header size in bytes (1 with no Hi/Lo, 3 with Hi/Lo)
/// if header size >=3: byte 1 = chunk type; chunk type 1 == "Hi/Lo byte unpack",
///                     byte 2 = flags, bit 0 set => Hi/Lo unpacking is applied
/// bytes[header..]   : the zstd stream
/// ```
///
/// With Hi/Lo unpacking on 16-bit data the decoded buffer is stored as all low
/// bytes followed by all high bytes; it is de-interleaved back to little-endian
/// samples here (confirmed against the fixtures).
fn zstd1_decode(input: &[u8], expected: usize, itemsize: usize) -> Result<Vec<u8>> {
    if input.is_empty() {
        return Err(Error::Decode("CZI Zstd1 subblock is empty".to_string()));
    }
    let header_size = input[0] as usize;
    if header_size == 0 || header_size > input.len() {
        return Err(Error::Decode(format!(
            "CZI Zstd1 header size {header_size} invalid for {} input bytes",
            input.len()
        )));
    }
    let mut hilo = false;
    if header_size >= 3 {
        let chunk_type = input[1];
        // chunk type 1 == "Hi/Lo byte unpack" preprocessing; its 1-byte payload
        // carries the enable flag in bit 0.
        if chunk_type == 1 {
            hilo = (input[2] & 0x01) != 0;
        }
    }

    let payload = &input[header_size..];
    let raw = zstd_raw_decode(payload, expected)?;

    if hilo && itemsize == 2 {
        Ok(deinterleave_hilo16(&raw))
    } else {
        // Hi/Lo unpacking is only defined for 16-bit samples; for other item
        // sizes the decoded buffer is already the sample bytes.
        Ok(raw)
    }
}

/// De-interleave a Hi/Lo byte-packed 16-bit buffer (`[low_0..low_n, high_0..high_n]`)
/// back to little-endian samples (`low_i, high_i, …`).
fn deinterleave_hilo16(raw: &[u8]) -> Vec<u8> {
    let n = raw.len() / 2;
    let (lo, hi) = raw.split_at(n);
    let mut out = vec![0u8; raw.len()];
    for i in 0..n {
        out[2 * i] = lo[i];
        out[2 * i + 1] = hi[i];
    }
    out
}

/// The scene/coordinate key of one subblock (kept only during `open()`).
struct Coord {
    s: i32,
    c: i32,
    z: i32,
    t: i32,
    subblock: usize,
}

/// A parsed `DirectoryEntryDV` from the subblock directory (fields we consume).
struct RawDirEntry {
    pixel_type: i32,
    file_position: i64,
    compression: i32,
    start_x: i32,
    start_y: i32,
    size_x: usize,
    size_y: usize,
    stored_x: usize,
    stored_y: usize,
    /// Coordinate indices (default 0 when the dimension is absent).
    s: i32,
    c: i32,
    z: i32,
    t: i32,
    /// Mosaic index (`M.Start`); `-1` when the dimension is absent.
    m: i32,
}

/// Parse the `FileHeaderSegment`, returning `(DirectoryPosition, MetadataPosition)`.
fn parse_file_header(buf: &[u8]) -> Result<(usize, usize)> {
    if !segment_id_is(buf, 0, b"ZISRAWFILE") {
        return Err(Error::Parse(
            "CZI: first segment is not ZISRAWFILE".to_string(),
        ));
    }
    let sd = SEGMENT_HEADER_LEN;
    // DirectoryPosition @ +52, MetadataPosition @ +60 within the segment data.
    let dir_pos = rd_i64(buf, sd + 52)?;
    let meta_pos = rd_i64(buf, sd + 60)?;
    let dir_pos = position_in_file(dir_pos, buf.len(), "DirectoryPosition")?;
    let meta_pos = position_in_file(meta_pos, buf.len(), "MetadataPosition")?;
    Ok((dir_pos, meta_pos))
}

/// Parse the `SubBlockDirectorySegment` (at the validated `dir_pos`) into raw
/// directory entries.
fn parse_directory(buf: &[u8], dir_pos: usize) -> Result<Vec<RawDirEntry>> {
    if !segment_id_is(buf, dir_pos, b"ZISRAWDIRECTORY") {
        return Err(Error::Parse(
            "CZI: DirectoryPosition does not point at ZISRAWDIRECTORY".to_string(),
        ));
    }
    // Bound the entry walk by this segment's own AllocatedSize (bytes 16..24 of
    // the header), not just the whole mmap, so a crafted directory cannot parse
    // entries out of the bytes belonging to a following segment.
    let allocated = rd_i64(buf, dir_pos + 16)?;
    if allocated < 0 {
        return Err(Error::Parse(
            "CZI directory AllocatedSize is negative".to_string(),
        ));
    }
    let allocated = usize::try_from(allocated)
        .map_err(|_| Error::Parse("CZI directory AllocatedSize overflows usize".to_string()))?;
    let data = dir_pos + SEGMENT_HEADER_LEN;
    let dir_end = data
        .checked_add(allocated)
        .map(|e| e.min(buf.len()))
        .ok_or_else(|| Error::Parse("CZI directory AllocatedSize runs past usize".to_string()))?;
    let entry_count = rd_i32(buf, data)?;
    if entry_count < 0 {
        return Err(Error::Parse(format!(
            "CZI directory EntryCount {entry_count} is negative"
        )));
    }
    let entry_count = entry_count as usize;
    if entry_count > MAX_SUBBLOCKS {
        return Err(Error::Parse(format!(
            "CZI directory EntryCount {entry_count} exceeds sane cap {MAX_SUBBLOCKS}"
        )));
    }
    // Cheap upfront floor: every declared entry ultimately points at its own
    // `ZISRAWSUBBLOCK` segment (>= `SEGMENT_HEADER_LEN` bytes), so a directory
    // claiming more entries than the whole file could possibly hold subblocks
    // for is corrupt. Reject BEFORE allocating the entry / subblock / coord
    // `Vec`s so a tiny file cannot force multi-GB of bookkeeping.
    let min_file_bytes = entry_count.checked_mul(SEGMENT_HEADER_LEN).ok_or_else(|| {
        Error::Parse("CZI directory EntryCount times segment size overflows usize".to_string())
    })?;
    if min_file_bytes > buf.len() {
        return Err(Error::Parse(format!(
            "CZI directory EntryCount {entry_count} needs at least {min_file_bytes} bytes but the file is {} bytes",
            buf.len()
        )));
    }

    // EntryCount i32, then 124 reserved bytes, then the entries.
    let mut cursor = data + 4 + 124;
    let mut entries: Vec<RawDirEntry> = Vec::new();
    for _ in 0..entry_count {
        if cursor > dir_end {
            return Err(Error::Parse(
                "CZI directory entry runs past its segment AllocatedSize".to_string(),
            ));
        }
        let (entry, consumed) = parse_dir_entry(buf, cursor)?;
        entries.push(entry);
        cursor = cursor
            .checked_add(consumed)
            .filter(|&c| c <= dir_end)
            .ok_or_else(|| {
                Error::Parse(
                    "CZI directory entry runs past its segment AllocatedSize".to_string(),
                )
            })?;
    }
    Ok(entries)
}

/// Parse one `DirectoryEntryDV` at `off`, returning it and the bytes consumed.
fn parse_dir_entry(buf: &[u8], off: usize) -> Result<(RawDirEntry, usize)> {
    // SchemaType[2], PixelType i32 @2, FilePosition i64 @6, FilePart i32 @14,
    // Compression i32 @18, PyramidType u8 @22, spare u8 @23, spare[4] @24,
    // DimensionCount i32 @28, then dims.
    let pixel_type = rd_i32(buf, off + 2)?;
    let file_position = rd_i64(buf, off + 6)?;
    let compression = rd_i32(buf, off + 18)?;
    let dim_count = rd_i32(buf, off + 28)?;
    if dim_count < 0 || dim_count as usize > MAX_DIMS_PER_ENTRY {
        return Err(Error::Parse(format!(
            "CZI subblock DimensionCount {dim_count} out of range (cap {MAX_DIMS_PER_ENTRY})"
        )));
    }
    let dim_count = dim_count as usize;

    let mut start_x = 0i32;
    let mut start_y = 0i32;
    let mut size_x: Option<usize> = None;
    let mut size_y: Option<usize> = None;
    let mut stored_x: Option<usize> = None;
    let mut stored_y: Option<usize> = None;
    let (mut s, mut c, mut z, mut t) = (0i32, 0i32, 0i32, 0i32);
    // The mosaic index: `-1` when the entry carries no `M` dimension, the same
    // sentinel `czifile` uses (`czifile.py:4598`).
    let mut m = -1i32;
    // Whether an `M` dimension entry has been seen yet: when a subblock carries
    // SEVERAL `M` dimension entries, the FIRST one in file order wins.
    let mut m_seen = false;

    for i in 0..dim_count {
        let d = off + DIR_ENTRY_FIXED + i * DIM_ENTRY_LEN;
        let dim = *buf
            .get(d)
            .ok_or_else(|| bounds(d, 1, buf.len()))?;
        let start = rd_i32(buf, d + 4)?;
        let size = rd_i32(buf, d + 8)?;
        // d + 12 is StartCoordinate f32 (ignored); d + 16 is StoredSize i32.
        let stored = rd_i32(buf, d + 16)?;

        match dim {
            b'X' => {
                start_x = start;
                size_x = Some(nonneg_usize(size, "X.Size")?);
                stored_x = Some(nonneg_usize(stored, "X.StoredSize")?);
            }
            b'Y' => {
                start_y = start;
                size_y = Some(nonneg_usize(size, "Y.Size")?);
                stored_y = Some(nonneg_usize(stored, "Y.StoredSize")?);
            }
            // S/C/Z/T carry ONE logical value per subblock in this model: we
            // key scenes/planes off `Start` and discard `Size`. A subblock that
            // legitimately spans multiple values on such an axis (Zeiss
            // line-scan packs e.g. `T.Size=3500` into one block) would silently
            // collapse to a single plane and misreport the acquisition, so fail
            // loud instead — matching the fail-closed policy for unmodelled dims
            // with a non-zero Start. Decoding packed frames is future work.
            b'S' => {
                reject_packed_axis(b'S', size)?;
                s = start;
            }
            b'C' => {
                reject_packed_axis(b'C', size)?;
                c = start;
            }
            b'Z' => {
                reject_packed_axis(b'Z', size)?;
                z = start;
            }
            b'T' => {
                reject_packed_axis(b'T', size)?;
                t = start;
            }
            // The mosaic index: ONE value per subblock, like `czifile`.
            //
            // A PACKED `M` (`Size > 1`) is ACCEPTED, not refused: `czifile`
            // takes the `Start` and ignores the `Size` (`czifile.py:4616-4617`
            // — no size check at all), and the `M` axis contributes NO pixels
            // to the plane geometry this reader models, so a packed `M` cannot
            // make the composed plane wrong. Refusing it would fail a whole
            // FILE over an axis that changes nothing but the draw order.
            //
            // A subblock that carries SEVERAL `M` dimension entries takes the
            // FIRST one in file order: `czifile` walks the dimension entries in
            // reverse (`czifile.py:4609`), so the first `M` in the file is the
            // last one assigned (`czifile.py:4617`).
            b'M' => {
                if !m_seen {
                    m = start;
                    m_seen = true;
                }
            }
            // Any other dimension (B, V, H, R, I, …) is not modelled. It is
            // harmless only at index 0; a non-zero start would mean unmodelled
            // data, so fail closed rather than silently fold planes together.
            _ => {
                if start != 0 {
                    return Err(Error::Unsupported(format!(
                        "CZI has an unmodelled dimension {:?} with Start={start} (not supported)",
                        dim as char
                    )));
                }
            }
        }
    }

    let (size_x, size_y, stored_x, stored_y) = match (size_x, size_y, stored_x, stored_y) {
        (Some(a), Some(b), Some(cc), Some(dd)) => (a, b, cc, dd),
        _ => {
            return Err(Error::Parse(
                "CZI subblock is missing an X or Y dimension entry".to_string(),
            ))
        }
    };

    let entry = RawDirEntry {
        pixel_type,
        file_position,
        compression,
        start_x,
        start_y,
        size_x,
        size_y,
        stored_x,
        stored_y,
        s,
        c,
        z,
        t,
        m,
    };
    Ok((entry, DIR_ENTRY_FIXED + dim_count * DIM_ENTRY_LEN))
}

/// Compute a subblock's absolute pixel-data offset and byte length.
///
/// `file_position` is the absolute offset of the `ZISRAWSUBBLOCK` segment. See
/// the module docs for the `max(256, 16 + storage_size)` fixed-part rule.
fn subblock_pixel_span(buf: &[u8], file_position: i64) -> Result<(usize, usize)> {
    let seg = position_in_file(file_position, buf.len(), "subblock FilePosition")?;
    if !segment_id_is(buf, seg, b"ZISRAWSUBBLOCK") {
        return Err(Error::Parse(
            "CZI: subblock FilePosition does not point at ZISRAWSUBBLOCK".to_string(),
        ));
    }
    // The segment's own `AllocatedSize` (bytes 16..24 of the 32-byte header)
    // bounds this subblock's data. A crafted tiny AllocatedSize with a large
    // Metadata/DataSize must not let the pixel span reach into the *next*
    // segment's bytes (it would stay in-mmap but source pixels from adjacent
    // data — a data-integrity bug), so validate against the segment end too.
    let allocated = rd_i64(buf, seg + 16)?;
    if allocated < 0 {
        return Err(Error::Parse(
            "CZI subblock AllocatedSize is negative".to_string(),
        ));
    }
    let allocated = usize::try_from(allocated)
        .map_err(|_| Error::Parse("CZI subblock AllocatedSize overflows usize".to_string()))?;
    let sd = seg + SEGMENT_HEADER_LEN;
    let seg_end = sd
        .checked_add(allocated)
        .ok_or_else(|| Error::Parse("CZI subblock AllocatedSize runs past usize".to_string()))?;
    let metadata_size = nonneg_usize(rd_i32(buf, sd)?, "subblock MetadataSize")?;
    let data_size = rd_i64(buf, sd + 8)?;
    if data_size < 0 {
        return Err(Error::Parse("CZI subblock DataSize is negative".to_string()));
    }
    let data_size = usize::try_from(data_size)
        .map_err(|_| Error::Parse("CZI subblock DataSize overflows usize".to_string()))?;

    // Re-read the subblock's own DirectoryEntryDV DimensionCount (at sd + 16 + 28)
    // to size the fixed part exactly, as the ZISRAW layout requires.
    let dim_count = rd_i32(buf, sd + SUBBLOCK_FIXED_HEADER + 28)?;
    if dim_count < 0 || dim_count as usize > MAX_DIMS_PER_ENTRY {
        return Err(Error::Parse(format!(
            "CZI subblock inner DimensionCount {dim_count} out of range"
        )));
    }
    let storage_size = DIR_ENTRY_FIXED + dim_count as usize * DIM_ENTRY_LEN;
    let fixed = SUBBLOCK_MIN_FIXED.max(SUBBLOCK_FIXED_HEADER + storage_size);

    let pixel_off = sd
        .checked_add(fixed)
        .and_then(|v| v.checked_add(metadata_size))
        .ok_or_else(|| Error::Parse("CZI subblock pixel offset overflow".to_string()))?;
    // Validate the pixel span fits both in the file AND within this segment's
    // own AllocatedSize (result intentionally discarded; the bounds check
    // itself is the point). The segment end must also be within the file, so a
    // huge AllocatedSize cannot itself waive the EOF check.
    let pixel_end = pixel_off
        .checked_add(data_size)
        .ok_or_else(|| Error::Parse("CZI subblock pixel span overflows usize".to_string()))?;
    if pixel_end > buf.len() {
        return Err(Error::Parse(
            "CZI subblock pixel data runs past EOF".to_string(),
        ));
    }
    if pixel_end > seg_end || seg_end > buf.len() {
        return Err(Error::Parse(
            "CZI subblock pixel data runs past its segment AllocatedSize".to_string(),
        ));
    }
    Ok((pixel_off, data_size))
}

/// The reason a CZI scene is refused because it LOST tiles: how many of its
/// full-resolution tiles were skipped, and why they were skipped. The scene's
/// rank among the file's scenes AND its `S` value are both named, so the
/// reason points at the scene of the FILE and not only at its rank among the
/// published scenes.
///
/// The scene is only refused when it actually lost a tile, so the cause list
/// is never empty.
fn lost_tiles_reason(rank: usize, s_val: i32, total: usize, skips: &SceneSkips) -> String {
    let mut causes = Vec::new();
    if skips.unmodelled_pixel_type > 0 {
        causes.push(format!(
            "{} tile(s) of a pixel type the reader does not model",
            skips.unmodelled_pixel_type
        ));
    }
    if skips.bad_span > 0 {
        causes.push(format!(
            "{} tile(s) whose pixel span is out of bounds",
            skips.bad_span
        ));
    }
    let cause = causes.join("; ");
    let skipped = skips.total();
    if skipped >= total {
        format!(
            "scene {rank} of the file (S={s_val}): all {total} of its full-resolution tiles \
             were skipped ({cause})"
        )
    } else {
        format!(
            "scene {rank} of the file (S={s_val}): {skipped} of its {total} full-resolution \
             tiles were skipped ({cause})"
        )
    }
}

/// Bounding box (origin + width/height in pixels) of a scene's tiles.
fn scene_bbox(members: &[&Coord], subblocks: &[SubBlock]) -> Result<(i32, i32, usize, usize)> {
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i64::MIN;
    let mut max_y = i64::MIN;
    for co in members {
        let sb = &subblocks[co.subblock];
        min_x = min_x.min(sb.start_x);
        min_y = min_y.min(sb.start_y);
        max_x = max_x.max(sb.start_x as i64 + sb.stored_x as i64);
        max_y = max_y.max(sb.start_y as i64 + sb.stored_y as i64);
    }
    // NOTE: the extent is deliberately NOT bounded by `MAX_SCENE_DIM` here. A
    // whole-slide (or crafted) scene with a huge derived extent MUST still OPEN
    // so `region()` can stream bounded windows out-of-core (see
    // `crafted_mosaic_extreme_start_opens_plane_refuses_region_ok`); the
    // dimension bound belongs to the consumer that scales with the whole extent
    // (a consumer that builds pyramids enforces `MAX_SCENE_DIM` on the scene
    // extent), not to `open()`, which allocates nothing plane-sized.
    let width = (max_x - min_x as i64)
        .try_into()
        .map_err(|_| Error::Decode("CZI scene width is invalid".to_string()))?;
    let height = (max_y - min_y as i64)
        .try_into()
        .map_err(|_| Error::Decode("CZI scene height is invalid".to_string()))?;
    Ok((min_x, min_y, width, height))
}

/// Sorted, de-duplicated coordinate values.
fn sorted_distinct<I: Iterator<Item = i32>>(iter: I) -> Vec<i32> {
    iter.collect::<BTreeSet<_>>().into_iter().collect()
}

// --- CZI metadata XML (best-effort: channel names + pixel size) --------------

/// Parse the `MetadataSegment` XML for channel names (in order) and the X/Y/Z
/// pixel size in micrometres. Best-effort: any failure yields defaults so it can
/// never gate decoding.
fn parse_metadata(buf: &[u8], meta_pos: usize) -> Result<(Vec<String>, Calibration)> {
    if !segment_id_is(buf, meta_pos, b"ZISRAWMETADATA") {
        return Ok((Vec::new(), Calibration::default()));
    }
    let sd = meta_pos + SEGMENT_HEADER_LEN;
    let xml_size = nonneg_usize(rd_i32(buf, sd)?, "MetadataSize")?;
    if xml_size > MAX_XML_BYTES {
        return Ok((Vec::new(), Calibration::default()));
    }
    let start = sd + METADATA_FIXED;
    let end = match start.checked_add(xml_size).filter(|&e| e <= buf.len()) {
        Some(e) => e,
        None => return Ok((Vec::new(), Calibration::default())),
    };
    let xml = match std::str::from_utf8(&buf[start..end]) {
        Ok(s) => s,
        Err(_) => return Ok((Vec::new(), Calibration::default())),
    };

    let mut reader = Reader::from_str(xml);
    let mut stack: Vec<String> = Vec::new();
    let mut channel_names: Vec<String> = Vec::new();
    let mut cur_dist_id: Option<char> = None;
    let mut in_value = false;
    let (mut px_x, mut px_y, mut px_z) = (None::<f64>, None::<f64>, None::<f64>);

    let handle_start = |e: &quick_xml::events::BytesStart,
                        stack: &[String],
                        channel_names: &mut Vec<String>,
                        cur_dist_id: &mut Option<char>| {
        let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
        if tag == "Channel" && stack.iter().any(|t| t == "Dimensions") {
            if let Some(name) = attr_value(e, b"Name") {
                channel_names.push(name);
            }
        }
        if tag == "Distance" {
            *cur_dist_id = attr_value(e, b"Id").and_then(|v| v.chars().next());
        }
        tag
    };

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let tag = handle_start(&e, &stack, &mut channel_names, &mut cur_dist_id);
                in_value = tag == "Value" && stack.last().map(String::as_str) == Some("Distance");
                stack.push(tag);
            }
            Ok(Event::Empty(e)) => {
                // Self-closing Channel/Distance still carry attributes.
                handle_start(&e, &stack, &mut channel_names, &mut cur_dist_id);
            }
            Ok(Event::Text(t)) => {
                if let Some((id, v)) = distance_value(in_value, cur_dist_id, &t) {
                    match id {
                        'X' => px_x = Some(v),
                        'Y' => px_y = Some(v),
                        'Z' => px_z = Some(v),
                        _ => {}
                    }
                }
            }
            Ok(Event::End(_)) => {
                if let Some(tag) = stack.pop() {
                    if tag == "Distance" {
                        cur_dist_id = None;
                    }
                    if tag == "Value" {
                        in_value = false;
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break, // best-effort: stop on any XML error, keep what we have
            _ => {}
        }
    }

    // Distances are in metres; convert to micrometres. Require X and Y.
    let calibration = match (px_x, px_y) {
        (Some(x), Some(y)) if x > 0.0 && y > 0.0 => Calibration::sized(
            (x * 1e6) as f32,
            (y * 1e6) as f32,
            px_z.map(|v| (v * 1e6) as f32),
        ),
        _ => Calibration::default(),
    };

    Ok((channel_names, calibration))
}

/// Parse a `<Value>` text node inside a `<Distance>` element into `(axis, value)`,
/// or `None` when we are not currently inside such a node. Metres, not converted.
fn distance_value(
    in_value: bool,
    id: Option<char>,
    t: &quick_xml::events::BytesText,
) -> Option<(char, f64)> {
    if !in_value {
        return None;
    }
    let id = id?;
    let txt = t.decode().ok()?;
    let v = txt.trim().parse::<f64>().ok()?;
    Some((id, v))
}

/// First attribute value for `key` on a start/empty element, if present.
fn attr_value(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<String> {
    e.attributes().flatten().find(|a| a.key.as_ref() == key).map(|a| {
        String::from_utf8_lossy(a.value.as_ref()).into_owned()
    })
}

// --- primitive readers + helpers ---------------------------------------------

/// True when the 16-byte SID at `off` equals `id` (null-padded to 16 bytes).
fn segment_id_is(buf: &[u8], off: usize, id: &[u8]) -> bool {
    match buf.get(off..off + 16) {
        Some(sid) => sid.iter().zip(id).all(|(a, b)| a == b) && sid[id.len()..].iter().all(|&b| b == 0),
        None => false,
    }
}

/// Convert a declared absolute file position to a bounds-checked `usize` offset.
fn position_in_file(pos: i64, len: usize, what: &str) -> Result<usize> {
    if pos < 0 {
        return Err(Error::Parse(format!("CZI {what} {pos} is negative")));
    }
    let pos = usize::try_from(pos)
        .map_err(|_| Error::Parse(format!("CZI {what} overflows usize")))?;
    // Need at least a segment header there.
    if pos.saturating_add(SEGMENT_HEADER_LEN) > len {
        return Err(Error::Parse(format!(
            "CZI {what} {pos} points past EOF ({len} bytes)"
        )));
    }
    Ok(pos)
}

fn nonneg_usize(v: i32, what: &str) -> Result<usize> {
    if v < 0 {
        return Err(Error::Parse(format!("CZI {what} {v} is negative")));
    }
    Ok(v as usize)
}

/// Reject a subblock that packs multiple values of a modelled non-X/Y axis
/// (S/C/Z/T) into one block (`Size > 1`, e.g. Zeiss line-scan). We model one
/// logical value per subblock on these axes, so a packed block would silently
/// under-report the acquisition; fail loud instead of decoding it (future work).
fn reject_packed_axis(dim: u8, size: i32) -> Result<()> {
    if size > 1 {
        return Err(Error::Unsupported(format!(
            "CZI subblock packs multiple {} values in one block (line-scan style) — not yet supported",
            dim as char
        )));
    }
    Ok(())
}

fn rd_i32(buf: &[u8], off: usize) -> Result<i32> {
    let s = buf.get(off..off + 4).ok_or_else(|| bounds(off, 4, buf.len()))?;
    Ok(i32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn rd_i64(buf: &[u8], off: usize) -> Result<i64> {
    let s = buf.get(off..off + 8).ok_or_else(|| bounds(off, 8, buf.len()))?;
    Ok(i64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
}

fn bounds(off: usize, need: usize, len: usize) -> Error {
    Error::Parse(format!(
        "CZI read out of bounds: need {need} bytes at offset {off}, file is {len} bytes"
    ))
}

/// Map a CZI `PixelType` id to `(per-sample DType, samples per pixel)`.
///
/// Color pixel types are carried as interleaved samples per pixel, NOT as extra
/// C channels: Bgr24 is `(U8, 3)` and Bgr48 is `(U16, 3)`. NOTE the id space:
/// this is the *PixelType* enum, so id 4 is Bgr48 — do not confuse it with the
/// separate *Compression* enum where id 4 is JpegXR. On decode the B/R samples
/// are swapped so callers always receive R,G,B order (see `bgr_to_rgb`).
fn pixel_type_to_dtype(pixel_type: i32) -> Result<(DType, u32)> {
    match pixel_type {
        0 => Ok((DType::U8, 1)),  // Gray8
        1 => Ok((DType::U16, 1)), // Gray16
        2 => Ok((DType::F32, 1)), // Gray32Float
        3 => Ok((DType::U8, 3)),  // Bgr24  -> RGB, 3x u8 per pixel
        4 => Ok((DType::U16, 3)), // Bgr48  -> RGB, 3x u16 per pixel
        // 10 Gray32 (u32), 13 Gray32Int, Bgr96Float, complex, …: not modelled.
        other => Err(Error::Unsupported(format!(
            "CZI PixelType {other} is not supported"
        ))),
    }
}

/// Swap the B and R samples of an interleaved `[B,G,R, B,G,R, …]` little-endian
/// tile IN PLACE, returning the same buffer as `[R,G,B, R,G,B, …]`.
///
/// Takes `le_bytes` by value (it is owned and unused after the call) and mutates
/// it in place — no extra tile-sized copy per color decode. CZI stores color as
/// BGR; we emit RGB. `item` is the per-sample byte size (1 for Bgr24, 2 for
/// Bgr48); the whole sample (all `item` bytes) is moved so this is correct
/// before the later native-endian byteswap. Byte length must be a multiple of
/// `3 * item`; a non-conforming length is returned unchanged (the caller's
/// exact-length check has already validated conforming tiles).
// Plain modulo (not `usize::is_multiple_of`, which is Rust 1.87+) keeps the
// crate buildable on older stable toolchains, matching `PlaneBuf::from_bytes`.
#[allow(clippy::manual_is_multiple_of)]
fn bgr_to_rgb(mut le_bytes: Vec<u8>, item: usize) -> Vec<u8> {
    let pixel = 3 * item;
    if pixel == 0 || le_bytes.len() % pixel != 0 {
        return le_bytes;
    }
    for px in le_bytes.chunks_exact_mut(pixel) {
        // px = [B_0..B_item, G_0..G_item, R_0..R_item]; swap the B and R samples.
        let (b, rest) = px.split_at_mut(item);
        let r = &mut rest[item..]; // skip the G sample in the middle
        b.swap_with_slice(r);
    }
    le_bytes
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_type_mapping() {
        assert_eq!(pixel_type_to_dtype(0).unwrap(), (DType::U8, 1)); // Gray8
        assert_eq!(pixel_type_to_dtype(1).unwrap(), (DType::U16, 1)); // Gray16
        assert_eq!(pixel_type_to_dtype(2).unwrap(), (DType::F32, 1)); // Gray32Float
        assert_eq!(pixel_type_to_dtype(3).unwrap(), (DType::U8, 3)); // Bgr24 -> RGB
        assert_eq!(pixel_type_to_dtype(4).unwrap(), (DType::U16, 3)); // Bgr48 -> RGB48
        assert!(pixel_type_to_dtype(10).is_err()); // Gray32 (u32) not modelled
        assert!(pixel_type_to_dtype(13).is_err()); // Gray32Int not modelled
    }

    #[test]
    fn bgr_to_rgb_swaps_b_and_r() {
        // Bgr24: 2 pixels [B,G,R] -> [R,G,B].
        assert_eq!(bgr_to_rgb(vec![1u8, 2, 3, 4, 5, 6], 1), vec![3, 2, 1, 6, 5, 4]);
        // Bgr48 (2-byte samples, still little-endian within each sample): the
        // whole B and R samples move, G stays. One pixel = 6 bytes.
        // [B=0x0201, G=0x0403, R=0x0605] -> [R=0x0605, G=0x0403, B=0x0201].
        assert_eq!(
            bgr_to_rgb(vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06], 2),
            vec![0x05, 0x06, 0x03, 0x04, 0x01, 0x02]
        );
        // Grayscale (item != 3-sample multiple) is returned unchanged.
        assert_eq!(bgr_to_rgb(vec![9, 8], 1), vec![9, 8]);
    }

    #[test]
    fn segment_id_matches_null_padded() {
        let mut buf = vec![0u8; 16];
        buf[..10].copy_from_slice(b"ZISRAWFILE");
        assert!(segment_id_is(&buf, 0, b"ZISRAWFILE"));
        assert!(!segment_id_is(&buf, 0, b"ZISRAWDIRECTORY"));
        // A trailing non-null byte after the id must fail (not a clean match).
        buf[10] = 0x41;
        assert!(!segment_id_is(&buf, 0, b"ZISRAWFILE"));
    }

    #[test]
    fn hilo_deinterleave_roundtrips_to_le() {
        // low bytes [0x01, 0x03], high bytes [0x02, 0x04] => LE samples 0x0201, 0x0403.
        let raw = [0x01, 0x03, 0x02, 0x04];
        assert_eq!(deinterleave_hilo16(&raw), vec![0x01, 0x02, 0x03, 0x04]);
    }

    // --- minimal hand-built CZI exercising the full parse + mosaic path -------
    //
    // `CziReader::open` takes a Path, so the synthetic file is written to a temp
    // path and opened, driving segment parsing, the subblock directory, the
    // byte-exact `max(256, 16 + storage_size)` pixel-offset rule, and 2-tile
    // mosaic composition — all without the multi-GB reference files.

    /// One 20-byte `DimensionEntryDV1` (StoredSize == Size).
    fn dim(ch: u8, start: i32, size: i32) -> [u8; 20] {
        let mut d = [0u8; 20];
        d[0] = ch;
        d[4..8].copy_from_slice(&start.to_le_bytes());
        d[8..12].copy_from_slice(&size.to_le_bytes());
        // d[12..16] StartCoordinate f32 = 0.0
        d[16..20].copy_from_slice(&size.to_le_bytes()); // StoredSize == Size
        d
    }

    /// One `DirectoryEntryDV` with an explicit PixelType and dims X,Y,C,Z,M.
    #[allow(clippy::too_many_arguments)]
    fn dir_entry_px(
        file_pos: i64,
        pixel_type: i32,
        xs: i32,
        xsz: i32,
        ys: i32,
        ysz: i32,
        c: i32,
        z: i32,
        m: i32,
    ) -> Vec<u8> {
        let mut e = vec![0u8; DIR_ENTRY_FIXED];
        e[0..2].copy_from_slice(b"DV");
        // pixel_type @2; file_position @6; compression @18 = 0.
        e[2..6].copy_from_slice(&pixel_type.to_le_bytes());
        e[6..14].copy_from_slice(&file_pos.to_le_bytes());
        e[28..32].copy_from_slice(&5i32.to_le_bytes()); // DimensionCount
        e.extend_from_slice(&dim(b'X', xs, xsz));
        e.extend_from_slice(&dim(b'Y', ys, ysz));
        e.extend_from_slice(&dim(b'C', c, 1));
        e.extend_from_slice(&dim(b'Z', z, 1));
        e.extend_from_slice(&dim(b'M', m, 1));
        e
    }

    /// One `DirectoryEntryDV` (Gray8, uncompressed) with dims X,Y,C,Z,M.
    #[allow(clippy::too_many_arguments)]
    fn dir_entry(file_pos: i64, xs: i32, xsz: i32, ys: i32, ysz: i32, c: i32, z: i32, m: i32) -> Vec<u8> {
        dir_entry_px(file_pos, 0, xs, xsz, ys, ysz, c, z, m)
    }

    /// Subblock segment data: fixed header + directory entry + fill to 256, then
    /// zero-length metadata, then the pixel bytes.
    fn subblock_data(entry: &[u8], pixels: &[u8]) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&0i32.to_le_bytes()); // MetadataSize
        d.extend_from_slice(&0i32.to_le_bytes()); // AttachmentSize
        d.extend_from_slice(&(pixels.len() as i64).to_le_bytes()); // DataSize
        d.extend_from_slice(entry); // 132 bytes (16..148)
        d.resize(SUBBLOCK_MIN_FIXED, 0); // fill to 256
        d.extend_from_slice(pixels); // pixels @256
        d
    }

    /// Wrap `data` in a 32-byte segment header (`SID` null-padded to 16 bytes).
    fn seg(sid: &[u8], data: &[u8]) -> Vec<u8> {
        let mut s = vec![0u8; 16];
        s[..sid.len()].copy_from_slice(sid);
        s.extend_from_slice(&(data.len() as i64).to_le_bytes()); // AllocatedSize
        s.extend_from_slice(&(data.len() as i64).to_le_bytes()); // UsedSize
        s.extend_from_slice(data);
        s
    }

    /// Build a 2-tile horizontal mosaic (single C/Z/T): tile A (pixels 1..4) at
    /// x=0, tile B (pixels 5..8) at x=2, each 2x2 Gray8 → composed 2x4.
    fn synthetic_czi() -> Vec<u8> {
        // Segment sizes are fixed, so the absolute offsets are computable ahead.
        let file_total = SEGMENT_HEADER_LEN + 80;
        let xml = b"<ImageDocument/>";
        let md_data_len = METADATA_FIXED + xml.len();
        let meta_total = SEGMENT_HEADER_LEN + md_data_len;
        let dir_data_len = 4 + 124 + 2 * (DIR_ENTRY_FIXED + 5 * DIM_ENTRY_LEN);
        let dir_total = SEGMENT_HEADER_LEN + dir_data_len;
        let sub_total = SEGMENT_HEADER_LEN + (SUBBLOCK_MIN_FIXED + 4);

        let meta_pos = file_total as i64;
        let dir_pos = (file_total + meta_total) as i64;
        let sub_a = (file_total + meta_total + dir_total) as i64;
        let sub_b = sub_a + sub_total as i64;

        // FileHeader data (80 bytes): DirectoryPosition @52, MetadataPosition @60.
        let mut fh = vec![0u8; 80];
        fh[52..60].copy_from_slice(&dir_pos.to_le_bytes());
        fh[60..68].copy_from_slice(&meta_pos.to_le_bytes());

        // Metadata data: XmlSize @0, AttachmentSize @4, Spare[248], then XML.
        let mut md = Vec::new();
        md.extend_from_slice(&(xml.len() as i32).to_le_bytes());
        md.extend_from_slice(&0i32.to_le_bytes());
        md.resize(METADATA_FIXED, 0);
        md.extend_from_slice(xml);

        // Directory data: EntryCount, 124 reserved, then two entries.
        let mut dd = Vec::new();
        dd.extend_from_slice(&2i32.to_le_bytes());
        dd.resize(4 + 124, 0);
        dd.extend_from_slice(&dir_entry(sub_a, 0, 2, 0, 2, 0, 0, 0));
        dd.extend_from_slice(&dir_entry(sub_b, 2, 2, 0, 2, 0, 0, 1));

        let entry_a = dir_entry(sub_a, 0, 2, 0, 2, 0, 0, 0);
        let entry_b = dir_entry(sub_b, 2, 2, 0, 2, 0, 0, 1);
        let sda = subblock_data(&entry_a, &[1, 2, 3, 4]);
        let sdb = subblock_data(&entry_b, &[5, 6, 7, 8]);

        let mut file = Vec::new();
        file.extend_from_slice(&seg(b"ZISRAWFILE", &fh));
        file.extend_from_slice(&seg(b"ZISRAWMETADATA", &md));
        file.extend_from_slice(&seg(b"ZISRAWDIRECTORY", &dd));
        file.extend_from_slice(&seg(b"ZISRAWSUBBLOCK", &sda));
        file.extend_from_slice(&seg(b"ZISRAWSUBBLOCK", &sdb));
        file
    }

    /// A unique temp path for a synthetic CZI (thread + pid + counter scoped).
    fn temp_path() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "phasit_synth_{:?}_{}_{}.czi",
            std::thread::current().id(),
            std::process::id(),
            n
        ))
    }

    /// Write `bytes` to a temp file and attempt to open it, returning the
    /// `Result` (the file is always cleaned up). Used by tests that expect a
    /// crafted/corrupt file to be rejected.
    fn try_open_synth(bytes: &[u8]) -> Result<CziReader> {
        let path = temp_path();
        std::fs::write(&path, bytes).unwrap();
        let r = CziReader::open(&path);
        let _ = std::fs::remove_file(&path);
        r
    }

    fn open_synth(bytes: &[u8]) -> CziReader {
        try_open_synth(bytes).unwrap()
    }

    /// One tile of a synthetic multi-subblock CZI.
    struct TileSpec {
        pixel_type: i32,
        xs: i32,
        xsz: i32,
        ys: i32,
        ysz: i32,
        c: i32,
        z: i32,
        m: i32,
        pixels: Vec<u8>,
    }

    /// Build a synthetic CZI from arbitrary tiles (fixed 5-dim X,Y,C,Z,M
    /// entries, zero-length subblock metadata). Segment sizes are deterministic
    /// so absolute FilePositions are computed up front.
    fn build_czi(tiles: &[TileSpec]) -> Vec<u8> {
        let n = tiles.len();
        let file_total = SEGMENT_HEADER_LEN + 80;
        let xml = b"<ImageDocument/>";
        let md_data_len = METADATA_FIXED + xml.len();
        let meta_total = SEGMENT_HEADER_LEN + md_data_len;
        let dir_data_len = 4 + 124 + n * (DIR_ENTRY_FIXED + 5 * DIM_ENTRY_LEN);
        let dir_total = SEGMENT_HEADER_LEN + dir_data_len;

        // Absolute FilePosition of each subblock segment, in file order.
        let mut sub_pos: Vec<i64> = Vec::with_capacity(n);
        let mut off = file_total + meta_total + dir_total;
        for t in tiles {
            sub_pos.push(off as i64);
            off += SEGMENT_HEADER_LEN + SUBBLOCK_MIN_FIXED + t.pixels.len();
        }

        let meta_pos = file_total as i64;
        let dir_pos = (file_total + meta_total) as i64;

        let mut fh = vec![0u8; 80];
        fh[52..60].copy_from_slice(&dir_pos.to_le_bytes());
        fh[60..68].copy_from_slice(&meta_pos.to_le_bytes());

        let mut md = Vec::new();
        md.extend_from_slice(&(xml.len() as i32).to_le_bytes());
        md.extend_from_slice(&0i32.to_le_bytes());
        md.resize(METADATA_FIXED, 0);
        md.extend_from_slice(xml);

        let mut dd = Vec::new();
        dd.extend_from_slice(&(n as i32).to_le_bytes());
        dd.resize(4 + 124, 0);
        for (t, &sp) in tiles.iter().zip(&sub_pos) {
            dd.extend_from_slice(&dir_entry_px(
                sp, t.pixel_type, t.xs, t.xsz, t.ys, t.ysz, t.c, t.z, t.m,
            ));
        }

        let mut file = Vec::new();
        file.extend_from_slice(&seg(b"ZISRAWFILE", &fh));
        file.extend_from_slice(&seg(b"ZISRAWMETADATA", &md));
        file.extend_from_slice(&seg(b"ZISRAWDIRECTORY", &dd));
        for (t, &sp) in tiles.iter().zip(&sub_pos) {
            let entry =
                dir_entry_px(sp, t.pixel_type, t.xs, t.xsz, t.ys, t.ysz, t.c, t.z, t.m);
            let sd = subblock_data(&entry, &t.pixels);
            file.extend_from_slice(&seg(b"ZISRAWSUBBLOCK", &sd));
        }
        file
    }

    // --- flexible builder for the scene / skip / packed-axis regression tests --
    //
    // Unlike `build_czi` (fixed X,Y,C,Z,M dims), each tile carries an arbitrary
    // `(dim, Start, Size)` list, so a test can add an `S` dimension, set a
    // packed `Size > 1`, etc. `bad_file_pos` optionally points the directory
    // entry's FilePosition at a bogus offset to exercise the skip-not-abort
    // span path (the real subblock segment is still written).

    /// One `DirectoryEntryDV` from explicit `(dim, Start, Size)` tuples
    /// (StoredSize == Size, so no pyramid level is implied). Must include X, Y.
    fn dir_entry_dims(file_pos: i64, pixel_type: i32, dims: &[(u8, i32, i32)]) -> Vec<u8> {
        let mut e = vec![0u8; DIR_ENTRY_FIXED];
        e[0..2].copy_from_slice(b"DV");
        e[2..6].copy_from_slice(&pixel_type.to_le_bytes());
        e[6..14].copy_from_slice(&file_pos.to_le_bytes());
        e[28..32].copy_from_slice(&(dims.len() as i32).to_le_bytes());
        for &(ch, start, size) in dims {
            e.extend_from_slice(&dim(ch, start, size));
        }
        e
    }

    struct RawTile {
        pixel_type: i32,
        /// `(dim char, Start, Size)`, StoredSize == Size. Must include X and Y.
        dims: Vec<(u8, i32, i32)>,
        pixels: Vec<u8>,
        /// Override the directory entry's declared FilePosition (to point it at
        /// a non-subblock offset and drive `subblock_pixel_span` to Err). `None`
        /// uses the real, valid subblock segment position.
        bad_file_pos: Option<i64>,
    }

    fn build_czi_raw(tiles: &[RawTile]) -> Vec<u8> {
        let n = tiles.len();
        let file_total = SEGMENT_HEADER_LEN + 80;
        let xml = b"<ImageDocument/>";
        let md_data_len = METADATA_FIXED + xml.len();
        let meta_total = SEGMENT_HEADER_LEN + md_data_len;
        let dir_data_len: usize = 4
            + 124
            + tiles
                .iter()
                .map(|t| DIR_ENTRY_FIXED + t.dims.len() * DIM_ENTRY_LEN)
                .sum::<usize>();
        let dir_total = SEGMENT_HEADER_LEN + dir_data_len;

        // Real absolute FilePosition of each subblock segment, in file order.
        // Every test entry is small (<= 240 bytes), so the fixed part pads to
        // SUBBLOCK_MIN_FIXED (256) and pixels land at segment-data offset 256.
        let mut sub_pos: Vec<i64> = Vec::with_capacity(n);
        let mut off = file_total + meta_total + dir_total;
        for t in tiles {
            sub_pos.push(off as i64);
            off += SEGMENT_HEADER_LEN + SUBBLOCK_MIN_FIXED + t.pixels.len();
        }

        let meta_pos = file_total as i64;
        let dir_pos = (file_total + meta_total) as i64;

        let mut fh = vec![0u8; 80];
        fh[52..60].copy_from_slice(&dir_pos.to_le_bytes());
        fh[60..68].copy_from_slice(&meta_pos.to_le_bytes());

        let mut md = Vec::new();
        md.extend_from_slice(&(xml.len() as i32).to_le_bytes());
        md.extend_from_slice(&0i32.to_le_bytes());
        md.resize(METADATA_FIXED, 0);
        md.extend_from_slice(xml);

        let mut dd = Vec::new();
        dd.extend_from_slice(&(n as i32).to_le_bytes());
        dd.resize(4 + 124, 0);
        for (t, &sp) in tiles.iter().zip(&sub_pos) {
            let declared = t.bad_file_pos.unwrap_or(sp);
            dd.extend_from_slice(&dir_entry_dims(declared, t.pixel_type, &t.dims));
        }

        let mut file = Vec::new();
        file.extend_from_slice(&seg(b"ZISRAWFILE", &fh));
        file.extend_from_slice(&seg(b"ZISRAWMETADATA", &md));
        file.extend_from_slice(&seg(b"ZISRAWDIRECTORY", &dd));
        for t in tiles {
            // The subblock's own inner DirectoryEntryDV must carry the same dim
            // count so `subblock_pixel_span` sizes the fixed part identically.
            let entry = dir_entry_dims(0, t.pixel_type, &t.dims);
            let sd = subblock_data(&entry, &t.pixels);
            file.extend_from_slice(&seg(b"ZISRAWSUBBLOCK", &sd));
        }
        file
    }

    // Linear scene grouping: `co.s` is attacker-controlled; the old per-scene
    // `coords.iter().filter(...)` was O(scenes x subblocks) == O(N^2) and pinned
    // a CPU on a crafted file with many distinct S values. Grouping is now a
    // single O(N) pass. Build 2000 distinct single-tile scenes and assert they
    // group right (correct scene count) and a plane from a HIGH-S scene decodes
    // (proves the O(1) lookup, not just scene 0).
    #[test]
    fn many_scenes_group_linearly_and_high_scene_decodes() {
        const N: i32 = 2000;
        let mut tiles = Vec::with_capacity(N as usize);
        for s in 0..N {
            tiles.push(RawTile {
                pixel_type: 0, // Gray8
                dims: vec![(b'X', 0, 1), (b'Y', 0, 1), (b'S', s, 1)],
                pixels: vec![(s % 256) as u8],
                bad_file_pos: None,
            });
        }
        let reader = open_synth(&build_czi_raw(&tiles));
        assert_eq!(reader.meta().scene_count(), N as usize, "one scene per S value");

        // Scenes are ordered by sorted S, so scene index N-1 is S = N-1.
        let top = (N - 1) as usize;
        let expect_px = ((N - 1) % 256) as u8;
        let p = reader.plane(top, 0, 0, 0).expect("decode plane from highest-S scene");
        assert_eq!(p.shape, [1, 1]);
        match p.data {
            PlaneBuf::U8(v) => assert_eq!(v, vec![expect_px]),
            other => panic!("expected U8, got {:?}", other.dtype()),
        }
    }

    // Heterogeneous-scene refusal: a heterogeneous scene (Gray8 + Bgr24 at the same coordinate) is
    // REFUSED, but a pure-grayscale sibling scene still opens and decodes.
    #[test]
    fn heterogeneous_scene_is_refused_not_aborted() {
        let tiles = vec![
            // scene 0: valid pure Gray8, 2x2.
            RawTile {
                pixel_type: 0,
                dims: vec![(b'X', 0, 2), (b'Y', 0, 2), (b'S', 0, 1)],
                pixels: vec![10, 20, 30, 40],
                bad_file_pos: None,
            },
            // scene 1: Gray8 + Bgr24 at the same (c,z,t) => heterogeneous.
            RawTile {
                pixel_type: 0,
                dims: vec![(b'X', 0, 2), (b'Y', 0, 2), (b'S', 1, 1), (b'M', 0, 1)],
                pixels: vec![1, 2, 3, 4],
                bad_file_pos: None,
            },
            RawTile {
                pixel_type: 3, // Bgr24 (samples = 3) mixed with Gray8 (samples = 1)
                dims: vec![(b'X', 0, 2), (b'Y', 0, 2), (b'S', 1, 1), (b'M', 1, 1)],
                pixels: vec![0u8; 12],
                bad_file_pos: None,
            },
        ];
        let reader = open_synth(&build_czi_raw(&tiles));
        assert_eq!(reader.meta().scene_count(), 1, "heterogeneous scene refused");
        let scene = reader.meta().scene(0).unwrap();
        assert_eq!(scene.dims, Dims5 { t: 1, c: 1, z: 1, y: 2, x: 2 });
        let p = reader.plane(0, 0, 0, 0).unwrap();
        match p.data {
            PlaneBuf::U8(v) => assert_eq!(v, vec![10, 20, 30, 40]),
            other => panic!("expected U8, got {:?}", other.dtype()),
        }
    }

    // Heterogeneous-scene refusal: when EVERY scene is heterogeneous, open() still fails closed.
    #[test]
    fn all_scenes_heterogeneous_fails_closed() {
        let tiles = vec![
            RawTile { pixel_type: 0, dims: vec![(b'X', 0, 2), (b'Y', 0, 2), (b'S', 0, 1), (b'M', 0, 1)], pixels: vec![1, 2, 3, 4], bad_file_pos: None },
            RawTile { pixel_type: 3, dims: vec![(b'X', 0, 2), (b'Y', 0, 2), (b'S', 0, 1), (b'M', 1, 1)], pixels: vec![0u8; 12], bad_file_pos: None },
        ];
        let res = try_open_synth(&build_czi_raw(&tiles));
        assert!(matches!(res, Err(Error::Unsupported(_))), "all-heterogeneous must fail closed");
    }

    // Lost-tile refusal: a single subblock whose directory FilePosition points at a non-
    // subblock offset (0 == the ZISRAWFILE header) fails span bounds. The scene
    // that holds it LOSES a tile, so it is refused by name; the other scene
    // still opens and decodes.
    #[test]
    fn a_scene_that_lost_a_corrupt_tile_is_refused_not_aborted() {
        let tiles = vec![
            // scene 0: a valid Gray8 tile, 2x2.
            RawTile {
                pixel_type: 0,
                dims: vec![(b'X', 0, 2), (b'Y', 0, 2), (b'S', 0, 1)],
                pixels: vec![10, 20, 30, 40],
                bad_file_pos: None,
            },
            // scene 1: one valid tile and one whose pixel span is out of bounds.
            RawTile {
                pixel_type: 0,
                dims: vec![(b'X', 0, 2), (b'Y', 0, 2), (b'S', 1, 1), (b'C', 0, 1)],
                pixels: vec![50, 60, 70, 80],
                bad_file_pos: None,
            },
            RawTile {
                pixel_type: 0,
                dims: vec![(b'X', 0, 2), (b'Y', 0, 2), (b'S', 1, 1), (b'C', 1, 1)],
                pixels: vec![90, 100, 110, 120],
                bad_file_pos: Some(0),
            },
        ];
        let reader = open_synth(&build_czi_raw(&tiles));
        assert_eq!(
            reader.meta().scene_count(),
            1,
            "the scene that lost a tile is refused, not published smaller"
        );
        let scene = reader.meta().scene(0).unwrap();
        assert_eq!(scene.dims, Dims5 { t: 1, c: 1, z: 1, y: 2, x: 2 });
        let p = reader.plane(0, 0, 0, 0).unwrap();
        match p.data {
            PlaneBuf::U8(v) => assert_eq!(v, vec![10, 20, 30, 40]),
            other => panic!("expected U8, got {:?}", other.dtype()),
        }
        let refused = reader.refused_scenes();
        assert_eq!(refused.len(), 1, "the scene that lost a tile is refused");
        assert!(
            refused[0].reason.contains("out of bounds"),
            "the reason names the cause: {}",
            refused[0].reason
        );
    }

    // Lost-tile refusal: when EVERY subblock span is corrupt, open() fails closed.
    #[test]
    fn all_subblock_spans_corrupt_fails_closed() {
        let tiles = vec![RawTile {
            pixel_type: 0,
            dims: vec![(b'X', 0, 2), (b'Y', 0, 2)],
            pixels: vec![1, 2, 3, 4],
            bad_file_pos: Some(0),
        }];
        let res = try_open_synth(&build_czi_raw(&tiles));
        assert!(matches!(res, Err(Error::Unsupported(_))), "no usable subblock => fail closed");
    }

    // Packed-axis refusal: a subblock packing multiple T values (T.Size = 2, line-scan style)
    // must fail LOUD (Unsupported) rather than silently collapsing to T = 1.
    #[test]
    fn packed_axis_subblock_is_unsupported_not_silent() {
        let tiles = vec![RawTile {
            pixel_type: 0,
            dims: vec![(b'X', 0, 2), (b'Y', 0, 2), (b'T', 0, 2)],
            pixels: vec![1, 2, 3, 4],
            bad_file_pos: None,
        }];
        let res = try_open_synth(&build_czi_raw(&tiles));
        assert!(
            matches!(res, Err(Error::Unsupported(_))),
            "packed T.Size>1 must be Unsupported"
        );
    }

    #[test]
    fn synthetic_mosaic_geometry_and_composition() {
        let reader = open_synth(&synthetic_czi());
        assert_eq!(reader.meta().scene_count(), 1);
        let scene = reader.meta().scene(0).unwrap();
        assert_eq!(scene.dims, Dims5 { t: 1, c: 1, z: 1, y: 2, x: 4 });
        assert_eq!(scene.dtype, DType::U8);

        // Composed plane: tile A (1..4) at x=0, tile B (5..8) at x=2.
        // Row 0 = [1,2,5,6], Row 1 = [3,4,7,8].
        let p = reader.plane(0, 0, 0, 0).unwrap();
        assert_eq!(p.shape, [2, 4]);
        match p.data {
            PlaneBuf::U8(v) => assert_eq!(v, vec![1, 2, 5, 6, 3, 4, 7, 8]),
            other => panic!("expected U8, got {:?}", other.dtype()),
        }
    }

    #[test]
    fn synthetic_out_of_range() {
        let reader = open_synth(&synthetic_czi());
        assert!(matches!(
            reader.plane(0, 0, 1, 0),
            Err(Error::OutOfRange { c: 1, .. })
        ));
        assert!(matches!(
            reader.plane(0, 1, 0, 0),
            Err(Error::OutOfRange { t: 1, .. })
        ));
        assert!(matches!(
            reader.plane(0, 0, 0, 1),
            Err(Error::OutOfRange { z: 1, .. })
        ));
        assert!(matches!(
            reader.plane(1, 0, 0, 0),
            Err(Error::SceneOutOfRange { scene: 1, count: 1 })
        ));
    }

    // A crafted mosaic with tiles at X.Start = i32::MIN and
    // X.Start = 0 in one scene. The per-tile `Start - origin` offset exceeds
    // i32::MAX; on plain i32 that subtraction overflow-panics under the default
    // (dev/test) overflow-checks profile — the i64 offset math in BOTH the
    // full-plane blit and the out-of-core region path must handle it without
    // panicking. An oversize bounding box now OPENS at open() (only the
    // overflow guard fires there; the cap moved to plane()), so this asserts:
    // open() succeeds, plane() still refuses via the cap (clean error, no
    // panic), and a bounded region() read over the FAR tile decodes correctly
    // despite the ~2^31 `Start - origin` column offset.
    #[test]
    fn crafted_mosaic_extreme_start_opens_plane_refuses_region_ok() {
        let tiles = vec![
            TileSpec { pixel_type: 0, xs: i32::MIN, xsz: 2, ys: 0, ysz: 2, c: 0, z: 0, m: 0, pixels: vec![1, 2, 3, 4] },
            TileSpec { pixel_type: 0, xs: 0, xsz: 2, ys: 0, ysz: 2, c: 0, z: 0, m: 1, pixels: vec![5, 6, 7, 8] },
        ];
        let reader = open_synth(&build_czi(&tiles));
        let scene = reader.meta().scene(0).unwrap();
        // origin_x = i32::MIN, so the bounding box spans 2^31 + 2 columns.
        let far_x = (i32::MIN as i64).unsigned_abs() as usize; // 2_147_483_648
        assert_eq!(scene.dims.x, far_x + 2);
        assert_eq!(scene.dims.y, 2);
        // The fully-composed plane is just past the 4 GiB cap: plane() refuses
        // it with a clean error, NEVER an overflow panic in the offset math.
        assert!(matches!(
            reader.plane(0, 0, 0, 0),
            Err(Error::OversizedAllocation { .. })
        ));
        // The far tile B sits at scene-x = 0 - i32::MIN = 2^31; a bounded window
        // there decodes via the i64 `Start - origin` path (tile A at scene-x 0 is
        // skipped without decoding), proving no overflow panic on the extreme
        // offset and correct out-of-core placement.
        let b = reader.region(0, 0, 0, 0, PixelRect::new(far_x, 0, 2, 2)).unwrap();
        assert_eq!(b.shape, [2, 2]);
        match b.data {
            PlaneBuf::U8(v) => assert_eq!(v, vec![5, 6, 7, 8]),
            other => panic!("expected U8, got {:?}", other.dtype()),
        }
    }

    // Unit test: `blit_tile` takes i64 offsets. A column offset beyond
    // i32::MAX must be rejected as out-of-bounds for a small scene, exercising
    // the i64 param + `try_into` conversion without panicking.
    #[test]
    fn blit_tile_rejects_offset_past_i32_max() {
        let mut composed = vec![0u8; 4];
        let tile = vec![0u8; 4];
        let res = blit_tile(
            &mut composed,
            2, // scene_x
            2, // scene_y
            2, // row_stride
            1, // pixel_bytes
            &tile,
            2, // stored_x
            2, // stored_y
            i64::from(i32::MAX) + 1, // col offset that would overflow i32
            0,
        );
        assert!(matches!(res, Err(Error::Decode(_))));
    }

    // A single subblock with an unmodelled PixelType (10 = Gray32/u32)
    // mixed in with valid Gray8 tiles must NOT abort the whole file. The
    // scene that holds it LOSES a tile, so it is refused by name; the other
    // scene still opens and decodes. (Bgr24/Bgr48 are now supported, so a
    // genuinely unmodelled type is used here instead.)
    #[test]
    fn a_scene_that_lost_an_unmodelled_tile_is_refused_not_aborted() {
        let tiles = vec![
            // scene 0: a valid Gray8 tile, 2x2.
            RawTile {
                pixel_type: 0,
                dims: vec![(b'X', 0, 2), (b'Y', 0, 2), (b'S', 0, 1)],
                pixels: vec![10, 20, 30, 40],
                bad_file_pos: None,
            },
            // scene 1: a Gray8 tile and a Gray32/u32 (4 B/px) tile, whose pixel
            // type the reader does not model.
            RawTile {
                pixel_type: 0,
                dims: vec![(b'X', 0, 2), (b'Y', 0, 2), (b'S', 1, 1)],
                pixels: vec![50, 60, 70, 80],
                bad_file_pos: None,
            },
            RawTile {
                pixel_type: 10,
                dims: vec![(b'X', 0, 2), (b'Y', 0, 2), (b'S', 1, 1)],
                pixels: vec![0u8; 16],
                bad_file_pos: None,
            },
        ];
        let reader = open_synth(&build_czi_raw(&tiles));
        assert_eq!(
            reader.meta().scene_count(),
            1,
            "the scene that lost a tile is refused, not published smaller"
        );
        let scene = reader.meta().scene(0).unwrap();
        assert_eq!(scene.dims, Dims5 { t: 1, c: 1, z: 1, y: 2, x: 2 });
        assert_eq!(scene.dtype, DType::U8);
        let p = reader.plane(0, 0, 0, 0).unwrap();
        match p.data {
            PlaneBuf::U8(v) => assert_eq!(v, vec![10, 20, 30, 40]),
            other => panic!("expected U8, got {:?}", other.dtype()),
        }
        let refused = reader.refused_scenes();
        assert_eq!(refused.len(), 1, "the scene that lost a tile is refused");
        assert!(
            refused[0].reason.contains("pixel type"),
            "the reason names the cause: {}",
            refused[0].reason
        );
    }

    // A single 2x2 Bgr24 tile decodes as a color plane — C stays 1, the
    // plane carries 3 interleaved samples per pixel, and B/R are swapped to RGB.
    #[test]
    fn synthetic_bgr24_single_tile_decodes_rgb() {
        // 4 pixels in stored BGR order, row-major (2x2). Pixel k = [B,G,R].
        let bgr = vec![
            1, 2, 3, // (0,0) B=1 G=2 R=3
            4, 5, 6, // (0,1) B=4 G=5 R=6
            7, 8, 9, // (1,0) B=7 G=8 R=9
            10, 11, 12, // (1,1) B=10 G=11 R=12
        ];
        let tiles = vec![TileSpec {
            pixel_type: 3, // Bgr24
            xs: 0,
            xsz: 2,
            ys: 0,
            ysz: 2,
            c: 0,
            z: 0,
            m: 0,
            pixels: bgr,
        }];
        let reader = open_synth(&build_czi(&tiles));
        let scene = reader.meta().scene(0).unwrap();
        // Color is NOT exploded into C: C stays 1, samples-per-pixel is 3.
        assert_eq!(scene.dims, Dims5 { t: 1, c: 1, z: 1, y: 2, x: 2 });
        assert_eq!(scene.dtype, DType::U8);
        assert_eq!(scene.samples, 3);

        let p = reader.plane(0, 0, 0, 0).unwrap();
        assert_eq!(p.shape, [2, 2]);
        assert_eq!(p.samples(), 3);
        match p.data {
            PlaneBuf::U8(v) => {
                assert_eq!(v.len(), 2 * 2 * 3, "interleaved RGB byte length");
                // Each pixel's B and R are swapped -> RGB.
                assert_eq!(v, vec![3, 2, 1, 6, 5, 4, 9, 8, 7, 12, 11, 10]);
            }
            other => panic!("expected U8, got {:?}", other.dtype()),
        }
    }

    // A 2-tile horizontal Bgr24 mosaic composes with a samples-aware row
    // stride. A stride bug (using width*itemsize instead of width*samples*
    // itemsize) would scramble the color rows here.
    #[test]
    fn synthetic_bgr24_mosaic_uses_samples_aware_stride() {
        // Tile A at x=0, tile B at x=2, each 2x2 Bgr24, one C/Z/T, distinct M.
        let tile_a = vec![
            10, 11, 12, 13, 14, 15, // row 0: A00=[B10,G11,R12], A01=[B13,G14,R15]
            16, 17, 18, 19, 20, 21, // row 1: A10, A11
        ];
        let tile_b = vec![
            30, 31, 32, 33, 34, 35, // row 0: B00, B01
            36, 37, 38, 39, 40, 41, // row 1: B10, B11
        ];
        let tiles = vec![
            TileSpec { pixel_type: 3, xs: 0, xsz: 2, ys: 0, ysz: 2, c: 0, z: 0, m: 0, pixels: tile_a },
            TileSpec { pixel_type: 3, xs: 2, xsz: 2, ys: 0, ysz: 2, c: 0, z: 0, m: 1, pixels: tile_b },
        ];
        let reader = open_synth(&build_czi(&tiles));
        let scene = reader.meta().scene(0).unwrap();
        assert_eq!(scene.dims, Dims5 { t: 1, c: 1, z: 1, y: 2, x: 4 });
        assert_eq!(scene.samples, 3);

        let p = reader.plane(0, 0, 0, 0).unwrap();
        assert_eq!(p.shape, [2, 4]);
        assert_eq!(p.samples(), 3);
        match p.data {
            PlaneBuf::U8(v) => {
                assert_eq!(v.len(), 2 * 4 * 3);
                // Row 0 = A00,A01,B00,B01; Row 1 = A10,A11,B10,B11 (each RGB).
                assert_eq!(
                    v,
                    vec![
                        12, 11, 10, 15, 14, 13, 32, 31, 30, 35, 34, 33, // row 0
                        18, 17, 16, 21, 20, 19, 38, 37, 36, 41, 40, 39, // row 1
                    ]
                );
            }
            other => panic!("expected U8, got {:?}", other.dtype()),
        }
    }

    // === out-of-core region() =================================================
    //
    // The core correctness proof is that `region()` is byte-identical to
    // `plane().crop()` for every rect, on grayscale, 16-bit and color mosaics,
    // including windows that straddle tile boundaries and windows over an
    // uncovered gap. Then: an oversize (whole-slide-style) scene OPENS, `plane()`
    // still refuses it via the cap, but `region()` reads bounded windows.

    /// A 2x2 Gray8 tile whose four bytes start at `base` (distinct per tile so a
    /// wrong row/col offset in the clipped blit mismatches immediately).
    fn g8(base: u8) -> Vec<u8> {
        vec![base, base + 1, base + 2, base + 3]
    }

    /// A 2x2 Gray16 tile: four u16 samples `base..base+4`, little-endian bytes.
    /// Takes a `u8` base (to share the `mosaic_tiles` closure signature) and
    /// widens it — the samples still span the full 16-bit little-endian layout.
    fn g16(base: u8) -> Vec<u8> {
        let base = base as u16;
        let mut v = Vec::with_capacity(8);
        for i in 0..4u16 {
            v.extend_from_slice(&(base + i).to_le_bytes());
        }
        v
    }

    /// A 2x2 Bgr24 tile: four pixels, each `[p, p+1, p+2]` in stored BGR order,
    /// `p = base + 3*i` (distinct bytes per pixel and per sample).
    fn bgr(base: u8) -> Vec<u8> {
        let mut v = Vec::with_capacity(12);
        for i in 0..4u8 {
            let p = base + i * 3;
            v.extend_from_slice(&[p, p + 1, p + 2]);
        }
        v
    }

    /// Three 2x2 tiles laid out with the bottom-right cell UNCOVERED, composing a
    /// 4x4 plane with a zero gap:
    ///
    /// ```text
    ///   tile0 (0,0) | tile1 (2,0)
    ///   ------------+------------
    ///   tile2 (0,2) |   (gap=0)
    /// ```
    fn mosaic_tiles(pixel_type: i32, mk: impl Fn(u8) -> Vec<u8>, bases: [u8; 3]) -> Vec<TileSpec> {
        vec![
            TileSpec { pixel_type, xs: 0, xsz: 2, ys: 0, ysz: 2, c: 0, z: 0, m: 0, pixels: mk(bases[0]) },
            TileSpec { pixel_type, xs: 2, xsz: 2, ys: 0, ysz: 2, c: 0, z: 0, m: 1, pixels: mk(bases[1]) },
            TileSpec { pixel_type, xs: 0, xsz: 2, ys: 2, ysz: 2, c: 0, z: 0, m: 2, pixels: mk(bases[2]) },
        ]
    }

    /// The rect battery exercised on the 4x4 mosaic: full plane, boundary
    /// straddles, every corner, the uncovered gap, edge strips, zero-size rects.
    fn mosaic_rects_4x4() -> Vec<PixelRect> {
        vec![
            PixelRect::new(0, 0, 4, 4), // full plane
            PixelRect::new(1, 0, 2, 2), // straddles the horizontal tile boundary
            PixelRect::new(0, 1, 2, 2), // straddles the vertical tile boundary
            PixelRect::new(1, 1, 2, 2), // straddles both + touches the gap corner
            PixelRect::new(0, 0, 1, 1), // top-left corner
            PixelRect::new(3, 0, 1, 1), // top-right corner
            PixelRect::new(0, 3, 1, 1), // bottom-left corner
            PixelRect::new(3, 3, 1, 1), // bottom-right corner (in the gap => 0)
            PixelRect::new(2, 2, 2, 2), // window entirely over the uncovered gap
            PixelRect::new(2, 2, 1, 1), // single pixel in the gap
            PixelRect::new(3, 0, 1, 4), // right-edge column
            PixelRect::new(0, 3, 4, 1), // bottom-edge row
            PixelRect::new(0, 2, 4, 2), // bottom half (two tiles + gap)
            PixelRect::new(0, 0, 0, 0), // zero-size rect at origin
            PixelRect::new(2, 2, 0, 0), // zero-size rect at a nonzero offset
        ]
    }

    /// Assert `region()` is byte-identical to `plane().crop()` for every rect
    /// (full `Plane` equality: data + shape + samples).
    fn assert_region_eq_crop(reader: &CziReader, rects: &[PixelRect]) {
        let full = reader.plane(0, 0, 0, 0).expect("compose full plane");
        for &rect in rects {
            let via_crop = full.crop(rect).expect("crop full plane");
            let via_region = reader.region(0, 0, 0, 0, rect).expect("out-of-core region");
            assert_eq!(via_region.shape, [rect.h, rect.w], "shape for {rect:?}");
            assert_eq!(via_region.samples, via_crop.samples, "samples for {rect:?}");
            assert_eq!(
                via_region, via_crop,
                "region() != plane().crop() for rect {rect:?}"
            );
        }
    }

    #[test]
    fn supports_region_is_true_for_czi() {
        // CZI overrides `region()` with a genuine out-of-core sub-region decode,
        // so it must advertise `supports_region() == true` — this is what keeps
        // a consumer that builds pyramids on the per-tile streaming path (never
        // materializing a whole-slide plane) instead of the plane-once crop
        // path.
        let reader = open_synth(&build_czi(&mosaic_tiles(0, g8, [1, 21, 41])));
        assert!(reader.supports_region());
    }

    #[test]
    fn region_matches_crop_gray8_mosaic() {
        let reader = open_synth(&build_czi(&mosaic_tiles(0, g8, [1, 21, 41])));
        let scene = reader.meta().scene(0).unwrap();
        assert_eq!(scene.dims, Dims5 { t: 1, c: 1, z: 1, y: 4, x: 4 });
        assert_eq!(scene.samples, 1);
        assert_region_eq_crop(&reader, &mosaic_rects_4x4());
    }

    #[test]
    fn region_matches_crop_gray16_mosaic() {
        // pixel_type = 1 (Gray16): 2 bytes per pixel, exercises the samples-aware
        // + itemsize-aware clipped blit and the LE->native conversion.
        let reader = open_synth(&build_czi(&mosaic_tiles(1, g16, [100, 200, 40])));
        let scene = reader.meta().scene(0).unwrap();
        assert_eq!(scene.dims, Dims5 { t: 1, c: 1, z: 1, y: 4, x: 4 });
        assert_eq!(scene.dtype, DType::U16);
        assert_eq!(scene.samples, 1);
        assert_region_eq_crop(&reader, &mosaic_rects_4x4());
    }

    #[test]
    fn region_matches_crop_bgr24_mosaic() {
        // pixel_type = 3 (Bgr24, samples = 3): a wrong per-pixel byte size in the
        // clipped blit would desync the interleave and mismatch immediately.
        let reader = open_synth(&build_czi(&mosaic_tiles(3, bgr, [1, 60, 120])));
        let scene = reader.meta().scene(0).unwrap();
        assert_eq!(scene.dims, Dims5 { t: 1, c: 1, z: 1, y: 4, x: 4 });
        assert_eq!(scene.dtype, DType::U8);
        assert_eq!(scene.samples, 3);
        assert_region_eq_crop(&reader, &mosaic_rects_4x4());
    }

    /// Two OVERLAPPING 2x2 tiles whose SUBBLOCK-DIRECTORY order is the
    /// REVERSE of their mosaic-index order: the tile with the higher M is
    /// written first. The reader must composite in ascending mosaic index —
    /// the order `czifile` uses (`czifile.py:1258-1259`, `:1308-1309`, applied
    /// at `:2810-2811`); libCZI is not verified — so the higher-M tile is
    /// drawn ON TOP of the overlap, not under it.
    ///
    /// ```text
    ///   tile M=1 (dir 0)   tile M=0 (dir 1)
    ///   xs=0               xs=1
    ///   10 11 | 20 21      x=0  x=1  x=2
    ///   12 13 | 22 23
    /// ```
    ///
    /// The overlap is the middle column (x=1). Ascending M draws the M=1 tile
    /// last, so x=1 is 11/13; the subblock-directory order would draw the M=0
    /// tile last and make x=1 20/22.
    #[test]
    fn overlapping_tiles_composite_in_mosaic_index_order() {
        let reader = open_synth(&build_czi(&[
            // M=1 first in the directory, M=0 second: the directory order is
            // the REVERSE of the mosaic-index order.
            TileSpec {
                pixel_type: 0,
                xs: 0,
                xsz: 2,
                ys: 0,
                ysz: 2,
                c: 0,
                z: 0,
                m: 1,
                pixels: vec![10, 11, 12, 13],
            },
            TileSpec {
                pixel_type: 0,
                xs: 1,
                xsz: 2,
                ys: 0,
                ysz: 2,
                c: 0,
                z: 0,
                m: 0,
                pixels: vec![20, 21, 22, 23],
            },
        ]));

        let scene = reader.meta().scene(0).unwrap();
        assert_eq!(scene.dims, Dims5 { t: 1, c: 1, z: 1, y: 2, x: 3 });

        // The whole plane: the overlap column (x=1) belongs to the tile with
        // the higher mosaic index.
        let p = reader.plane(0, 0, 0, 0).expect("compose full plane");
        assert_eq!(p.shape, [2, 3]);
        match p.data {
            PlaneBuf::U8(v) => assert_eq!(
                v,
                vec![10, 11, 21, 12, 13, 23],
                "the tile with the higher mosaic index must be drawn on top"
            ),
            other => panic!("expected U8, got {:?}", other.dtype()),
        }

        // The out-of-core path composes in the same order.
        let overlap = reader
            .region(0, 0, 0, 0, PixelRect::new(1, 0, 1, 2))
            .expect("read the overlap column");
        assert_eq!(overlap.shape, [2, 1]);
        match overlap.data {
            PlaneBuf::U8(v) => assert_eq!(v, vec![11, 13]),
            other => panic!("expected U8, got {:?}", other.dtype()),
        }
    }

    // --- the mosaic-axis rules, one test each ---------------------------------
    //
    // Every test below composes 2x2 Gray8 tiles that OVERLAP and only the
    // mosaic axis differs, so the bytes say which tile was drawn on top. The
    // two-tile tests use these tiles:
    //
    //   tile A: X 0..2, Y 0..2, pixels [10, 11, 12, 13]
    //   tile B: X 1..3, Y 0..2, pixels [20, 21, 22, 23]
    //
    // They share the scene column x=1. A drawn last puts A's column 1 (11, 13)
    // there; B drawn last puts B's column 0 (20, 22) there. The tests that
    // compose three tiles say which columns those three share.

    /// A 2x2 Gray8 tile at `X.Start = xs`, carrying the given mosaic-axis dims.
    fn mosaic_tile(xs: i32, m_dims: &[(u8, i32, i32)], pixels: Vec<u8>) -> RawTile {
        let mut dims = vec![(b'X', xs, 2), (b'Y', 0, 2)];
        dims.extend_from_slice(m_dims);
        RawTile {
            pixel_type: 0,
            dims,
            pixels,
            bad_file_pos: None,
        }
    }

    /// The composed plane of a mosaic, `wide` pixels wide and two rows tall.
    fn mosaic_plane(reader: &CziReader, wide: usize) -> Vec<u8> {
        let p = reader.plane(0, 0, 0, 0).expect("compose full plane");
        assert_eq!(p.shape, [2, wide]);
        match p.data {
            PlaneBuf::U8(v) => v,
            other => panic!("expected U8, got {:?}", other.dtype()),
        }
    }

    /// An ABSENT mosaic dimension is index -1 (czifile.py:4598), so in a
    /// SORTED plane a tile without one is drawn BELOW the tiles that carry a
    /// non-negative `M` — even when it comes first in the subblock directory.
    #[test]
    fn absent_mosaic_axis_draws_below_the_mosaic_tiles() {
        // A carries M=0 and comes FIRST in the directory; B carries no M at
        // all. Ascending mosaic index draws B (-1) first and A (0) last.
        let reader = open_synth(&build_czi_raw(&[
            mosaic_tile(0, &[(b'M', 0, 1)], vec![10, 11, 12, 13]),
            mosaic_tile(1, &[], vec![20, 21, 22, 23]),
        ]));
        assert_eq!(
            mosaic_plane(&reader, 3),
            vec![10, 11, 21, 12, 13, 23],
            "the tile without a mosaic dimension is drawn first, so the M=0 tile is on top"
        );
    }

    /// A plane whose tiles ALL carry a negative mosaic index keeps the
    /// subblock-directory order: the sort only runs when some entry of the
    /// plane carries a non-negative index (czifile.py:1308).
    #[test]
    fn plane_with_only_negative_mosaic_indices_keeps_directory_order() {
        // A carries M=-3 and comes first, B carries M=-5 and comes second.
        // Sorting them by mosaic index would swap them: -5 before -3.
        let reader = open_synth(&build_czi_raw(&[
            mosaic_tile(0, &[(b'M', -3, 1)], vec![10, 11, 12, 13]),
            mosaic_tile(1, &[(b'M', -5, 1)], vec![20, 21, 22, 23]),
        ]));
        assert_eq!(
            mosaic_plane(&reader, 3),
            vec![10, 20, 21, 12, 22, 23],
            "a plane with no non-negative mosaic index keeps its directory order"
        );
    }

    /// A MIXED plane whose `M` values are all negative is NOT sorted at all:
    /// no tile of it carries a non-negative index, so the plane keeps its
    /// subblock-directory order and a tile without an `M` is not necessarily
    /// drawn below the tiles that carry one.
    #[test]
    fn mixed_plane_with_only_negative_mosaic_indices_keeps_directory_order() {
        // A carries no M at all, B carries M=-5, C carries no M at all, and
        // they come in that order in the directory. Sorting them by mosaic
        // index would draw B (-5) first and A (-1) over it, which puts a
        // different tile on top of the column x=1 that A and B share — C
        // reaches x=2 and x=3 only.
        let reader = open_synth(&build_czi_raw(&[
            mosaic_tile(0, &[], vec![10, 11, 12, 13]),
            mosaic_tile(1, &[(b'M', -5, 1)], vec![20, 21, 22, 23]),
            mosaic_tile(2, &[], vec![30, 31, 32, 33]),
        ]));
        assert_eq!(
            mosaic_plane(&reader, 4),
            vec![10, 20, 30, 31, 12, 22, 32, 33],
            "a mixed plane with no non-negative mosaic index keeps its directory order"
        );
    }

    /// A SORTED plane that mixes a negative mosaic index with an absent one
    /// draws the NEGATIVE index first: the tile without an `M` (index -1) is
    /// above the tile with `M = -5` and below the tile with `M = 0`.
    #[test]
    fn sorted_mixed_plane_draws_the_negative_mosaic_index_first() {
        // A carries no M at all, B carries M=-5 and C carries M=0, so the
        // plane IS sorted (C's index is non-negative) and the draw order is
        // B (-5), A (-1), C (0). A and B share the column x=1, and A is drawn
        // over B there, so the plane carries A's column 1 (11, 13) there.
        let reader = open_synth(&build_czi_raw(&[
            mosaic_tile(0, &[], vec![10, 11, 12, 13]),
            mosaic_tile(1, &[(b'M', -5, 1)], vec![20, 21, 22, 23]),
            mosaic_tile(2, &[(b'M', 0, 1)], vec![30, 31, 32, 33]),
        ]));
        assert_eq!(
            mosaic_plane(&reader, 4),
            vec![10, 11, 30, 31, 12, 13, 32, 33],
            "a sorted mixed plane draws the negative mosaic index first"
        );
    }

    /// Two tiles with the SAME mosaic index keep their subblock-directory
    /// order: the sort is stable.
    #[test]
    fn equal_mosaic_indices_keep_the_subblock_directory_order() {
        let reader = open_synth(&build_czi_raw(&[
            mosaic_tile(0, &[(b'M', 4, 1)], vec![10, 11, 12, 13]),
            mosaic_tile(1, &[(b'M', 4, 1)], vec![20, 21, 22, 23]),
        ]));
        assert_eq!(
            mosaic_plane(&reader, 3),
            vec![10, 20, 21, 12, 22, 23],
            "tiles that share a mosaic index keep their directory order"
        );
    }

    /// A subblock that carries SEVERAL mosaic dimension entries takes the FIRST
    /// one in file order: `czifile` walks the dimension entries in reverse
    /// (czifile.py:4609), so the first M in the file is the last one assigned.
    #[test]
    fn duplicate_mosaic_dimension_takes_the_first_one() {
        // A carries M=7 and then M=3; its mosaic index is 7, so it is drawn
        // after B's M=5 and ends up on top.
        let reader = open_synth(&build_czi_raw(&[
            mosaic_tile(0, &[(b'M', 7, 1), (b'M', 3, 1)], vec![10, 11, 12, 13]),
            mosaic_tile(1, &[(b'M', 5, 1)], vec![20, 21, 22, 23]),
        ]));
        assert_eq!(
            mosaic_plane(&reader, 3),
            vec![10, 11, 21, 12, 13, 23],
            "the FIRST mosaic dimension entry wins"
        );
    }

    /// A packed mosaic dimension (`M.Size > 1`) is ACCEPTED, like `czifile`
    /// accepts it (czifile.py:4616-4617 takes the Start and ignores the Size).
    /// It used to fail the open of the whole file.
    #[test]
    fn packed_mosaic_dimension_still_opens_the_file() {
        let reader = open_synth(&build_czi_raw(&[
            mosaic_tile(0, &[(b'M', 1, 2)], vec![10, 11, 12, 13]),
            mosaic_tile(1, &[(b'M', 0, 1)], vec![20, 21, 22, 23]),
        ]));
        assert_eq!(
            mosaic_plane(&reader, 3),
            vec![10, 11, 21, 12, 13, 23],
            "a packed mosaic axis uses its Start and does not refuse the file"
        );
    }

    /// Two tiny 2x2 tiles placed far apart (A at (0,0), B at (70000,70000)) so
    /// the scene bounding box is 70002x70002 and its fully-composed Gray8 plane
    /// is ~4.56 GiB — past `MAX_DECODED_PLANE_BYTES`. This is the whole-slide
    /// scenario in miniature.
    fn oversize_scene() -> Vec<u8> {
        build_czi(&[
            TileSpec { pixel_type: 0, xs: 0, xsz: 2, ys: 0, ysz: 2, c: 0, z: 0, m: 0, pixels: vec![1, 2, 3, 4] },
            TileSpec { pixel_type: 0, xs: 70000, xsz: 2, ys: 70000, ysz: 2, c: 0, z: 0, m: 1, pixels: vec![5, 6, 7, 8] },
        ])
    }

    #[test]
    fn oversize_scene_opens_plane_refuses_region_works() {
        let reader = open_synth(&oversize_scene());
        let scene = reader.meta().scene(0).unwrap();
        // The bounding box is huge, but the scene OPENS.
        assert_eq!(scene.dims, Dims5 { t: 1, c: 1, z: 1, y: 70002, x: 70002 });
        assert_eq!(scene.dtype, DType::U8);
        // Sanity: the full plane really is past the 4 GiB cap.
        assert!(70002usize * 70002 > crate::guard::MAX_DECODED_PLANE_BYTES);

        // plane() still materializes the whole plane, so it MUST refuse.
        assert!(
            matches!(reader.plane(0, 0, 0, 0), Err(Error::OversizedAllocation { .. })),
            "plane() on an oversize scene must be refused by the cap"
        );

        // region() over the small window covering tile A decodes only tile A
        // (tile B, 70000 px away, is never decoded).
        let a = reader.region(0, 0, 0, 0, PixelRect::new(0, 0, 2, 2)).unwrap();
        assert_eq!(a.shape, [2, 2]);
        match a.data {
            PlaneBuf::U8(v) => assert_eq!(v, vec![1, 2, 3, 4]),
            other => panic!("expected U8, got {:?}", other.dtype()),
        }

        // region() over the far tile B reads its pixels (and skips tile A).
        let b = reader.region(0, 0, 0, 0, PixelRect::new(70000, 70000, 2, 2)).unwrap();
        assert_eq!(b.shape, [2, 2]);
        match b.data {
            PlaneBuf::U8(v) => assert_eq!(v, vec![5, 6, 7, 8]),
            other => panic!("expected U8, got {:?}", other.dtype()),
        }

        // region() over the empty gap between the tiles returns all zeros.
        let gap = reader.region(0, 0, 0, 0, PixelRect::new(10, 10, 2, 2)).unwrap();
        assert_eq!(gap.shape, [2, 2]);
        match gap.data {
            PlaneBuf::U8(v) => assert_eq!(v, vec![0, 0, 0, 0]),
            other => panic!("expected U8, got {:?}", other.dtype()),
        }
    }

    #[test]
    fn region_own_size_exceeding_cap_is_refused() {
        // On the oversize scene, a rect whose OWN byte size exceeds the cap is
        // refused before allocation, even though the enclosing scene opened.
        let reader = open_synth(&oversize_scene());
        let huge = PixelRect::new(0, 0, 70000, 70000); // ~4.56 GiB of Gray8
        assert!(70000usize * 70000 > crate::guard::MAX_DECODED_PLANE_BYTES);
        assert!(
            matches!(
                reader.region(0, 0, 0, 0, huge),
                Err(Error::OversizedAllocation { .. })
            ),
            "a rect larger than the cap must be refused"
        );
    }

    #[test]
    fn region_out_of_bounds_and_overflow_are_clean_errors() {
        let reader = open_synth(&build_czi(&mosaic_tiles(0, g8, [1, 21, 41])));
        // rect past dims.x (x + w == 5 > 4).
        assert!(matches!(
            reader.region(0, 0, 0, 0, PixelRect::new(3, 0, 2, 1)),
            Err(Error::Decode(_))
        ));
        // rect past dims.y (y + h == 5 > 4).
        assert!(matches!(
            reader.region(0, 0, 0, 0, PixelRect::new(0, 3, 1, 2)),
            Err(Error::Decode(_))
        ));
        // x + w overflows usize -> clean Decode, never a panic/wrap.
        assert!(matches!(
            reader.region(0, 0, 0, 0, PixelRect::new(usize::MAX, 0, 2, 1)),
            Err(Error::Decode(_))
        ));
        // y + h overflows usize.
        assert!(matches!(
            reader.region(0, 0, 0, 0, PixelRect::new(0, usize::MAX, 1, 2)),
            Err(Error::Decode(_))
        ));
        // Out-of-range scene / t / c / z stay consistent with plane().
        assert!(matches!(
            reader.region(1, 0, 0, 0, PixelRect::new(0, 0, 1, 1)),
            Err(Error::SceneOutOfRange { scene: 1, .. })
        ));
        assert!(matches!(
            reader.region(0, 1, 0, 0, PixelRect::new(0, 0, 1, 1)),
            Err(Error::OutOfRange { t: 1, .. })
        ));
    }

    // Real-file parity: on a real uncompressed Gray16 mosaic CZI, `region()`
    // must be byte-exact vs `plane().crop()` for a full plane, an interior
    // window, and an edge window. The file is read from the folder named by
    // `PHASIT_REAL_DATA_DIR` (the layout is in the crate README); gated on
    // the file's presence (mirrors `tests/czi_real.rs`) and skips cleanly when
    // it is absent — unless the run sets `PHASIT_REAL_DATA_REQUIRED=1`, which
    // turns that skip into a failure.
    #[test]
    fn region_matches_crop_real_mosaic_czi() {
        const SAMPLE: &str = "czi/mosaic.czi";
        let Some(path) = crate::real_data::real_file(SAMPLE) else {
            return;
        };
        let reader = CziReader::open(&path).expect("open real mosaic CZI");
        let dims = reader.meta().scene(0).unwrap().dims;
        let z_mid = dims.z / 2;
        // A full plane, an interior sub-window, and a bottom-right edge window.
        let rects = [
            PixelRect::new(0, 0, dims.x, dims.y),
            PixelRect::new(dims.x / 4, dims.y / 4, dims.x / 2, dims.y / 2),
            PixelRect::new(dims.x.saturating_sub(37), dims.y.saturating_sub(29), 37, 29),
        ];
        let full = reader.plane(0, 0, 0, z_mid).expect("compose real plane");
        for &rect in &rects {
            let via_crop = full.crop(rect).expect("crop real plane");
            let via_region = reader
                .region(0, 0, 0, z_mid, rect)
                .expect("out-of-core region on real file");
            assert_eq!(via_region.shape, [rect.h, rect.w], "real shape for {rect:?}");
            assert_eq!(
                via_region, via_crop,
                "real region() != plane().crop() for rect {rect:?}"
            );
        }
        eprintln!("real-file region parity PASSED for {SAMPLE}");
    }

    // Byte-exact Zstd fixtures produced by imagecodecs (the SAME codec
    // czifile uses to decode real Zeiss zstd subblocks). Feed each `.bin`
    // through the reader's zstd path exactly as `decode_tile` dispatches by
    // compression id (zstd0 -> id 5 plain stream; zstd1/zstd1hilo -> id 6 header
    // + optional Hi/Lo) and assert the output equals the `.raw` ground truth.
    #[test]
    fn zstd_fixtures_match_ground_truth() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/zstd");
        let manifest = std::fs::read_to_string(format!("{dir}/manifest.json"))
            .expect("read zstd manifest.json");
        let entries = parse_manifest(&manifest);
        assert!(!entries.is_empty(), "manifest parsed no entries");

        let mut hilo_seen = 0usize;
        for (name, variant, itemsize) in &entries {
            let bin = std::fs::read(format!("{dir}/{name}.bin"))
                .unwrap_or_else(|e| panic!("read {name}.bin: {e}"));
            let raw = std::fs::read(format!("{dir}/{name}.raw"))
                .unwrap_or_else(|e| panic!("read {name}.raw: {e}"));
            let expected = raw.len();
            let out = match variant.as_str() {
                "zstd0" => zstd_raw_decode(&bin, expected),
                "zstd1" | "zstd1hilo" => zstd1_decode(&bin, expected, *itemsize),
                other => panic!("unknown variant {other} for {name}"),
            }
            .unwrap_or_else(|e| panic!("decode {name} ({variant}): {e}"));
            assert_eq!(out, raw, "{name} ({variant}) decoded bytes != ground truth");
            if variant == "zstd1hilo" {
                hilo_seen += 1;
            }
        }
        assert!(hilo_seen >= 1, "no Hi/Lo fixture was exercised");
    }

    /// Minimal manifest parser: yields `(name, variant, itemsize)` per object.
    /// The manifest is generated + pretty-printed with the only nested bracket
    /// being the `shape` array (`[]`, never `{}`), so splitting on `}` gives one
    /// object per chunk. Avoids pulling in a JSON dev-dependency.
    fn parse_manifest(txt: &str) -> Vec<(String, String, usize)> {
        let mut out = Vec::new();
        for chunk in txt.split('}') {
            if let (Some(name), Some(variant), Some(itemsize)) = (
                json_str(chunk, "name"),
                json_str(chunk, "variant"),
                json_num(chunk, "itemsize"),
            ) {
                out.push((name, variant, itemsize));
            }
        }
        out
    }

    /// Extract the string value of `"key": "value"` from a manifest object chunk.
    fn json_str(chunk: &str, key: &str) -> Option<String> {
        let pat = format!("\"{key}\"");
        let after = &chunk[chunk.find(&pat)? + pat.len()..];
        let after = &after[after.find(':')? + 1..];
        let after = &after[after.find('"')? + 1..];
        Some(after[..after.find('"')?].to_string())
    }

    /// Extract the unsigned integer value of `"key": N` from a manifest chunk.
    fn json_num(chunk: &str, key: &str) -> Option<usize> {
        let pat = format!("\"{key}\"");
        let after = &chunk[chunk.find(&pat)? + pat.len()..];
        let after = after[after.find(':')? + 1..].trim_start();
        let end = after.find(|c: char| !c.is_ascii_digit()).unwrap_or(after.len());
        after[..end].parse().ok()
    }
}
