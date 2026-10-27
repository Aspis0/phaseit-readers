// SPDX-License-Identifier: Apache-2.0
//! The reference set of the JPEG-XR CZI parity: the `czifile` ground truth
//! dumped by `tests/parity/dump_ref_czi_jxr.py`.
//!
//! The dumps live in `tests/parity/_ref/czi_jxr/` (a `manifest.json` and one
//! `.npy` per region) and are gitignored: they are derived from a third-party
//! sample and are generated locally. The manifest names, per region, the
//! window, the mosaic tiles that take part in it, the order the region was
//! ACTUALLY composited in, and the two composition orders — the one `czifile`
//! uses (ascending mosaic index) and the one `phasit-imread` uses (the same:
//! ascending mosaic index). The two orders are
//! recorded side by side because the SUBBLOCK-DIRECTORY order is not the same
//! order as the mosaic order in every file, and a region where they disagree
//! is a region where the composition order decides the bytes.
//!
//! A failure names the `.npy` relative to `tests/parity/_ref`, never the
//! machine's directory layout: the reasons travel into shared logs.
//!
//! COORDINATES: the manifest records a window in the CZI's own coordinates and
//! the origin of the bounding box beside it; the reader takes scene-relative
//! coordinates, so `regions` subtracts the recorded origin.

use std::path::{Path, PathBuf};

use crate::real_data::reference_dump;

/// The folder holding the reference dumps, relative to the crate root.
pub const REF_ROOT: &str = "tests/parity/_ref";

/// The variable that selects a subset of the reference regions.
///
/// It names the regions to compare, comma separated, for a targeted run. Unset
/// — the default — compares every region.
#[cfg(all(feature = "ndarray", feature = "jpegxr"))]
pub const REGION_FILTER_VAR: &str = "PHASIT_CZI_JXR_REGIONS";

/// The folder holding the reference dumps.
pub fn ref_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(REF_ROOT)
}

/// The reference manifest, when the dumps are there.
pub fn manifest() -> Option<serde_json::Value> {
    let path = ref_dir().join("czi_jxr/manifest.json");
    reference_dump(&path)?;
    let bytes = std::fs::read(&path).expect("read the region manifest");
    Some(serde_json::from_slice(&bytes).expect("parse the region manifest"))
}

/// One region of the reference manifest.
#[derive(Clone)]
pub struct Region {
    pub name: String,
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    /// The `.npy` of the region, relative to `tests/parity/_ref`. A failure
    /// names this, never the machine's directory layout.
    pub npy_rel: String,
    /// How many mosaic tiles take part in the region.
    pub tile_count: usize,
    /// The order the region was ACTUALLY composited in, as the producer
    /// recorded it.
    pub composition_order: String,
    /// Whether the two composition orders pick the same tiles in the same order.
    pub orders_agree: bool,
    /// How many pixels of the region some tile covers.
    pub covered_pixels: usize,
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

/// The regions of the reference manifest.
///
/// The manifest records a window in the CZI's OWN coordinates — the ones the
/// tiles carry in `X.Start`/`Y.Start` — and the origin of the bounding box
/// beside it (`scene.origin_x`, `scene.origin_y`). The reader takes the
/// SCENE-RELATIVE coordinates (its tiles are placed at `Start - origin`, in
/// `src/czi_reader.rs` — `CziReader::plane` and `CziReader::region`), so the
/// origin is subtracted here. A wrong origin moves the window, and a moved
/// window decodes to different bytes.
pub fn regions(manifest: &serde_json::Value) -> Vec<Region> {
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
        .map(|spec| Region {
            name: spec["name"].as_str().expect("region name").to_string(),
            x: scene_relative(spec["x"].as_i64().expect("region x"), origin_x, "x"),
            y: scene_relative(spec["y"].as_i64().expect("region y"), origin_y, "y"),
            w: spec["w"].as_u64().expect("region w") as usize,
            h: spec["h"].as_u64().expect("region h") as usize,
            npy_rel: format!(
                "czi_jxr/{}",
                spec["npy_path"].as_str().expect("region npy_path")
            ),
            tile_count: spec["tile_count"].as_u64().expect("region tile_count") as usize,
            composition_order: spec["composition_order"]
                .as_str()
                .expect("region composition_order")
                .to_string(),
            orders_agree: spec["orders_agree"].as_bool().expect("region orders_agree"),
            covered_pixels: spec["covered_pixels"]
                .as_u64()
                .expect("region covered_pixels") as usize,
        })
        .collect()
}

/// The regions to compare: all of them, unless the run names a subset.
#[cfg(all(feature = "ndarray", feature = "jpegxr"))]
pub fn selected_regions(manifest: &serde_json::Value) -> Vec<Region> {
    let all = regions(manifest);
    let Some(filter) = std::env::var_os(REGION_FILTER_VAR) else {
        return all;
    };
    let wanted: Vec<String> = filter
        .to_string_lossy()
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();
    let picked: Vec<Region> = all
        .iter()
        .filter(|region| wanted.iter().any(|name| *name == region.name))
        .cloned()
        .collect();
    assert_eq!(
        picked.len(),
        wanted.len(),
        "the filter names a region the manifest does not hold"
    );
    eprintln!("comparing {} of {} regions", picked.len(), all.len());
    picked
}
