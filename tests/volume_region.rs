// SPDX-License-Identifier: Apache-2.0
//! The default `volume()` reads a 3-D box plane by plane: a volume must hold the bytes of `region` over the box's
//! planes, in that order, and a box the scene cannot fill must be refused rather than panic.
//! The fixtures are `tests/fixtures/tiff/`, whose manifest states the dims, dtype and samples every volume repeats.

use std::path::{Path, PathBuf};

use phasit_imread::tiff_reader::TiffReader;
use phasit_imread::types::{Box3, PixelRect};
use phasit_imread::{Error, ImageReader, PlaneBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tiff")
        .join(name)
}

/// The samples in the form a flat buffer is compared in: native-endian bytes.
fn native_bytes(data: &PlaneBuf) -> Vec<u8> {
    match data {
        PlaneBuf::U8(values) => values.iter().flat_map(|v| v.to_ne_bytes()).collect(),
        PlaneBuf::U16(values) => values.iter().flat_map(|v| v.to_ne_bytes()).collect(),
        PlaneBuf::I16(values) => values.iter().flat_map(|v| v.to_ne_bytes()).collect(),
        PlaneBuf::F32(values) => values.iter().flat_map(|v| v.to_ne_bytes()).collect(),
    }
}

/// A volume is the box's regions, plane by plane, and it repeats the scene's pixel type and sample count.
fn assert_stacked(reader: &TiffReader, scene: usize, t: usize, c: usize, bx: Box3, what: &str) {
    let volume = reader
        .volume(scene, t, c, bx)
        .unwrap_or_else(|err| panic!("{what}: the box is inside the scene, got {err:?}"));
    let meta = reader.meta().scene(scene).expect("the scene");
    assert_eq!(
        volume.dims,
        [bx.w, bx.h, bx.d],
        "{what}: the volume's dims differ"
    );
    assert_eq!(
        volume.samples, meta.samples,
        "{what}: the volume's samples differ from the scene"
    );
    assert_eq!(
        volume.dtype(),
        meta.dtype,
        "{what}: the volume's dtype differs from the scene"
    );

    let rect = PixelRect::new(bx.x, bx.y, bx.w, bx.h);
    let mut stacked = Vec::new();
    for z in bx.z..bx.z + bx.d {
        let plane = reader
            .region(scene, t, c, z, rect)
            .unwrap_or_else(|err| panic!("{what}: the plane at z={z}, got {err:?}"));
        stacked.extend_from_slice(&native_bytes(&plane.data));
    }
    assert_eq!(
        native_bytes(&volume.data),
        stacked,
        "{what}: the volume's bytes differ from the stacked regions"
    );
}

#[test]
fn a_multi_z_tiff_volume_equals_stacking_its_regions() {
    // 2 channels x 2 slices: z > 1, so the stacking order itself is under test.
    let reader = TiffReader::open(&fixture("imagej-hyperstack.tif")).expect("open the hyperstack");
    for c in 0..2 {
        let cases = [
            (Box3::new(0, 0, 0, 4, 3, 2), "the whole stack"),
            (Box3::new(1, 1, 0, 2, 2, 2), "a window over both slices"),
            (Box3::new(0, 0, 1, 4, 3, 1), "a one-plane box at z=1"),
            (
                Box3::new(3, 2, 1, 1, 1, 1),
                "a single voxel at the far corner",
            ),
        ];
        for (bx, what) in cases {
            assert_stacked(&reader, 0, 0, c, bx, what);
        }
    }
}

#[test]
fn a_16_bit_volume_equals_stacking_its_regions() {
    let reader = TiffReader::open(&fixture("two-series.ome.tif")).expect("open the OME-TIFF");
    assert_stacked(&reader, 0, 0, 0, Box3::new(0, 0, 0, 4, 3, 1), "a U16 scene");
    assert_stacked(
        &reader,
        0,
        0,
        0,
        Box3::new(1, 1, 0, 2, 2, 1),
        "a U16 window",
    );

    let reader = TiffReader::open(&fixture("int16-gray.tif")).expect("open the int16 TIFF");
    assert_stacked(
        &reader,
        0,
        0,
        0,
        Box3::new(2, 1, 0, 2, 2, 1),
        "an I16 scene",
    );

    let reader = TiffReader::open(&fixture("float32-gray.tif")).expect("open the float TIFF");
    assert_stacked(
        &reader,
        0,
        0,
        0,
        Box3::new(1, 1, 0, 2, 2, 1),
        "an F32 scene",
    );
}

