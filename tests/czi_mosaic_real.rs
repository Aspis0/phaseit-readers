// SPDX-License-Identifier: Apache-2.0
//! Real-file bit-exact parity test for the CZI reader on the owner's mosaic (`czi/mosaic.czi`).
//! The reference is `czifile`'s output, dumped by `tests/parity/dump_ref_czi.py`; the dumps are local and gitignored.
//! It certifies the geometry, stride, dtype and origin of the mosaic, not the composition order: the owner's mosaic
//! judges both orders alike. `tests/czi_jxr_real.rs` certifies the order.
//! The skip guard is the shared real-data gate in `tests/real_data/mod.rs`.

mod real_data;

use std::path::{Path, PathBuf};

use phasit_imread::czi_reader::CziReader;
use phasit_imread::types::PixelRect;
use phasit_imread::{DType, ImageReader};

use real_data::{real_file, reference_dump};

const SAMPLE: &str = "czi/mosaic.czi";

const REF_ROOT: &str = "tests/parity/_ref";

fn ref_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(REF_ROOT)
}

fn manifest() -> Option<serde_json::Value> {
    let path = ref_dir().join("czi_mosaic/manifest.json");
    reference_dump(&path)?;
    let bytes = std::fs::read(&path).expect("read the mosaic manifest");
    Some(serde_json::from_slice(&bytes).expect("parse the mosaic manifest"))
}

/// One tile of the reference manifest, in the reader's scene-relative coordinates.
struct Tile {
    mosaic_index: i64,
    dir_index: i64,
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
}

struct Window {
    name: String,
    kind: String,
    /// Scene-relative, like the tiles: `Start - origin`.
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    t: usize,
    c: usize,
    z: usize,
    tile_count: usize,
    orders_agree: bool,
    tiles: Vec<Tile>,
    npy_rel: String,
}

/// A negative coordinate means the manifest does not match the file: fail here, do not wrap into a `usize`.
fn scene_relative(value: i64, origin: i64, what: &str) -> usize {
    usize::try_from(value - origin).unwrap_or_else(|_| {
        panic!("the manifest's {what} ({value}) is not inside the scene (origin {origin})")
    })
}

/// Signed: a tile's rectangle may reach past the origin.
fn scene_relative_i64(value: i64, origin: i64, what: &str) -> i64 {
    let relative = value - origin;
    assert!(
        relative >= 0,
        "the manifest's {what} ({value}) is not inside the scene (origin {origin})"
    );
    relative
}

/// The last tile drawn over a pixel wins it, as in a mosaic composite.
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

/// Recomputed from the tile rectangles, not taken from the producer's manifest.
fn orders_agree(window: &Window) -> bool {
    let mut by_mosaic: Vec<&Tile> = window.tiles.iter().collect();
    by_mosaic.sort_by_key(|tile| (tile.mosaic_index, tile.dir_index));
    let mut by_directory: Vec<&Tile> = window.tiles.iter().collect();
    by_directory.sort_by_key(|tile| tile.dir_index);
    winner_map(&by_mosaic, window) == winner_map(&by_directory, window)
}

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

#[test]
fn czi_mosaic_real_composition() {
    let Some(manifest) = manifest() else {
        return;
    };

    // Both orders are ascending mosaic index, czifile's.
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

#[test]
fn czi_mosaic_real_order_blindness() {
    let Some(manifest) = manifest() else {
        return;
    };

    // The whole-file census is the producer's number: the manifest records only the dumped windows' tiles.
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
    // It must cover an overlap band and a single tile: only the band can show the composition order.
    assert!(
        all.iter().any(|w| w.tile_count >= 2),
        "the reference covers a window where two tiles overlap"
    );
    assert!(
        all.iter().any(|w| w.tile_count == 1),
        "the reference covers a window where one tile covers everything"
    );
    let planes: std::collections::BTreeSet<(usize, usize, usize)> =
        all.iter().map(|w| (w.t, w.c, w.z)).collect();
    assert!(
        planes.len() >= 2,
        "the reference covers at least two planes"
    );
}

/// Every window is compared and every difference is reported before the test fails.
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

/// Returns the difference as an error, never a panic, so the caller reports every window.
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
