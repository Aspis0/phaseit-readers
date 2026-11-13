// SPDX-License-Identifier: Apache-2.0
//! The N5 reader on synthetic datasets: every sample type, every compression, 2-D and 3-D geometry with partial edge
//! blocks, a missing block read as zeros, the legacy compression key, the calibration and the `region` default.
//! The datasets are written by `tests/support/n5.rs` and removed by its `Drop`.

mod support;

use phasit_imread::n5_reader::N5Reader;
use phasit_imread::types::{Dims5, PixelRect};
use phasit_imread::{DType, Error, ImageReader, Plane, PlaneBuf};

use support::n5::{sample_i32, N5Dataset};

/// The sample at `(y, x)` of plane `z` as the plane holds it (the writer gives float samples a +0.5).
fn expected(z: usize, y: usize, x: usize, dtype: DType) -> f64 {
    match dtype {
        DType::F32 => sample_i32(z, y, x) as f64 + 0.5,
        _ => sample_i32(z, y, x) as f64,
    }
}

/// The plane `z` of the reader, one sample at a time in y then x, widened to f64 (every dtype is exact in it).
fn samples(reader: &N5Reader, z: usize) -> Vec<f64> {
    let plane = reader.plane(0, 0, 0, z).expect("read the plane");
    let dims = reader.meta().scene(0).expect("the scene").dims;
    let data = plane.data;
    (0..dims.y * dims.x)
        .map(|at| match &data {
            PlaneBuf::U8(v) => v[at] as f64,
            PlaneBuf::U16(v) => v[at] as f64,
            PlaneBuf::I16(v) => v[at] as f64,
            PlaneBuf::F32(v) => v[at] as f64,
        })
        .collect()
}

/// The samples the writer states for every pixel of the `dims` plane `z`.
fn stated(z: usize, dims: Dims5, dtype: DType) -> Vec<f64> {
    (0..dims.y)
        .flat_map(|y| (0..dims.x).map(move |x| expected(z, y, x, dtype)))
        .collect()
}

/// A dataset whose blocks are written as its `attributes.json` states them.
fn volume(
    name: &str,
    dimensions: &[usize],
    block: &[usize],
    dtype: DType,
    compression: Option<&'static str>,
) -> N5Dataset {
    let dataset = N5Dataset::new(name, dimensions, block, dtype, compression);
    dataset.write_all_blocks();
    dataset
}

#[test]
fn a_2d_u8_dataset_decodes_its_one_plane() {
    // x = 7, y = 5, block (3, 2): the last column and the last row are partial blocks.
    let dataset = volume("u8_2d", &[7, 5], &[3, 2], DType::U8, None);
    let reader = N5Reader::open(&dataset).expect("open the synthetic N5");
    let scene = reader.meta().scene(0).expect("the scene");
    assert_eq!(
        scene.dims,
        Dims5 {
            t: 1,
            c: 1,
            z: 1,
            y: 5,
            x: 7
        }
    );
    assert_eq!((scene.dtype, scene.samples), (DType::U8, 1));
    let plane = reader.plane(0, 0, 0, 0).expect("the plane");
    assert_eq!(plane.shape, [5, 7]);
    assert_eq!(samples(&reader, 0), stated(0, scene.dims, DType::U8));
}

#[test]
fn a_3d_dataset_decodes_every_plane_of_every_sample_type_and_compression() {
    // x=7, y=5, z=3 against a block of (3, 2, 2): every edge block is partial in every axis.
    for (dtype, name, compression) in [
        (DType::U8, "u8_3d", None),
        (DType::U16, "u16_3d", Some("gzip")),
        (DType::I16, "i16_3d", Some("zstd")),
        (DType::F32, "f32_3d", None),
    ] {
        let dataset = volume(name, &[7, 5, 3], &[3, 2, 2], dtype, compression);
        let reader = N5Reader::open(&dataset).expect("open the dataset");
        let scene = reader.meta().scene(0).expect("the scene");
        assert_eq!(scene.dtype, dtype, "{name}");
        assert_eq!(
            scene.dims,
            Dims5 {
                t: 1,
                c: 1,
                z: 3,
                y: 5,
                x: 7
            },
            "{name}"
        );
        for z in 0..3 {
            assert_eq!(
                samples(&reader, z),
                stated(z, scene.dims, dtype),
                "{name} plane {z}"
            );
            assert_eq!(reader.plane(0, 0, 0, z).expect("the plane").shape, [5, 7]);
        }
    }
}

#[test]
fn gzip_and_zstd_planes_hold_the_same_samples_as_raw() {
    let plain = volume("raw_cmp", &[4, 3, 2], &[2, 2, 1], DType::U16, None);
    for (compression, name) in [("gzip", "gzip_cmp"), ("zstd", "zstd_cmp")] {
        let other = volume(name, &[4, 3, 2], &[2, 2, 1], DType::U16, Some(compression));
        let (raw, compressed) = (
            N5Reader::open(&plain).unwrap(),
            N5Reader::open(&other).unwrap(),
        );
        for z in 0..2 {
            assert_eq!(
                samples(&compressed, z),
                samples(&raw, z),
                "{compression} plane {z}"
            );
        }
    }
}

