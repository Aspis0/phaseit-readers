// SPDX-License-Identifier: Apache-2.0
//! Real-file bit-exact parity test for the CZI reader on the owner's own CZI
//! mosaic (`czi/mosaic.czi`).
//!
//! The file is the folder `PHASIT_REAL_DATA_DIR` names, read as
//! `czi/mosaic.czi` (the layout is in the crate README). It is a whole-plane
//! mosaic: every (C,Z) plane is a grid of 1024x1024 Gray16 tiles that OVERLAP
//! in a narrow band, and the mosaic index decides which tile wins in the
//! overlap.
//!
//! The reference is the `czifile` ground truth dumped by
//! `tests/parity/dump_ref_czi.py`: six 512x512 windows of two planes — one
//! across the band two side-by-side tiles share, one across the band two
//! stacked tiles share, and one inside a single tile, for each of the two
//! planes. The dumps live in `tests/parity/_ref/czi_mosaic/` (manifest.json
//! plus one `.npy` per window) and are gitignored: they are derived from a
//! local file and are generated locally.
//!
//! COMPOSITION ORDER: the reference is `czifile`'s own output — the windows
//! are read by `czifile.imread` with the plane selected, so `czifile`
//! composes them in ascending mosaic index, the tile with the higher M on
//! top (`czifile.py:1308`, `czifile.py:1309`, applied at `czifile.py:2810`
//! and `czifile.py:2811`). `phasit-imread` composites them in the same order
//! (`src/czi_reader.rs`, `CziReader::open` — the per-plane mosaic sort), so
//! the reader and the ground truth agree on every pixel.
//!
//! WHAT THIS REFERENCE CERTIFIES — AND WHAT IT DOES NOT: the owner's mosaic
//! CANNOT tell the composition order from the subblock-directory order. The
//! manifest records the census: for every overlapping pair of tiles of the
//! file, the mosaic order and the directory order pick the SAME winner (the
//! census count is zero), so the reference bytes are identical whichever of
//! the two orders a reader uses. This reference therefore certifies the
//! GEOMETRY, the STRIDE, the DTYPE and the ORIGIN of the owner's mosaic — a
//! reader that placed a tile at the wrong offset, read it with the wrong
//! dtype or lost the overlap band is caught. It does NOT certify the
//! composition ORDER. That is certified by the JPEG-XR reference
//! (`tests/czi_jxr_real.rs`, the Axioscan sample), where the two orders do
//! disagree and a window therefore goes red when the order is wrong.
//!
//! WHAT IS RECOMPUTED HERE: the manifest records the tiles of every dumped
//! window, so the test recomputes the order-blindness of each window from
//! those tile rectangles — the winner per pixel, under both orders — instead
//! of taking the producer's word for it. The CENSUS of the whole file cannot
//! be recomputed from the manifest (it records the tiles of the dumped
//! windows only): it is the producer's number, and the module doc above says
//! what it is used for.
//!
//! SKIP GUARD: the test skips cleanly when `PHASIT_REAL_DATA_DIR` is unset,
//! when the sample is absent, or when the reference dumps are absent. The
//! guard is the shared real-data gate: `PHASIT_REAL_DATA_REQUIRED=1` turns
//! every such skip into a failure (`tests/real_data/mod.rs`), so a required
//! run cannot pass without the sample and the dumps.

mod real_data;

use std::path::{Path, PathBuf};

use phasit_imread::czi_reader::CziReader;
use phasit_imread::types::PixelRect;
use phasit_imread::{DType, ImageReader};

use real_data::{real_file, reference_dump};

/// The owner's mosaic, relative to `PHASIT_REAL_DATA_DIR`.
const SAMPLE: &str = "czi/mosaic.czi";

/// The folder holding the reference dumps, relative to the crate root.
const REF_ROOT: &str = "tests/parity/_ref";

/// The folder holding the reference dumps.
fn ref_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(REF_ROOT)
}

/// The reference manifest, when the dumps are there.
fn manifest() -> Option<serde_json::Value> {
    let path = ref_dir().join("czi_mosaic/manifest.json");
    reference_dump(&path)?;
    let bytes = std::fs::read(&path).expect("read the mosaic manifest");
    Some(serde_json::from_slice(&bytes).expect("parse the mosaic manifest"))
}

/// One tile of the reference manifest.
///
/// The coordinates are the reader's SCENE-RELATIVE ones — the origin of the
/// bounding box is subtracted, like the window's.
struct Tile {
    /// The tile's mosaic index (its `M`).
    mosaic_index: i64,
    /// The tile's position in the subblock directory.
    dir_index: i64,
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
}