#[test]
fn a_multi_sample_volume_keeps_its_samples() {
    // An RGBA page interleaves four samples per pixel: a volume that lost them would be half as long.
    let reader = TiffReader::open(&fixture("rgba-8bit.tif")).expect("open the RGBA TIFF");
    let bx = Box3::new(1, 0, 0, 2, 3, 1);
    let volume = reader.volume(0, 0, 0, bx).expect("read the volume");
    assert_eq!(volume.samples, 4);
    assert_stacked(&reader, 0, 0, 0, bx, "an RGBA box");
}

#[test]
fn an_empty_box_is_refused() {
    let reader = TiffReader::open(&fixture("imagej-hyperstack.tif")).expect("open the hyperstack");
    for bx in [
        Box3::new(0, 0, 0, 0, 3, 2),
        Box3::new(0, 0, 0, 4, 0, 2),
        Box3::new(0, 0, 0, 4, 3, 0),
    ] {
        let err = reader
            .volume(0, 0, 0, bx)
            .expect_err("a box with an empty side is refused");
        assert!(
            matches!(err, Error::Decode(_)),
            "an empty box is a bad request, got {err:?}"
        );
    }
}

#[test]
fn a_box_past_the_scene_is_refused_on_every_axis() {
    let reader = TiffReader::open(&fixture("imagej-hyperstack.tif")).expect("open the hyperstack");
    for bx in [
        Box3::new(3, 0, 0, 2, 1, 1),
        Box3::new(0, 2, 0, 1, 2, 1),
        Box3::new(0, 0, 1, 1, 1, 2),
    ] {
        let err = reader
            .volume(0, 0, 0, bx)
            .expect_err("a box past the scene is refused");
        assert!(
            matches!(err, Error::Decode(_)),
            "a box past the scene is a bad request, got {err:?}"
        );
    }
}

#[test]
fn a_wrong_coordinate_is_out_of_range_before_the_box_is_measured() {
    let reader = TiffReader::open(&fixture("imagej-hyperstack.tif")).expect("open the hyperstack");
    for (t, c) in [(1, 0), (0, 2)] {
        let err = reader
            .volume(0, t, c, Box3::new(0, 0, 0, 0, 0, 0))
            .expect_err("a call with a wrong coordinate is refused");
        assert!(
            matches!(err, Error::OutOfRange { .. }),
            "a wrong coordinate is out of range, got {err:?}"
        );
    }
    let err = reader
        .volume(1, 0, 0, Box3::new(0, 0, 0, 4, 3, 2))
        .expect_err("a call with a wrong scene is refused");
    assert!(
        matches!(err, Error::SceneOutOfRange { .. }),
        "a wrong scene is out of range, got {err:?}"
    );
}

#[test]
fn an_absurd_box_is_refused_without_a_panic() {
    let reader = TiffReader::open(&fixture("imagej-hyperstack.tif")).expect("open the hyperstack");
    let cases = [
        (Box3::new(usize::MAX, 0, 0, 2, 1, 1), "x+w overflows"),
        (Box3::new(0, usize::MAX, 0, 1, 2, 1), "y+h overflows"),
        (Box3::new(0, 0, 0, 1, 1, usize::MAX), "z+d overflows"),
    ];
    for (bx, what) in cases {
        let err = reader
            .volume(0, 0, 0, bx)
            .expect_err("an overflowing box is refused");
        assert!(
            matches!(err, Error::Decode(_)),
            "{what}: a box is a bad request, got {err:?}"
        );
    }
}
