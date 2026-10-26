// SPDX-License-Identifier: Apache-2.0
//! Real-file bit-exact parity test for the LIF reader.
//!
//! Uses real Leica confocal `.lif` files, read from the folder named by
//! `PHASIT_REAL_DATA_DIR` (the layout is in the crate README), and the
//! `liffile` `_ref` dumps (`tests/parity/_ref/manifest_lif.json` + `*.npy`).
//! Skips cleanly (early return) when the variable is unset, or the samples or
//! the manifest are absent, so CI without the files stays green — but when they
//! ARE present the `checked >= 48` assertion guarantees a silent skip cannot
//! pass. The harness dumps planes from EVERY image in both files (not just
//! image[0]), so this covers both the C=3 (`Series001`) and
//! C=2 (`Series003/004/005`) channel layouts.
//!
//! The `Series001_pmd_0` manifest entry is deliberately NOT decoded: its pixels
//! live in the `.lifext` companion, so the reader (which reads the `.lif` only)
//! does not expose it as a scene. Every other reference plane must be bit-exact.

mod real_data;

use phasit_imread::lif_reader::LifReader;
use phasit_imread::{DType, Dims5, Error, ImageReader};

use real_data::real_file;

// Only referenced by the bit-exact parity test (ndarray-gated).
#[cfg(feature = "ndarray")]
const SAMPLE_CTRL: &str = "lif/control.lif";
const SAMPLE_MULTI: &str = "lif/four-scene.lif";

#[test]
fn lif_geometry_and_out_of_range() {
    let Some(path) = real_file(SAMPLE_MULTI) else {
        return;
    };

    let reader = LifReader::open(&path).expect("open LIF");
    let meta = reader.meta();
    // Series001 (3ch) + Series003/004/005 (2ch); pmd_0 lives in the .lifext.
    assert_eq!(meta.scene_count(), 4, "the sample has 4 in-.lif images");

    // Match Series001 by name (scene order follows document order).
    let (idx, s001) = meta
        .scenes
        .iter()
        .enumerate()
        .find(|(_, s)| s.name == "Series001")
        .expect("Series001 present");
    assert_eq!(
        s001.dims,
        Dims5 { t: 1, c: 3, z: 59, y: 1024, x: 1024 },
        "Series001 geometry"
    );
    assert_eq!(s001.dtype, DType::U8);
    assert_eq!(s001.channels.len(), 3);

    // A valid plane has the right shape/dtype.
    let p = reader.plane(idx, 0, 0, 0).expect("plane t0 c0 z0");
    assert_eq!(p.shape, [1024, 1024]);
    assert_eq!(p.data.len(), 1024 * 1024);
    assert_eq!(p.dtype(), DType::U8);

    // Out-of-range coordinates must be clean errors, never panics.
    match reader.plane(idx, 0, 0, 59) {
        Err(Error::OutOfRange { z: 59, .. }) => {}
        other => panic!("expected OutOfRange for z=59, got {other:?}"),
    }
    match reader.plane(idx, 0, 3, 0) {
        Err(Error::OutOfRange { c: 3, .. }) => {}
        other => panic!("expected OutOfRange for c=3, got {other:?}"),
    }
    match reader.plane(idx, 1, 0, 0) {
        Err(Error::OutOfRange { t: 1, .. }) => {}
        other => panic!("expected OutOfRange for t=1, got {other:?}"),
    }
    assert!(matches!(
        reader.plane(99, 0, 0, 0),
        Err(Error::SceneOutOfRange { scene: 99, .. })
    ));
}