/// One window of the reference manifest.
struct Window {
    name: String,
    kind: String,
    /// The window in the reader's SCENE-RELATIVE coordinates (the ones the
    /// tiles are placed at: `Start - origin`).
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    /// The canonical (t, c, z) indices the reader takes.
    t: usize,
    c: usize,
    z: usize,
    /// How many mosaic tiles take part in the window.
    tile_count: usize,
    /// Whether the mosaic order and the subblock-directory order pick the
    /// same winner for every pixel of the window.
    orders_agree: bool,
    /// The tiles that take part in the window, in scene-relative coordinates.
    tiles: Vec<Tile>,
    /// The `.npy` of the window, relative to `tests/parity/_ref`.
    npy_rel: String,
}

/// A scene-relative coordinate, checked.
///
/// The manifest records a window in the CZI's own coordinates and the reader
/// takes scene-relative ones, so the origin is subtracted. A negative result
/// means the manifest does not match the file: it has to fail here, not wrap
/// around into a huge `usize`.
fn scene_relative(value: i64, origin: i64, what: &str) -> usize {
    usize::try_from(value - origin).unwrap_or_else(|_| {
        panic!("the manifest's {what} ({value}) is not inside the scene (origin {origin})")
    })
}

/// A scene-relative coordinate, signed: a tile's rectangle reaches past the
/// origin, so its edges are not all inside the scene.
fn scene_relative_i64(value: i64, origin: i64, what: &str) -> i64 {
    let relative = value - origin;
    assert!(
        relative >= 0,
        "the manifest's {what} ({value}) is not inside the scene (origin {origin})"
    );
    relative
}

/// Which tile wins each pixel of the window, for one draw order.
///
/// The tiles are drawn in the given order and the LAST one that covers a pixel
/// wins it — that is how a mosaic is composited. The map holds the directory
/// index of the winning tile, and -1 where no tile covers the pixel.
fn winner_map(tiles: &[&Tile], window: &Window) -> Vec<i64> {
    let mut map = vec![-1i64; window.w * window.h];
    for tile in tiles {
        let ix0 = window.x.max(tile.x0 as usize);
        let ix1 = (window.x + window.w).min(tile.x1 as usize);
        let iy0 = window.y.max(tile.y0 as usize);
        let iy1 = (window.y + window.h).min(tile.y1 as usize);
        for y in iy0..iy1 {
            for x in ix0..ix1 {
                map[(y - window.y) * window.w + (x - window.x)] = tile.dir_index;
            }
        }
    }
    map
}

/// Whether the two composition orders pick the same winner, per pixel.
///
/// The comparison is made from the TILE RECTANGLES the manifest records: the
/// producer's own method, recomputed here, so the test does not take the
/// producer's word for it.
fn orders_agree(window: &Window) -> bool {
    let mut by_mosaic: Vec<&Tile> = window.tiles.iter().collect();
    by_mosaic.sort_by_key(|tile| (tile.mosaic_index, tile.dir_index));
    let mut by_directory: Vec<&Tile> = window.tiles.iter().collect();
    by_directory.sort_by_key(|tile| tile.dir_index);
    winner_map(&by_mosaic, window) == winner_map(&by_directory, window)
}

/// The windows of the reference manifest, in the reader's coordinates.
fn windows(manifest: &serde_json::Value) -> Vec<Window> {
    let origin_x = manifest["scene"]["origin_x"]
        .as_i64()
        .expect("scene origin x");
    let origin_y = manifest["scene"]["origin_y"]
        .as_i64()
        .expect("scene origin y");
    manifest["regions"]
        .as_array()
        .expect("the manifest holds a regions array")
        .iter()
        .map(|spec| Window {
            name: spec["name"].as_str().expect("region name").to_string(),
            kind: spec["kind"].as_str().expect("region kind").to_string(),
            // The reader takes SCENE-RELATIVE coordinates (its tiles are placed
            // at `Start - origin`), so the recorded origin is subtracted here.
            x: scene_relative(spec["x"].as_i64().expect("region x"), origin_x, "x"),
            y: scene_relative(spec["y"].as_i64().expect("region y"), origin_y, "y"),
            w: spec["w"].as_u64().expect("region w") as usize,
            h: spec["h"].as_u64().expect("region h") as usize,
            t: spec["plane"]["t"].as_u64().expect("plane t") as usize,
            c: spec["plane"]["c"].as_u64().expect("plane c") as usize,
            z: spec["plane"]["z"].as_u64().expect("plane z") as usize,
            tile_count: spec["tile_count"].as_u64().expect("tile_count") as usize,
            orders_agree: spec["orders_agree"].as_bool().expect("region orders_agree"),
            tiles: spec["tiles_intersecting"]
                .as_array()
                .expect("region tiles_intersecting")
                .iter()
                .map(|tile| Tile {
                    mosaic_index: tile["mosaic_index"].as_i64().expect("tile mosaic_index"),
                    dir_index: tile["dir_index"].as_i64().expect("tile dir_index"),
                    x0: scene_relative_i64(tile["x0"].as_i64().expect("tile x0"), origin_x, "x0"),
                    y0: scene_relative_i64(tile["y0"].as_i64().expect("tile y0"), origin_y, "y0"),
                    x1: scene_relative_i64(tile["x1"].as_i64().expect("tile x1"), origin_x, "x1"),
                    y1: scene_relative_i64(tile["y1"].as_i64().expect("tile y1"), origin_y, "y1"),
                })
                .collect(),
            npy_rel: format!(
                "czi_mosaic/{}",
                spec["npy_path"].as_str().expect("region npy_path")
            ),
        })
        .collect()
}

