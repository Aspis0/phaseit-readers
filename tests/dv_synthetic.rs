// SPDX-License-Identifier: Apache-2.0
//! The DeltaVision reader on synthetic files: byte order, sample types, ImgSequence, the extended header, the
//! calibration numbers and every refusal. The files are written by `tests/support/dv.rs` and removed by `Scratch`.

mod support;

use phasit_imread::detect_from;
use phasit_imread::dv_reader::DvReader;
use phasit_imread::types::{Dims5, PixelRect};
use phasit_imread::{DType, Error, Format, ImageReader, PlaneBuf};

use support::dv::{sample_at, DvFile};
use support::scratch::Scratch;

fn scratch(name: &str) -> Scratch {
    Scratch::new(std::env::temp_dir().join(format!("phasit_dv_{name}_{}.dv", std::process::id())))
}

fn open(file: &DvFile, name: &str) -> (Scratch, DvReader) {
    let path = scratch(name);
    file.write(&path);
    let reader = DvReader::open(&path).expect("open the synthetic DV");
    (path, reader)
}

fn i16_plane(reader: &DvReader, t: usize, c: usize, z: usize) -> Vec<i16> {
    match reader.plane(0, t, c, z).expect("read the plane").data {
        PlaneBuf::I16(values) => values,
        other => panic!("expected I16, got {other:?}"),
    }
}

fn f32_plane(reader: &DvReader, t: usize, c: usize, z: usize) -> Vec<f32> {
    match reader.plane(0, t, c, z).expect("read the plane").data {
        PlaneBuf::F32(values) => values,
        other => panic!("expected F32, got {other:?}"),
    }
}

fn u16_plane(reader: &DvReader, t: usize, c: usize, z: usize) -> Vec<u16> {
    match reader.plane(0, t, c, z).expect("read the plane").data {
        PlaneBuf::U16(values) => values,
        other => panic!("expected U16, got {other:?}"),
    }
}

fn u8_plane(reader: &DvReader, t: usize, c: usize, z: usize) -> Vec<u8> {
    match reader.plane(0, t, c, z).expect("read the plane").data {
        PlaneBuf::U8(values) => values,
        other => panic!("expected U8, got {other:?}"),
    }
}

fn section_i16(nx: usize, ny: usize, section: usize) -> Vec<i16> {
    (0..ny)
        .flat_map(|y| (0..nx).map(move |x| sample_at(section, y, x)))
        .collect()
}

#[test]
fn a_little_endian_i16_file_decodes_its_planes() {
    let (_path, reader) = open(&DvFile::i16(4, 3, 2, 1, 1, 0), "le_i16");
    let scene = reader.meta().scene(0).unwrap();
    assert_eq!(
        scene.dims,
        Dims5 {
            t: 1,
            c: 1,
            z: 2,
            y: 3,
            x: 4
        }
    );
    assert_eq!(scene.dtype, DType::I16);
    assert_eq!(scene.samples, 1);

    assert_eq!(i16_plane(&reader, 0, 0, 0), section_i16(4, 3, 0));
    assert_eq!(i16_plane(&reader, 0, 0, 1), section_i16(4, 3, 1));
    let plane = reader.plane(0, 0, 0, 0).unwrap();
    assert_eq!(plane.shape, [3, 4]);
    assert_eq!(
        plane.data,
        PlaneBuf::I16(vec![0, 1, 2, 3, 10, 11, 12, 13, 20, 21, 22, 23])
    );
}

#[test]
fn a_big_endian_i16_file_is_converted_to_native_values() {
    let (_path, reader) = open(
        &DvFile {
            big_endian: true,
            ..DvFile::i16(4, 3, 2, 1, 1, 0)
        },
        "be_i16",
    );
    assert_eq!(i16_plane(&reader, 0, 0, 0), section_i16(4, 3, 0));
    assert_eq!(i16_plane(&reader, 0, 0, 1), section_i16(4, 3, 1));
}

#[test]
fn a_mode_6_file_decodes_u16_values() {
    let (_path, reader) = open(&DvFile::u16(4, 3, 1, 1, 1, 0), "mode6");
    assert_eq!(reader.meta().scene(0).unwrap().dtype, DType::U16);
    assert_eq!(
        u16_plane(&reader, 0, 0, 0),
        vec![0, 1, 2, 3, 10, 11, 12, 13, 20, 21, 22, 23]
    );
}

#[test]
fn a_big_endian_mode_6_file_is_converted_to_native_values() {
    let (_path, reader) = open(
        &DvFile {
            big_endian: true,
            ..DvFile::u16(4, 3, 1, 1, 1, 0)
        },
        "be_mode6",
    );
    assert_eq!(
        u16_plane(&reader, 0, 0, 0),
        vec![0, 1, 2, 3, 10, 11, 12, 13, 20, 21, 22, 23]
    );
}

