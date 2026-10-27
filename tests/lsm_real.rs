// SPDX-License-Identifier: Apache-2.0
//! Real-file bit-exact parity test for the LSM reader.
//!
//! Uses a real Zeiss LSM confocal stack, read from the folder named by
//! `PHASIT_REAL_DATA_DIR` (the layout is in the crate README), and the `_ref`
//! dumps: the tifffile ones for the stack, the Bio-Formats one for the tile
//! scan. Skips cleanly (early return) when the variable is unset, or
//! the sample or reference manifest is absent, so CI without the file stays
//! green — unless the run sets `PHASIT_REAL_DATA_REQUIRED=1`, which turns
//! every such skip into a failure. When they ARE present the `checked >= 8`
//! assertion guarantees a silent skip cannot pass.
//!
//! The tile scan's reference hashes are NOT committed: a hash of a private
//! acquisition file is a fingerprint of it. They live in a gitignored dump
//! (`tests/parity/_ref/tilescan/manifest.json`), made by
//! `tests/parity/dump_ref_lsm.py` from Bio-Formats' own output.

mod real_data;

use std::path::{Path, PathBuf};

use phasit_imread::lsm_reader::LsmReader;
use phasit_imread::{DType, Dims5, Error, ImageReader, PlaneBuf};

use real_data::{real_file, reference_dump};

// The CLI's own SHA-256, so a plane is hashed exactly the way the `imread
// json` output hashes it — the same bytes Bio-Formats' hashes were taken of.
#[path = "../src/bin/imread/sha256.rs"]
mod sha256;

const SAMPLE: &str = "lsm/stack.lsm";
const TILE_SCAN: &str = "lsm/tilescan.lsm";

/// The folder holding the reference dumps.
fn ref_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/parity/_ref")
}

/// Bio-Formats' reference of the tile scan's planes, when the dump is there.
///
/// The dump is gitignored: the hashes of a private acquisition file are a
/// fingerprint of it, so they must not live in the repository. `reference_dump`
/// turns a missing dump into a skip — or into a failure when the run demands
/// the real data — so a required run cannot pass without it.
fn tile_scan_reference() -> Option<serde_json::Value> {
    let path = ref_dir().join("tilescan/manifest.json");
    reference_dump(&path)?;
    let bytes = std::fs::read(&path).expect("read the tile-scan reference manifest");
    Some(serde_json::from_slice(&bytes).expect("parse the tile-scan reference manifest"))
}

/// The reference hash of one series' plane.
fn reference_sha256(reference: &serde_json::Value, series: usize) -> String {
    reference["planes"]
        .as_array()
        .expect("the reference manifest holds a planes array")
        .iter()
        .find(|plane| plane["series"].as_u64() == Some(series as u64))
        .unwrap_or_else(|| panic!("the reference has no plane for series {series}"))["sha256"]
        .as_str()
        .expect("the reference plane states a sha256")
        .to_string()
}

#[test]
fn lsm_geometry_and_out_of_range() {
    let Some(path) = real_file(SAMPLE) else {
        return;
    };

    let reader = LsmReader::open(&path).expect("open LSM");
    let meta = reader.meta();
    assert_eq!(meta.scene_count(), 1, "LSM is single-scene");

    let scene = meta.scene(0).unwrap();
    assert_eq!(
        scene.dims,
        Dims5 { t: 1, c: 2, z: 35, y: 1024, x: 1024 },
        "LSM = 2 channels, 35 Z, one time point"
    );
    assert_eq!(scene.dtype, DType::U16);
    assert_eq!(scene.channels.len(), 2);

    // Calibration from CZ-LSM-Info (best-effort): ~0.0659 µm XY, 0.5 µm Z.
    let px = scene.px_size_um.expect("LSM has voxel calibration");
    assert!((px.z - 0.5).abs() < 1e-3, "z px = {}", px.z);
    assert!((px.x - 0.0659).abs() < 1e-3, "x px = {}", px.x);

    // A valid plane has the right shape and dtype.
    let p = reader.plane(0, 0, 0, 0).expect("plane c0 z0");
    assert_eq!(p.shape, [1024, 1024]);
    assert_eq!(p.data.len(), 1024 * 1024);
    assert_eq!(p.dtype(), DType::U16);

    // Out-of-range coordinates must be clean OutOfRange, never panics.
    match reader.plane(0, 0, 0, 35) {
        Err(Error::OutOfRange { scene: 0, t: 0, c: 0, z: 35 }) => {}
        other => panic!("expected OutOfRange for z=35, got {other:?}"),
    }
    match reader.plane(0, 0, 2, 0) {
        Err(Error::OutOfRange { scene: 0, t: 0, c: 2, z: 0 }) => {}
        other => panic!("expected OutOfRange for c=2, got {other:?}"),
    }
    assert!(matches!(
        reader.plane(1, 0, 0, 0),
        Err(Error::OutOfRange { scene: 1, .. })
    ));
}