#[test]
fn the_legacy_compression_key_is_read() {
    let mut dataset = volume("legacy", &[4, 3, 2], &[2, 2, 1], DType::U16, Some("gzip"));
    dataset.compress_legacy("gzip");
    let reader = N5Reader::open(&dataset).expect("open the legacy dataset");
    for z in 0..2 {
        let dims = reader.meta().scene(0).expect("the scene").dims;
        assert_eq!(
            samples(&reader, z),
            stated(z, dims, DType::U16),
            "plane {z}"
        );
    }
}

#[test]
fn a_missing_block_reads_as_zeros_and_leaves_its_neighbours_alone() {
    // The block at (1, 1, 1) covers y 2..4 and x 2..4 of plane 1; the rest of the plane is the writer's samples.
    let dataset = volume("missing", &[4, 4, 2], &[2, 2, 1], DType::U16, None);
    std::fs::remove_file(dataset.block_path(&[1, 1, 1])).expect("remove a block");
    let reader = N5Reader::open(&dataset).expect("a missing block is not an error");
    let dims = reader.meta().scene(0).expect("the scene").dims;
    let held: Vec<f64> = (0..4)
        .flat_map(|y| {
            (0..4).map(move |x| match (y >= 2, x >= 2) {
                (true, true) => 0.0,
                _ => expected(1, y, x, DType::U16),
            })
        })
        .collect();
    assert_eq!(samples(&reader, 1), held);
    assert_eq!(samples(&reader, 0), stated(0, dims, DType::U16));
}

#[test]
fn the_calibration_is_published_in_um_and_nm_and_omitted_otherwise() {
    let cases = [
        (
            r#"{"dimensions":[0.5,0.25,2],"unit":"um"}"#,
            Some((0.5, 0.25, Some(2.0))),
        ),
        (
            r#"{"dimensions":[250,100],"unit":"nm"}"#,
            Some((0.25, 0.1, None)),
        ),
        (r#"{"dimensions":[1,1],"unit":"mm"}"#, None),
        (r#"{"dimensions":[1],"unit":"um"}"#, None),
    ];
    for (resolution, published) in cases {
        let mut dataset = N5Dataset::new("calib", &[4, 3, 2], &[2, 2, 1], DType::U8, None);
        dataset.calibrate(resolution);
        let reader = N5Reader::open(&dataset).expect("open the calibrated dataset");
        let scene = reader.meta().scene(0).expect("the scene");
        assert_eq!(
            scene.px_size_um.map(|px| (px.x, px.y, px.z)),
            published,
            "{resolution}"
        );
        assert_eq!(
            scene.notes.len(),
            usize::from(published.is_none()),
            "{resolution}"
        );
    }
}

#[test]
fn region_is_the_trait_default_and_matches_cropping_the_plane() {
    let dataset = volume("region", &[7, 5, 3], &[3, 2, 2], DType::U16, None);
    let reader = N5Reader::open(&dataset).expect("open the dataset");
    assert!(!reader.supports_region());
    let whole: Plane = reader.plane(0, 0, 0, 2).expect("the plane");
    for rect in [
        PixelRect::new(0, 0, 7, 5),
        PixelRect::new(2, 1, 4, 3),
        PixelRect::new(6, 4, 1, 1),
        PixelRect::new(0, 0, 0, 2),
    ] {
        assert_eq!(
            reader.region(0, 0, 0, 2, rect).expect("the region"),
            whole.crop(rect).expect("the crop"),
            "{rect:?}"
        );
    }
}

#[test]
fn a_plane_outside_the_scene_is_refused_without_a_panic() {
    let dataset = volume("coords", &[4, 3, 2], &[2, 2, 1], DType::U8, None);
    let reader = N5Reader::open(&dataset).expect("open the dataset");
    assert!(matches!(
        reader.plane(1, 0, 0, 0),
        Err(Error::SceneOutOfRange { scene: 1, count: 1 })
    ));
    for (t, c, z) in [(1, 0, 0), (0, 1, 0), (0, 0, 2)] {
        assert!(
            matches!(reader.plane(0, t, c, z), Err(Error::OutOfRange { .. })),
            "t={t} c={c} z={z}"
        );
    }
}

#[test]
fn a_dataset_over_the_decode_ceiling_is_refused_before_it_is_assembled() {
    // A raw dataset may claim any extent; the plane is refused where it is materialised.
    let dataset = N5Dataset::new("ceiling", &[70_000, 70_000, 1], &[1, 1, 1], DType::U8, None);
    let reader = N5Reader::open(&dataset).expect("an over-ceiling dataset still opens");
    assert!(matches!(
        reader.plane(0, 0, 0, 0),
        Err(Error::OversizedAllocation { .. })
    ));
}
