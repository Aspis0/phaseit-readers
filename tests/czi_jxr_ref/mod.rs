// SPDX-License-Identifier: Apache-2.0
//! The reference set of the JPEG-XR CZI parity: `czifile`'s ground truth, dumped by `tests/parity/dump_ref_czi_jxr.py`
//! into the gitignored `tests/parity/_ref/czi_jxr/` (manifest.json plus one `.npy` per region), and read here.
//! The two composition orders are recorded side by side: they disagree in some regions, and there the order decides
//! the bytes.
//! Failures name the `.npy` relative to `tests/parity/_ref`, never the machine's layout, because the reasons go into
//! shared logs.
//! The manifest records window origins in the CZI's own coordinates; `regions` subtracts the recorded origin.

use std::path::{Path, PathBuf};

use crate::real_data::reference_dump;

pub const REF_ROOT: &str = "tests/parity/_ref";

/// Comma-separated region names select a subset for a targeted run; unset compares every region.
#[cfg(all(feature = "ndarray", feature = "jpegxr"))]
pub const REGION_FILTER_VAR: &str = "PHASIT_CZI_JXR_REGIONS";

pub fn ref_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(REF_ROOT)
}

pub fn manifest() -> Option<serde_json::Value> {
    let path = ref_dir().join("czi_jxr/manifest.json");
    reference_dump(&path)?;
    let bytes = std::fs::read(&path).expect("read the region manifest");
    Some(serde_json::from_slice(&bytes).expect("parse the region manifest"))
}

#[derive(Clone)]
pub struct Region {
    pub name: String,
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    pub npy_rel: String,
    pub tile_count: usize,
    pub composition_order: String,
    pub orders_agree: bool,
    pub covered_pixels: usize,
}

/// A negative result means the manifest does not match the file: fail, do not wrap into a `usize`.
fn scene_relative(value: i64, origin: i64, what: &str) -> usize {
    usize::try_from(value - origin).unwrap_or_else(|_| {
        panic!("the manifest's {what} ({value}) is not inside the scene (origin {origin})")
    })
}

/// A wrong origin moves the window, and a moved window decodes to different bytes.
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