/// A mosaic shot: the file holds 49 tiles, one page each, and the reader
/// publishes one scene per tile. The scene count, the geometry and the
/// channel name must match Bio-Formats, and a scene's plane must be
/// bit-exact with the Bio-Formats series of the same index.
#[test]
fn lsm_tile_scan_publishes_one_scene_per_tile() {
    let Some(path) = real_file(TILE_SCAN) else {
        return;
    };
    let Some(reference) = tile_scan_reference() else {
        return;
    };

    let reader = LsmReader::open(&path).expect("open the tile scan");
    let meta = reader.meta();
    assert_eq!(meta.scene_count(), 49, "one scene per tile");

    let scene = meta.scene(0).unwrap();
    assert_eq!(
        scene.dims,
        Dims5 { t: 1, c: 1, z: 1, y: 256, x: 256 },
        "a tile is one page of one channel"
    );
    assert_eq!(scene.dtype, DType::U8);
    assert_eq!(scene.channels.len(), 1);
    assert_eq!(
        scene.channels[0].name.as_deref(),
        Some("Ch2 GaAsP"),
        "the channel name Bio-Formats publishes"
    );

    // The scene names carry Bio-Formats' suffix.
    assert_eq!(meta.scene(0).unwrap().name, "tilescan #1");
    assert_eq!(meta.scene(48).unwrap().name, "tilescan #49");

    // The planes of the first and last scene, bit-exact with the hashes
    // Bio-Formats reported for its series 0 and 48.
    for scene_index in [0usize, 48] {
        let plane = reader
            .plane(scene_index, 0, 0, 0)
            .expect("the scene's plane");
        let PlaneBuf::U8(bytes) = &plane.data else {
            panic!("expected a U8 plane");
        };
        assert_eq!(bytes.len(), 256 * 256);
        assert_eq!(
            sha256::sha256_hex(bytes),
            reference_sha256(&reference, scene_index),
            "scene {scene_index} is not bit-exact vs Bio-Formats"
        );
    }

    // A scene outside the file is out of range, never a panic.
    assert!(matches!(
        reader.plane(49, 0, 0, 0),
        Err(Error::OutOfRange { scene: 49, .. })
    ));
}