#[test]
fn a_mode_2_file_decodes_exact_f32_values() {
    let (_path, reader) = open(&DvFile::f32(4, 3, 2, 1, 1, 0), "f32");
    assert_eq!(reader.meta().scene(0).unwrap().dtype, DType::F32);
    let expected: Vec<f32> = section_i16(4, 3, 1)
        .iter()
        .map(|v| *v as f32 + 0.5)
        .collect();
    assert_eq!(f32_plane(&reader, 0, 0, 1), expected);
}

#[test]
fn a_mode_0_file_decodes_u8_values() {
    let (_path, reader) = open(&DvFile::u8(4, 3, 1, 1, 1, 0), "mode0");
    assert_eq!(reader.meta().scene(0).unwrap().dtype, DType::U8);
    assert_eq!(
        u8_plane(&reader, 0, 0, 0),
        vec![0, 1, 2, 3, 10, 11, 12, 13, 20, 21, 22, 23]
    );
}

#[test]
fn a_big_endian_mode_2_file_decodes_exact_f32_values() {
    let (_path, reader) = open(
        &DvFile {
            big_endian: true,
            ..DvFile::f32(4, 3, 2, 1, 1, 0)
        },
        "be_f32",
    );
    let expected: Vec<f32> = section_i16(4, 3, 1)
        .iter()
        .map(|v| *v as f32 + 0.5)
        .collect();
    assert_eq!(f32_plane(&reader, 0, 0, 1), expected);
}

#[test]
fn the_fifth_wavelength_is_read() {
    // W = 5 is the largest the format defines; a reader that refuses it would pass every test with W < 5.
    let file = DvFile::i16(4, 3, 10, 5, 1, 0);
    let (_path, reader) = open(&file, "five_waves");
    let scene = reader.meta().scene(0).unwrap();
    assert_eq!((scene.dims.c, scene.dims.t, scene.dims.z), (5, 1, 2));
    assert_eq!(scene.channels.len(), 5);
    for c in 0..5 {
        // ZTW: section (c * T + t) * Z + z with T = 1, Z = 2, t = 0, z = 1.
        assert_eq!(i16_plane(&reader, 0, c, 1), section_i16(4, 3, 2 * c + 1));
    }
}

#[test]
fn the_three_img_sequence_orders_locate_the_section() {
    // W=2, T=3, Z=4 -> 24 sections, non-square in every axis, so a swapped formula lands on another section.
    let (w, t_max, z_max) = (2usize, 3usize, 4usize);
    let nz = w * t_max * z_max;
    // (t=0, c=1, z=0) lands on section 12 / 1 / 4 under ZTW / WZT / ZWT: were the three to coincide everywhere,
    // reading the right section would prove nothing.
    assert_eq!((t_max * z_max, 1, z_max), (12, 1, 4));
    for seq in 0..=2i16 {
        let file = DvFile::i16(5, 4, nz, w, t_max, seq);
        let (_path, reader) = open(&file, &format!("seq{seq}"));
        let scene = reader.meta().scene(0).unwrap();
        assert_eq!((scene.dims.c, scene.dims.t, scene.dims.z), (2, 3, 4));
        for t in 0..t_max {
            for c in 0..w {
                for z in 0..z_max {
                    let idx = match seq {
                        0 => (c * t_max + t) * z_max + z,
                        1 => (t * z_max + z) * w + c,
                        _ => (t * w + c) * z_max + z,
                    };
                    assert_eq!(
                        i16_plane(&reader, t, c, z),
                        section_i16(5, 4, idx),
                        "seq {seq} at (t={t}, c={c}, z={z})"
                    );
                }
            }
        }
    }
}

#[test]
fn num_times_0_and_num_waves_0_read_as_1() {
    let file = DvFile {
        num_times: 0,
        num_waves: 0,
        ..DvFile::i16(4, 3, 2, 1, 1, 0)
    };
    let (_path, reader) = open(&file, "zero_counts");
    let scene = reader.meta().scene(0).unwrap();
    assert_eq!((scene.dims.t, scene.dims.c, scene.dims.z), (1, 1, 2));
    assert_eq!(i16_plane(&reader, 0, 0, 0), section_i16(4, 3, 0));
    assert_eq!(i16_plane(&reader, 0, 0, 1), section_i16(4, 3, 1));
}

#[test]
fn next_shifts_the_data() {
    let file = DvFile {
        next: 256,
        ..DvFile::i16(4, 3, 2, 1, 1, 0)
    };
    let (_path, reader) = open(&file, "extended");
    assert_eq!(i16_plane(&reader, 0, 0, 0), section_i16(4, 3, 0));
    assert_eq!(i16_plane(&reader, 0, 0, 1), section_i16(4, 3, 1));
}