/// Bit-exact parity: every non-ext LIF `_ref` plane must equal our decoded
/// plane, element-for-element. This is the real gate on the DimID→TCZYX mapping,
/// the channel ordering, and the per-plane byte-offset arithmetic.
#[cfg(feature = "ndarray")]
#[test]
fn lif_planes_bit_exact_vs_liffile_reference() {
    use std::path::Path;

    use phasit_imread::PlaneBuf;

    let ref_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/parity/_ref");
    let manifest_path = ref_dir.join("manifest_lif.json");
    let Some(ctrl) = real_file(SAMPLE_CTRL) else {
        return;
    };
    let Some(multi) = real_file(SAMPLE_MULTI) else {
        return;
    };
    if !manifest_path.exists() {
        eprintln!("skipping bit-exact parity: manifest_lif.json absent");
        return;
    }

    let manifest = std::fs::read_to_string(&manifest_path).expect("read manifest_lif.json");

    // Open each sample once and reuse across its planes.
    let reader_ctrl = LifReader::open(&ctrl).expect("open the control LIF");
    let reader_multi = LifReader::open(&multi).expect("open the four-scene LIF");

    let mut checked = 0usize;
    for obj in extract_objects(&manifest) {
        let image_name = json_str_field(&obj, "image_name").expect("image_name field");
        // The pmd_0 image is ext-resident: our reader never exposes it. Skip.
        if image_name.ends_with("_pmd_0") {
            continue;
        }
        let sample = json_str_field(&obj, "sample").expect("sample field");
        // The manifest names the sample by its file name without the extension.
        let reader = match sample.as_str() {
            "control" => &reader_ctrl,
            "four-scene" => &reader_multi,
            other => panic!("unexpected sample {other:?} in manifest"),
        };

        // Reference plane_index carries Z and C; T is absent (single time point).
        let z = json_int_field(&obj, "Z").expect("Z index") as usize;
        let c = json_int_field(&obj, "C").expect("C index") as usize;
        let shape = json_shape(&obj).expect("plane_shape field");
        let npy_rel = json_str_field(&obj, "npy_path").expect("npy_path field");

        // Find the scene whose name matches; the reader must expose it.
        let scene_idx = reader
            .meta()
            .scenes
            .iter()
            .position(|s| s.name == image_name)
            .unwrap_or_else(|| panic!("scene {image_name:?} not found in reader"));

        let npy_path = ref_dir.join(&npy_rel);
        // A manifest entry whose .npy is missing means a PARTIAL reference set: a
        // broken dev env, not a legitimate skip. Hard-fail so `checked >= 11`
        // cannot be dodged by a half-present `_ref` dir.
        assert!(
            npy_path.exists(),
            "partial LIF _ref: {} is missing (broken reference set)",
            npy_path.display()
        );

        let arr: ndarray::Array2<u8> =
            ndarray_npy::read_npy(&npy_path).expect("read reference .npy");
        assert_eq!(
            [arr.shape()[0], arr.shape()[1]],
            shape,
            "reference npy shape disagrees with manifest for {image_name} Z{z} C{c}"
        );

        let plane = reader.plane(scene_idx, 0, c, z).expect("decode LIF plane");
        assert_eq!(
            plane.shape,
            [shape[0], shape[1]],
            "our plane shape ({image_name} Z{z} C{c})"
        );
        let ours = match &plane.data {
            PlaneBuf::U8(v) => v,
            other => panic!("expected U8 plane for LIF, got {:?}", other.dtype()),
        };
        let reference: Vec<u8> = arr.iter().copied().collect();
        assert_eq!(ours.len(), reference.len(), "plane length ({image_name} Z{z} C{c})");
        assert_eq!(
            ours, &reference,
            "LIF {image_name} Z{z} C{c} is NOT bit-exact vs the liffile reference (mapping bug?)"
        );
        checked += 1;
    }

    assert!(
        checked >= 48,
        "expected the 48 non-ext LIF reference planes, only matched {checked}"
    );
    eprintln!("bit-exact parity: matched {checked} LIF planes element-for-element");
}

// --- minimal JSON helpers for the pretty-printed parity manifest -------------
// Mirrors the helpers in `lsm_real.rs`; a targeted scan avoids a JSON dep.

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