/// Bit-exact parity: every LSM `_ref` plane must equal our decoded plane,
/// element-for-element. This is the real gate on the page→(t,c,z) mapping and
/// the manual strip reader.
#[cfg(feature = "ndarray")]
#[test]
fn lsm_planes_bit_exact_vs_tifffile_reference() {
    let Some(sample) = real_file(SAMPLE) else {
        return;
    };
    let ref_root = ref_dir();
    let manifest_path = ref_root.join("manifest.json");
    if reference_dump(&manifest_path).is_none() {
        return;
    }

    let manifest = std::fs::read_to_string(&manifest_path).expect("read manifest.json");
    let reader = LsmReader::open(&sample).expect("open LSM");
    // The manifest holds every sample it was dumped from; this sample's planes
    // live under a folder named after its file name without the extension.
    let stem = Path::new(SAMPLE)
        .file_stem()
        .expect("the sample has a file name")
        .to_string_lossy()
        .into_owned();

    let mut checked = 0usize;
    for obj in extract_objects(&manifest) {
        // LSM reference entries carry Z and C in their nested plane_index; the
        // The sample-offset "S" entries have no Z and are skipped here.
        let (z, c) = match (json_int_field(&obj, "Z"), json_int_field(&obj, "C")) {
            (Some(z), Some(c)) => (z as usize, c as usize),
            _ => continue,
        };
        let npy_rel = json_str_field(&obj, "npy_path").expect("npy_path field");
        if !npy_rel.starts_with(&format!("{stem}/")) {
            continue; // belongs to a different sample
        }
        let shape = json_shape(&obj).expect("plane_shape field");

        let npy_path = ref_root.join(&npy_rel);
        // A manifest entry whose .npy is missing means a PARTIAL reference set: a
        // broken dev env, not a legitimate skip. Hard-fail so `checked >= 8` can
        // never be dodged by a half-present `_ref` dir. (The whole-manifest-absent
        // case is handled by the early return above.) The message names the dump
        // by its path under `tests/parity/_ref`, never by the absolute path of
        // the build machine.
        assert!(
            npy_path.exists(),
            "partial LSM _ref: {npy_rel} is missing (broken reference set)"
        );

        let arr: ndarray::Array2<u16> =
            ndarray_npy::read_npy(&npy_path).expect("read reference .npy");
        assert_eq!(
            [arr.shape()[0], arr.shape()[1]],
            shape,
            "reference npy shape disagrees with manifest for Z{z} C{c}"
        );

        let plane = reader.plane(0, 0, c, z).expect("decode LSM plane");
        assert_eq!(plane.shape, [shape[0], shape[1]], "our plane shape (Z{z} C{c})");
        let ours = match &plane.data {
            PlaneBuf::U16(v) => v,
            other => panic!("expected U16 plane for LSM, got {:?}", other.dtype()),
        };
        let reference: Vec<u16> = arr.iter().copied().collect();
        assert_eq!(ours.len(), reference.len(), "plane length (Z{z} C{c})");
        assert_eq!(
            ours, &reference,
            "LSM Z{z} C{c} is NOT bit-exact vs the tifffile reference (mapping bug?)"
        );
        checked += 1;
    }

    assert!(
        checked >= 8,
        "expected the 8 LSM reference planes, only matched {checked}"
    );
    eprintln!("bit-exact parity: matched {checked} LSM planes element-for-element");
}

// --- minimal JSON helpers for the pretty-printed parity manifest -------------
// Mirrors the helpers in `tiff_real.rs`; a targeted scan avoids a JSON dep.

#[cfg(feature = "ndarray")]
fn extract_objects(s: &str) -> Vec<String> {
    let mut objs = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, ch) in s.char_indices() {
        match ch {
            '{' => {
                if depth == 0 {
                    start = i;
                }
                depth += 1;
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    objs.push(s[start..=i].to_string());
                }
            }
            _ => {}
        }
    }
    objs
}

#[cfg(feature = "ndarray")]
fn json_str_field(obj: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let ki = obj.find(&pat)?;
    let rest = &obj[ki + pat.len()..];
    let colon = rest.find(':')?;
    let rest = &rest[colon + 1..];
    let q1 = rest.find('"')?;
    let rest = &rest[q1 + 1..];
    let q2 = rest.find('"')?;
    Some(rest[..q2].to_string())
}

#[cfg(feature = "ndarray")]
fn json_int_field(obj: &str, key: &str) -> Option<i64> {
    let pat = format!("\"{key}\"");
    let ki = obj.find(&pat)?;
    let rest = &obj[ki + pat.len()..];
    let colon = rest.find(':')?;
    let rest = rest[colon + 1..].trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '-')
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

#[cfg(feature = "ndarray")]
fn json_shape(obj: &str) -> Option<[usize; 2]> {
    let ki = obj.find("\"plane_shape\"")?;
    let after = &obj[ki..];
    let lb = after.find('[')?;
    let rb = after[lb..].find(']')? + lb;
    let inner = &after[lb + 1..rb];
    let nums: Vec<usize> = inner
        .split(',')
        .filter_map(|x| x.trim().parse().ok())
        .collect();
    if nums.len() == 2 {
        Some([nums[0], nums[1]])
    } else {
        None
    }
}
