// SPDX-License-Identifier: Apache-2.0
//! Real-file structural smoke test for the CZI reader.
//!
//! Uses real Zeiss confocal `.czi` files, read from the folder named by
//! `PHASIT_REAL_DATA_DIR` (the layout is in the crate README). Skips cleanly
//! (early return) when the variable is unset or a sample is absent, so CI
//! without the files stays green. When the files ARE present these checks gate:
//!   * dimensions (C/Z/T/Y/X in canonical TCZYX order),
//!   * mosaic composition (`czi/mosaic.czi` composes 4 M-tiles per (C,Z) into
//!     one bounding-box scene plane),
//!   * a mid-stack plane decodes to the expected shape/dtype with sane values.
//!
//! Bit-exact parity against the `czifile` reference is a SEPARATE harness (the
//! multi-GB reference dumps are built elsewhere); this test only proves
//! structural correctness.

mod real_data;

use phasit_imread::czi_reader::CziReader;
use phasit_imread::{DType, Error, ImageReader, PlaneBuf};

use real_data::real_file;

/// The mosaic sample: several tiles per (C,Z), composed into one scene.
const SAMPLE_MOSAIC: &str = "czi/mosaic.czi";
/// The pre-stitched sample: one tile per (C,Z).
const SAMPLE_STITCH: &str = "czi/stitched.czi";

/// Assert a decoded plane is structurally sane: right shape/dtype and not blank
/// (at least one non-zero 16-bit sample).
fn assert_u16_plane_sane(data: &PlaneBuf, expect_shape: [usize; 2]) {
    match data {
        PlaneBuf::U16(v) => {
            assert_eq!(v.len(), expect_shape[0] * expect_shape[1], "plane length");
            let max = v.iter().copied().max().unwrap_or(0);
            assert!(max > 0, "decoded plane is entirely zero (mis-decode?)");
        }
        other => panic!("expected U16 plane, got {:?}", other.dtype()),
    }
}

#[test]
fn czi_mosaic_geometry_and_plane() {
    let Some(path) = real_file(SAMPLE_MOSAIC) else {
        return;
    };

    let reader = CziReader::open(&path).expect("open mosaic CZI");
    let meta = reader.meta();
    assert_eq!(meta.scene_count(), 1, "the mosaic sample is a single scene");

    let scene = meta.scene(0).unwrap();
    // Ground truth: C=2, Z=55, T=1, Gray16, 4 M-tiles composed into one plane.
    assert_eq!(scene.dims.c, 2, "channels");
    assert_eq!(scene.dims.z, 55, "z planes");
    assert_eq!(scene.dims.t, 1, "time points");
    assert_eq!(scene.dtype, DType::U16);
    assert_eq!(scene.channels.len(), 2);
    // The composed mosaic plane must be larger than a single tile (tiles span
    // X/Y positions -486..1509, so the bounding box is ~1996 on a side).
    assert!(scene.dims.x > 1000 && scene.dims.y > 1000, "composed mosaic size");

    // A mid-stack plane decodes to the composed scene shape with sane values.
    let z_mid = scene.dims.z / 2;
    let expect = [scene.dims.y, scene.dims.x];
    let p = reader.plane(0, 0, 0, z_mid).expect("decode mid-stack plane");
    assert_eq!(p.shape, expect);
    assert_u16_plane_sane(&p.data, expect);

    // Out-of-range coordinates are clean errors, never panics.
    match reader.plane(0, 0, 2, 0) {
        Err(Error::OutOfRange { c: 2, .. }) => {}
        other => panic!("expected OutOfRange for c=2, got {other:?}"),
    }
    match reader.plane(0, 0, 0, 55) {
        Err(Error::OutOfRange { z: 55, .. }) => {}
        other => panic!("expected OutOfRange for z=55, got {other:?}"),
    }
    match reader.plane(0, 1, 0, 0) {
        Err(Error::OutOfRange { t: 1, .. }) => {}
        other => panic!("expected OutOfRange for t=1, got {other:?}"),
    }
    assert!(matches!(
        reader.plane(9, 0, 0, 0),
        Err(Error::SceneOutOfRange { scene: 9, .. })
    ));
}

