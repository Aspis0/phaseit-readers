// SPDX-License-Identifier: Apache-2.0
//! Real-file smoke test of the CZI reader: dimensions, mosaic and a mid-stack plane, read from `PHASIT_REAL_DATA_DIR`.
//! Skips when the variable or a sample is absent; `PHASIT_REAL_DATA_REQUIRED=1` makes those failures.
//! Bit-exact parity against `czifile` is a separate harness; this test proves structure only.

mod real_data;

use phasit_imread::czi_reader::CziReader;
use phasit_imread::{DType, Error, ImageReader, PlaneBuf};

use real_data::{bgr_sample, real_file};

const SAMPLE_MOSAIC: &str = "czi/mosaic.czi";
const SAMPLE_STITCH: &str = "czi/stitched.czi";

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
    // Tiles span -486..1509 on X/Y, so the composed plane is about 1996 pixels on a side.
    assert!(scene.dims.x > 1000 && scene.dims.y > 1000, "composed mosaic size");

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

    // A full sweep of every plane is too slow for a smoke test: the channels at z_mid are checked.
    for c in 0..scene.dims.c {
        let pc = reader.plane(0, 0, c, z_mid).expect("decode channel plane");
        assert_eq!(pc.shape, expect, "channel {c} shape");
        assert_eq!(pc.dtype(), DType::U16);
    }
}

#[test]
fn czi_bgr24_color_geometry_and_plane() {
    // Third-party and not committed, so it is an optional extra: it skips even when the real data is required.
    let Some(path) = bgr_sample() else {
        return;
    };

    let reader = CziReader::open(&path).expect("open Bgr24 CZI");
    let meta = reader.meta();
    assert_eq!(meta.scene_count(), 1, "c_winnt.czi is a single scene");

    let scene = meta.scene(0).unwrap();
    // Ground truth: Bgr24 is 3 samples per pixel, not extra C channels (C stays 1).
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