/// The manifest must name the composition the reference was made with.
#[test]
fn czi_mosaic_real_composition() {
    let Some(manifest) = manifest() else {
        return;
    };

    // The manifest names the composition order the reference was made with and
    // the one the reader uses. They are the same order — both ascending mosaic
    // index, czifile's.
    let composition = &manifest["composition"];
    assert_eq!(
        composition["reference_order"].as_str(),
        Some("mosaic_index_ascending"),
        "the reference is composed in czifile's order"
    );
    assert_eq!(
        composition["reader_order"].as_str(),
        Some("mosaic_index_ascending"),
        "the reader composes in czifile's order too: the tile with the higher \
         mosaic index ends up on top"
    );
    assert_eq!(
        composition["fill_value"]["czifile"].as_u64(),
        Some(0),
        "czifile fills the pixels no tile covers with zero"
    );
    assert_eq!(
        composition["fill_value"]["phasit_imread"].as_u64(),
        Some(0),
        "the reader fills the pixels no tile covers with zero"
    );
}

/// The owner's mosaic cannot tell the composition order from the
/// subblock-directory order, and the manifest has to say so.
#[test]
fn czi_mosaic_real_order_blindness() {
    let Some(manifest) = manifest() else {
        return;
    };

    // The census of the whole file: how many overlapping pairs of tiles there
    // are, and how many of them the mosaic order and the subblock-directory
    // order judge differently. The owner's mosaic has none — that is why this
    // reference certifies the geometry and not the composition order.
    //
    // The manifest records the tiles of the DUMPED WINDOWS only, so this count
    // of the whole file cannot be recomputed from it: it is the producer's
    // number. The windows below are recomputed here, from the tile rectangles
    // the manifest does record.
    let census = &manifest["composition"]["order_census"];
    assert_eq!(
        census["pairs_with_a_different_winner"].as_u64(),
        Some(0),
        "the owner's mosaic is expected to be order-blind (its composition \
         order is certified by the JPEG-XR reference); a non-zero count means \
         the reference was made from another file — update the module doc"
    );
    eprintln!(
        "the order census of the file: {} overlapping pairs, {} of them judged \
         differently by the two orders",
        census["overlapping_pairs"]
            .as_u64()
            .expect("census overlapping_pairs"),
        census["pairs_with_a_different_winner"]
            .as_u64()
            .expect("census pairs_with_a_different_winner")
    );

    // The two orders pick the same winner in every window of this file, so no
    // window of it can tell the composition order from the directory order.
    // Nothing here may claim otherwise — and the verdict is recomputed from
    // the tile rectangles, not taken from the manifest.
    for window in &windows(&manifest) {
        let recomputed = orders_agree(window);
        assert_eq!(
            recomputed, window.orders_agree,
            "window {}: the verdict recomputed from the tile rectangles differs \
             from the manifest's",
            window.name
        );
        assert!(
            recomputed,
            "window {}: the two composition orders disagree here, so this \
             reference CAN see the order after all — update the module doc",
            window.name
        );
    }
}