#[test]
fn a_next_past_16_bits_still_shifts_the_data() {
    // 70000 does not fit in an i16, so reading `next` as one word's low half would refuse this file.
    let file = DvFile {
        next: 70_000,
        ..DvFile::i16(4, 3, 2, 1, 1, 0)
    };
    let (_path, reader) = open(&file, "wide_extended");
    assert_eq!(i16_plane(&reader, 0, 0, 0), section_i16(4, 3, 0));
    assert_eq!(i16_plane(&reader, 0, 0, 1), section_i16(4, 3, 1));
}

#[test]
fn the_calibration_numbers_are_published() {
    let file = DvFile {
        dx: 0.11,
        dy: 0.22,
        dz: 0.33,
        ..DvFile::i16(4, 3, 2, 1, 1, 0)
    };
    let (_path, reader) = open(&file, "calib");
    let px = reader
        .meta()
        .scene(0)
        .unwrap()
        .px_size_um
        .expect("a pixel size");
    assert_eq!(px.x, 0.11);
    assert_eq!(px.y, 0.22);
    assert_eq!(px.z, Some(0.33));
}

#[test]
fn a_size_that_is_not_a_distance_is_not_published() {
    // `f32::NAN` is not > 0 and `f32::INFINITY` is: only the is_finite() half of the rule rejects the latter, so a
    // reader that dropped it would publish an infinite pixel.
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.1, 0.0] {
        let file = DvFile {
            dx: bad,
            ..DvFile::i16(4, 3, 1, 1, 1, 0)
        };
        let (_path, reader) = open(&file, &format!("bad_dx_{bad}"));
        let px = reader.meta().scene(0).unwrap().px_size_um;
        assert_eq!(px.map(|px| px.x), None, "dx = {bad} is not a distance");
    }
}

#[test]
fn region_equals_cropping_the_plane() {
    let (_path, reader) = open(&DvFile::i16(4, 3, 2, 1, 1, 0), "region");
    for rect in [
        PixelRect::new(0, 0, 4, 3),
        PixelRect::new(1, 1, 2, 2),
        PixelRect::new(3, 2, 1, 1),
        PixelRect::new(0, 0, 0, 2),
    ] {
        let whole = reader.plane(0, 0, 0, 1).unwrap().crop(rect).unwrap();
        assert_eq!(reader.region(0, 0, 0, 1, rect).unwrap(), whole, "{rect:?}");
    }
}

#[test]
fn a_plane_outside_the_scene_is_refused_without_a_panic() {
    let (_path, reader) = open(&DvFile::i16(4, 3, 2, 1, 1, 0), "coords");
    assert!(matches!(
        reader.plane(1, 0, 0, 0),
        Err(Error::SceneOutOfRange { scene: 1, count: 1 })
    ));
    assert!(matches!(
        reader.plane(0, 0, 0, 2),
        Err(Error::OutOfRange { z: 2, .. })
    ));
    assert!(matches!(
        reader.plane(0, 1, 0, 0),
        Err(Error::OutOfRange { t: 1, .. })
    ));
    assert!(matches!(
        reader.plane(0, 0, 1, 0),
        Err(Error::OutOfRange { c: 1, .. })
    ));
}

#[test]
fn every_dv_extension_is_detected_and_an_mrc_renamed_dv_is_refused() {
    for ext in ["dv", "DV", "r3d", "R3D"] {
        let name = format!("stack.{ext}");
        assert_eq!(detect_from(b"", Some(&name)).unwrap(), Format::Dv, "{name}");
    }
    let file = DvFile::i16(4, 3, 1, 1, 1, 0);
    let bytes = file.bytes();
    assert!(matches!(
        detect_from(&bytes, Some("stack.txt")),
        Err(Error::Unsupported(_))
    ));
    assert_eq!(detect_from(&bytes, Some("stack.dv")).unwrap(), Format::Dv);
    // An MRC file renamed .dv has no nDVID and is refused by the DV reader with the DV sentence.
    let mrc_path = scratch("mrc_as_dv.mrc");
    support::mrc::MrcFile::i16(4, 3, 1).write(&mrc_path);
    let mrc_bytes = std::fs::read(&*mrc_path).unwrap();
    let dv_path = scratch("mrc_as_dv.dv");
    std::fs::write(&*dv_path, &mrc_bytes).unwrap();
    match phasit_imread::open(&dv_path) {
        Err(Error::Unsupported(message)) => {
            assert!(message.contains("not a DeltaVision file"), "{message}")
        }
        _ => panic!("expected Unsupported"),
    }
}

#[test]
fn open_routes_a_dv_file_to_the_dv_reader() {
    // `Scratch` owns the file and removes it on drop, so even a failed assertion leaves nothing behind.
    let path = scratch("routed");
    DvFile::i16(4, 3, 1, 1, 1, 0).write(&path);
    let reader = phasit_imread::open(&path).expect("open through the dispatcher");
    assert_eq!(reader.meta().scene_count(), 1);
    assert_eq!(reader.meta().scene(0).unwrap().dtype, DType::I16);
}
