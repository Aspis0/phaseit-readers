// SPDX-License-Identifier: Apache-2.0
//! Real-file smoke / parity test for the TIFF reader.
//!
//! Uses a real RGB confocal TIFF, read from the folder named by
//! `PHASIT_REAL_DATA_DIR` (the layout is in the crate README). Skips cleanly
//! (early return) when the variable is unset or the sample is absent, so CI
//! without the file stays green.

mod real_data;

use phasit_imread::tiff_reader::TiffReader;
use phasit_imread::{DType, Dims5, Error, ImageReader, PlaneBuf};

use real_data::real_file;

const SAMPLE: &str = "tiff/rgb.tif";

#[test]
fn rgb_tiff_geometry_and_planes() {
    let Some(path) = real_file(SAMPLE) else {
        return;
    };

    let reader = TiffReader::open(&path).expect("open the RGB TIFF");
    let meta = reader.meta();
    assert_eq!(meta.scene_count(), 1, "TIFF is single-scene");

    let scene = meta.scene(0).unwrap();
    assert_eq!(
        scene.dims,
        Dims5 { t: 1, c: 1, z: 1, y: 712, x: 1023 },
        "the sample is one RGB channel of three components, one page"
    );
    assert_eq!(scene.dtype, DType::U8);
    assert_eq!(scene.samples, 3, "the samples of a pixel stay together");
    assert_eq!(scene.channels.len(), 1);

    // The plane carries all three components, interleaved per pixel.
    let plane = reader.plane(0, 0, 0, 0).expect("plane");
    assert_eq!(plane.shape, [712, 1023]);
    assert_eq!(plane.samples, 3);
    assert_eq!(plane.data.len(), 712 * 1023 * 3);

    // Cheap sanity from the Python reference: S0 and S2 are byte-identical,
    // and component 1 differs from component 0. NOTE: this pair is symmetric
    // under an R/B swap, so it is NOT sufficient to prove the component
    // order -- the bit-exact check below
    // (`rgb_planes_bit_exact_vs_tifffile_reference`) is the real gate.
    let (s0, s1, s2) = components(&plane.data);
    assert_eq!(s0, s2, "R (S0) == B (S2)");
    assert_ne!(s0, s1, "R (S0) != G (S1)");

    // Out-of-range channel must be a clean OutOfRange, not a panic.
    match reader.plane(0, 0, 1, 0) {
        Err(Error::OutOfRange { scene: 0, t: 0, c: 1, z: 0 }) => {}
        other => panic!("expected OutOfRange for c=1, got {other:?}"),
    }
    // Out-of-range scene too.
    assert!(matches!(
        reader.plane(1, 0, 0, 0),
        Err(Error::OutOfRange { scene: 1, .. })
    ));
}

/// The three components of an interleaved 3-sample plane.
fn components(data: &PlaneBuf) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let PlaneBuf::U8(values) = data else {
        panic!("expected U8 plane");
    };
    let mut s0 = Vec::new();
    let mut s1 = Vec::new();
    let mut s2 = Vec::new();
    for pixel in values.chunks(3) {
        s0.push(pixel[0]);
        s1.push(pixel[1]);
        s2.push(pixel[2]);
    }
    (s0, s1, s2)
}

/// Bit-exact parity: every component of the RGB plane must equal the
/// tifffile reference `.npy` dumped in `tests/parity/_ref/`, element-for-element.
/// This is the real component-order gate (a `sample = S-1-c` BGR bug would
/// fail here even though the cheap R==B/R!=G check above would still pass).
///
/// Gated on the `ndarray` feature (needed for `ndarray-npy`); skips cleanly when
/// the sample or reference dump is absent so CI without the refs stays green.
#[cfg(feature = "ndarray")]
#[test]
fn rgb_planes_bit_exact_vs_tifffile_reference() {
    use std::path::Path;

    use phasit_imread::PlaneBuf;

    let Some(sample) = real_file(SAMPLE) else {
        return;
    };
    let ref_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/parity/_ref");
    let manifest_path = ref_dir.join("manifest.json");
    if !manifest_path.exists() {
        eprintln!("skipping bit-exact parity: manifest absent");
        return;
    }

    let manifest = std::fs::read_to_string(&manifest_path).expect("read manifest.json");
    let reader = TiffReader::open(&sample).expect("open the RGB TIFF");
    // The manifest holds every sample it was dumped from; this sample's planes
    // live under a folder named after its file name without the extension.
    let stem = Path::new(SAMPLE)
        .file_stem()
        .expect("the sample has a file name")
        .to_string_lossy()
        .into_owned();

    let mut checked = 0usize;
    for obj in extract_objects(&manifest) {
        // RGB reference entries are indexed by sample offset "S"; the LSM
        // entries use Z/C and are skipped here.
        let s = match json_int_field(&obj, "S") {
            Some(v) => v as usize,
            None => continue,
        };
        let npy_rel = json_str_field(&obj, "npy_path").expect("npy_path field");
        if !npy_rel.starts_with(&format!("{stem}/")) {
            continue; // belongs to a different sample
        }
        let shape = json_shape(&obj).expect("plane_shape field");

        let npy_path = ref_dir.join(&npy_rel);
        if !npy_path.exists() {
            eprintln!("skipping bit-exact parity: {} absent", npy_path.display());
            return;
        }

        let arr: ndarray::Array2<u8> =
            ndarray_npy::read_npy(&npy_path).expect("read reference .npy");
        assert_eq!(
            [arr.shape()[0], arr.shape()[1]],
            shape,
            "reference npy shape disagrees with manifest for S{s}"
        );

        // The reader publishes ONE plane of three interleaved components; the
        // reference holds one component per file, so the plane is split here.
        let plane = reader.plane(0, 0, 0, 0).expect("decode plane");
        assert_eq!(plane.shape, [shape[0], shape[1]], "our plane shape (S{s})");
        let PlaneBuf::U8(values) = &plane.data else {
            panic!("expected U8 plane for the RGB sample");
        };
        let ours: Vec<u8> = values.iter().skip(s).step_by(3).copied().collect();
        // ndarray `iter()` yields logical row-major order, matching our buffer.
        let reference: Vec<u8> = arr.iter().copied().collect();
        assert_eq!(ours.len(), reference.len(), "plane length (S{s})");
        assert_eq!(
            ours, reference,
            "RGB S{s} is NOT bit-exact vs the tifffile reference (component order bug?)"
        );
        checked += 1;
    }

    assert!(
        checked >= 3,
        "expected the 3 RGB reference planes, only matched {checked}"
    );
    eprintln!("bit-exact parity: matched {checked} RGB planes element-for-element");
}

// --- minimal JSON helpers for the pretty-printed parity manifest -------------
// The manifest is machine-generated and stable; a targeted scan avoids pulling
// in a JSON dependency just for the test.

/// Split a top-level JSON array of objects into per-object substrings by brace
/// depth (handles the nested `plane_index` object).
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

/// Extract a string field `"key": "value"` from one object substring.
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

/// Extract an integer field `"key": N` from one object substring.
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

/// Extract the two-element `"plane_shape": [Y, X]` array from one object.
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