/// The reference set must be complete, and it must cover the mosaic.
#[test]
fn czi_mosaic_real_reference_set() {
    let Some(manifest) = manifest() else {
        return;
    };

    let all = windows(&manifest);
    eprintln!(
        "the mosaic reference set holds {} windows: {}",
        all.len(),
        all.iter()
            .map(|w| w.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    assert!(
        !all.is_empty(),
        "the reference set holds windows to compare"
    );
    for window in &all {
        assert!(
            ref_dir().join(&window.npy_rel).exists(),
            "partial reference set: {} is missing (broken reference set)",
            window.npy_rel
        );
    }
    // The set must cover what the reader could get wrong: an overlap band (two
    // or more tiles) AND a single tile (where the order cannot matter).
    assert!(
        all.iter().any(|w| w.tile_count >= 2),
        "the reference covers a window where two tiles overlap"
    );
    assert!(
        all.iter().any(|w| w.tile_count == 1),
        "the reference covers a window where one tile covers everything"
    );
    // ... and more than one plane, so the test is not about a single tile set.
    let planes: std::collections::BTreeSet<(usize, usize, usize)> =
        all.iter().map(|w| (w.t, w.c, w.z)).collect();
    assert!(
        planes.len() >= 2,
        "the reference covers at least two planes"
    );
}

/// Every reference window must be bit-exact against the `czifile` dump.
///
/// Every window is compared and EVERY difference is reported before the test
/// fails: the first mismatch does not hide the ones after it, and a window
/// that cannot be decoded or does not have the expected shape is reported
/// like a difference, not like a crash.
#[cfg(feature = "ndarray")]
#[test]
fn czi_mosaic_real_windows_bit_exact() {
    let Some(path) = real_file(SAMPLE) else {
        return;
    };
    let Some(manifest) = manifest() else {
        return;
    };

    let reader = CziReader::open(&path).expect("open the owner's mosaic CZI");
    assert_eq!(reader.meta().scene_count(), 1, "the mosaic holds one scene");
    let scene = reader.meta().scene(0).expect("scene 0");
    assert_eq!(scene.dtype, DType::U16, "dtype");
    assert_eq!(scene.samples, 1, "samples per pixel");
    assert!(reader.supports_region(), "the CZI reader streams regions");

    let all = windows(&manifest);
    let mut differences = Vec::new();
    for window in &all {
        match compare_window(&reader, window) {
            Ok(summary) => eprintln!("window {} ({}): {summary}", window.name, window.kind),
            Err(detail) => {
                eprintln!("window {}: {detail}", window.name);
                differences.push(format!("{}: {detail}", window.name));
            }
        }
    }

    assert!(
        differences.is_empty(),
        "{} of {} windows are not bit-exact vs the czifile reference:\n  {}",
        differences.len(),
        all.len(),
        differences.join("\n  ")
    );
}

/// Compare one window with the reference and describe the difference, if any.
///
/// Every failure is a `Result`, never a panic: the caller compares every
/// window and reports them all, so one broken window does not hide the ones
/// after it.
#[cfg(feature = "ndarray")]
fn compare_window(reader: &CziReader, window: &Window) -> Result<String, String> {
    use ndarray::Array2;
    use phasit_imread::PlaneBuf;

    let expected: Array2<u16> = match ndarray_npy::read_npy(ref_dir().join(&window.npy_rel)) {
        Ok(array) => array,
        Err(e) => return Err(format!("read the reference .npy: {e}")),
    };
    if [expected.shape()[0], expected.shape()[1]] != [window.h, window.w] {
        return Err(format!(
            "the reference is {:?} but the window is {}x{}",
            expected.shape(),
            window.w,
            window.h
        ));
    }

    let plane = match reader.region(
        0,
        window.t,
        window.c,
        window.z,
        PixelRect::new(window.x, window.y, window.w, window.h),
    ) {
        Ok(plane) => plane,
        Err(e) => return Err(format!("decode the window: {e}")),
    };
    if plane.shape != [window.h, window.w] {
        return Err(format!(
            "the decoded window is {:?} but the reference is {}x{}",
            plane.shape, window.w, window.h
        ));
    }
    if plane.samples != 1 {
        return Err(format!(
            "the decoded window holds {} samples, the reference holds 1",
            plane.samples
        ));
    }
    let ours = match &plane.data {
        PlaneBuf::U16(v) => v,
        other => return Err(format!("expected U16 data, got {:?}", other.dtype())),
    };
    let reference: Vec<u16> = expected.iter().copied().collect();
    if ours == &reference {
        return Ok(format!(
            "equal ({} samples, {} tile(s))",
            ours.len(),
            window.tile_count
        ));
    }
    let differing = ours
        .iter()
        .zip(reference.iter())
        .filter(|(a, b)| a != b)
        .count();
    Err(format!("{differing} of {} samples differ", ours.len()))
}
