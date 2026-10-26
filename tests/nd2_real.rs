// SPDX-License-Identifier: Apache-2.0
//! Byte-exact real-file parity test for the ND2 reader.
//!
//! Mirrors `tests/czi_real.rs`: reads a real Nikon ND2 file from the folder
//! named by `PHASIT_REAL_DATA_DIR` (the layout is in the crate README) and
//! skips cleanly (early return) when the variable is unset, when the file is
//! absent, or when the pre-generated reference dumps are absent, so CI without
//! them stays green. When both ARE present these checks gate the reader against
//! ground truth produced by the `nd2` Python reference:
//!   * scene count and dims (T/C/Z/Y/X),
//!   * every reference plane decodes byte-identically (native LE bytes),
//!   * the first-8 samples match the manifest,
//!   * out-of-range coordinates are clean errors.
//!
//! The fixtures dir is gitignored and NOT regenerated here.

mod real_data;

use std::path::{Path, PathBuf};

use phasit_imread::nd2_reader::Nd2Reader;
use phasit_imread::{DType, Error, ImageReader, PlaneBuf};

use real_data::real_file;

const SAMPLE: &str = "nd2/stack.nd2";

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/nd2_real")
}

/// Reinterpret a decoded U16 plane's samples as little-endian bytes.
fn u16_plane_to_le_bytes(buf: &PlaneBuf) -> Vec<u8> {
    match buf {
        PlaneBuf::U16(v) => v.iter().flat_map(|s| s.to_le_bytes()).collect(),
        other => panic!("expected U16 plane, got {:?}", other.dtype()),
    }
}

#[test]
fn nd2_real_parity() {
    let Some(sample) = real_file(SAMPLE) else {
        return;
    };
    let manifest_path = fixtures_dir().join("manifest_real.json");
    if !manifest_path.exists() {
        eprintln!("skipping: manifest {} not present", manifest_path.display());
        return;
    }

    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).expect("read manifest"))
            .expect("parse manifest_real.json");

    let reader = Nd2Reader::open(&sample).expect("open ND2 sample");
    let meta = reader.meta();
    assert_eq!(meta.scene_count(), 1, "sample is a single scene");

    let scene = meta.scene(0).unwrap();
    let want_z = manifest["sizes"]["Z"].as_u64().unwrap() as usize;
    let want_y = manifest["Y"].as_u64().unwrap() as usize;
    let want_x = manifest["X"].as_u64().unwrap() as usize;
    assert_eq!(scene.dims.t, manifest["nT"].as_u64().unwrap() as usize, "T");
    assert_eq!(scene.dims.c, manifest["nC"].as_u64().unwrap() as usize, "C");
    assert_eq!(scene.dims.z, want_z, "Z");
    assert_eq!(scene.dims.y, want_y, "Y");
    assert_eq!(scene.dims.x, want_x, "X");
    assert_eq!(scene.dtype, DType::U16, "dtype");
    assert_eq!(scene.samples, 1, "samples per pixel");

    for plane_spec in manifest["planes"].as_array().expect("planes array") {
        let name = plane_spec["name"].as_str().unwrap();
        let s = plane_spec["scene"].as_u64().unwrap() as usize;
        let t = plane_spec["t"].as_u64().unwrap() as usize;
        let c = plane_spec["c"].as_u64().unwrap() as usize;
        let z = plane_spec["z"].as_u64().unwrap() as usize;
        let y = plane_spec["y"].as_u64().unwrap() as usize;
        let x = plane_spec["x"].as_u64().unwrap() as usize;

        let plane = reader
            .plane(s, t, c, z)
            .unwrap_or_else(|e| panic!("decode plane {name} ({s},{t},{c},{z}): {e}"));
        assert_eq!(plane.shape, [y, x], "shape of {name}");
        assert_eq!(plane.samples, 1, "samples of {name}");

        // first-8 samples must match the manifest.
        if let PlaneBuf::U16(v) = &plane.data {
            let first8: Vec<u64> = v.iter().take(8).map(|&s| s as u64).collect();
            let want8: Vec<u64> = plane_spec["first8"]
                .as_array()
                .unwrap()
                .iter()
                .map(|n| n.as_u64().unwrap())
                .collect();
            assert_eq!(first8, want8, "first8 of {name}");
        } else {
            panic!("plane {name} is not U16");
        }

        // Full byte-exact comparison against the reference .raw dump.
        let raw = std::fs::read(fixtures_dir().join(name))
            .unwrap_or_else(|e| panic!("read reference dump {name}: {e}"));
        let got = u16_plane_to_le_bytes(&plane.data);
        assert_eq!(got.len(), raw.len(), "byte length of {name}");
        assert!(got == raw, "byte-exact mismatch for {name}");
    }

    // Out-of-range coordinates are clean errors, never panics.
    match reader.plane(0, 0, 0, want_z) {
        Err(Error::OutOfRange { z, .. }) if z == want_z => {}
        other => panic!("expected OutOfRange z={want_z}, got {other:?}"),
    }
    match reader.plane(0, 0, 1, 0) {
        Err(Error::OutOfRange { c: 1, .. }) => {}
        other => panic!("expected OutOfRange c=1, got {other:?}"),
    }
    match reader.plane(1, 0, 0, 0) {
        Err(Error::SceneOutOfRange { scene: 1, .. }) => {}
        other => panic!("expected SceneOutOfRange scene=1, got {other:?}"),
    }
}