#[test]
fn czi_stitched_geometry_and_plane() {
    let Some(path) = real_file(SAMPLE_STITCH) else {
        return;
    };

    let reader = CziReader::open(&path).expect("open stitched CZI");
    let meta = reader.meta();
    assert_eq!(meta.scene_count(), 1, "stitched file is a single scene");

    let scene = meta.scene(0).unwrap();
    // Ground truth: C=4, Z=90, T=1, M=1 (pre-stitched), Gray16, ~1947x1951.
    assert_eq!(scene.dims.c, 4, "channels");
    assert_eq!(scene.dims.z, 90, "z planes");
    assert_eq!(scene.dims.t, 1, "time points");
    assert_eq!(scene.dtype, DType::U16);
    assert_eq!(scene.channels.len(), 4);

    // Decode a mid-stack plane from a non-zero channel; single tile => full plane.
    let z_mid = scene.dims.z / 2;
    let expect = [scene.dims.y, scene.dims.x];
    let p = reader.plane(0, 0, 2, z_mid).expect("decode mid-stack plane");
    assert_eq!(p.shape, expect);
    assert_u16_plane_sane(&p.data, expect);

    // Full plane_index sweep would be expensive; just confirm every channel at
    // z_mid decodes to the same composed shape.
    for c in 0..scene.dims.c {
        let pc = reader.plane(0, 0, c, z_mid).expect("decode channel plane");
        assert_eq!(pc.shape, expect, "channel {c} shape");
        assert_eq!(pc.dtype(), DType::U16);
    }
}

#[test]
fn czi_bgr24_color_geometry_and_plane() {
    // Real uncompressed Bgr24 sample (Zeiss/Bio-Formats `c_winnt.czi`, 275x174,
    // single subblock). Third-party — NOT committed; gated behind an env var so
    // the test skips cleanly when unset (mirrors the other real-file gates).
    let path_buf = match std::env::var("PHASIT_CZI_BGR_SAMPLE") {
        Ok(p) => std::path::PathBuf::from(p),
        Err(_) => {
            eprintln!("skipping: PHASIT_CZI_BGR_SAMPLE not set");
            return;
        }
    };
    let path = path_buf.as_path();
    if !path.exists() {
        eprintln!("skipping: Bgr24 sample {} not present", path.display());
        return;
    }

    let reader = CziReader::open(path).expect("open Bgr24 CZI");
    let meta = reader.meta();
    assert_eq!(meta.scene_count(), 1, "c_winnt.czi is a single scene");

    let scene = meta.scene(0).unwrap();
    // Ground truth: 275x174 Bgr24, single subblock. Color is carried as 3
    // samples per pixel, NOT as extra C channels (C stays 1).
    assert_eq!(scene.dims.c, 1, "color must NOT explode into C channels");
    assert_eq!(scene.dims.z, 1);
    assert_eq!(scene.dims.t, 1);
    assert_eq!(scene.dims.y, 174, "height");
    assert_eq!(scene.dims.x, 275, "width");
    assert_eq!(scene.dtype, DType::U8, "per-sample type of Bgr24 is U8");
    assert_eq!(scene.samples, 3, "3 samples per pixel (RGB)");

    // The plane is an owned, interleaved RGB buffer: 275*174*3 == 143550 bytes.
    let p = reader.plane(0, 0, 0, 0).expect("decode Bgr24 plane");
    assert_eq!(p.shape, [174, 275]);
    assert_eq!(p.samples(), 3);
    match &p.data {
        PlaneBuf::U8(v) => {
            assert_eq!(v.len(), 275 * 174 * 3, "interleaved RGB sample count");
            assert!(v.iter().any(|&s| s > 0), "decoded plane is entirely zero");
        }
        other => panic!("expected U8 (RGB) plane, got {:?}", other.dtype()),
    }
}
