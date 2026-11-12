// SPDX-License-Identifier: Apache-2.0
//! The MRC2014 reader on synthetic files: byte order, sample types, ISPG stacking, the extended header, the
//! calibration numbers and every refusal. The files are written by `tests/support/mrc.rs` and removed by `Scratch`.

mod support;

use phasit_imread::detect_from;
use phasit_imread::mrc_reader::MrcReader;
use phasit_imread::types::{Dims5, PixelRect};
use phasit_imread::{DType, Error, Format, ImageReader, PlaneBuf};

use support::mrc::{sample_at, MrcFile};
use support::scratch::Scratch;

fn scratch(name: &str) -> Scratch {
    Scratch::new(std::env::temp_dir().join(format!("phasit_mrc_{name}_{}.mrc", std::process::id())))
}

fn open(file: &MrcFile, name: &str) -> (Scratch, MrcReader) {
    let path = scratch(name);
    file.write(&path);
    let reader = MrcReader::open(&path).expect("open the synthetic MRC");
    (path, reader)
}

fn i16_plane(reader: &MrcReader, t: usize, z: usize) -> Vec<i16> {
    match reader.plane(0, t, 0, z).expect("read the plane").data {
        PlaneBuf::I16(values) => values,
        other => panic!("expected I16, got {other:?}"),
    }
}

fn f32_plane(reader: &MrcReader, t: usize, z: usize) -> Vec<f32> {
    match reader.plane(0, t, 0, z).expect("read the plane").data {
        PlaneBuf::F32(values) => values,
        other => panic!("expected F32, got {other:?}"),
    }
}

fn u16_plane(reader: &MrcReader, t: usize, z: usize) -> Vec<u16> {
    match reader.plane(0, t, 0, z).expect("read the plane").data {
        PlaneBuf::U16(values) => values,
        other => panic!("expected U16, got {other:?}"),
    }
}

fn section_i16(nx: usize, ny: usize, section: usize) -> Vec<i16> {
    (0..ny)
        .flat_map(|y| (0..nx).map(move |x| sample_at(section, y, x)))
        .collect()
}

fn refusal_text(file: &MrcFile, name: &str) -> String {
    let path = scratch(name);
    file.write(&path);
    match MrcReader::open(&path) {
        Err(Error::Unsupported(message)) => message,
        Err(other) => panic!("expected Unsupported, got {other}"),
        Ok(_) => panic!("the file opened"),
    }
}

#[test]
fn a_little_endian_i16_file_decodes_its_planes() {
    let (_path, reader) = open(&MrcFile::i16(4, 3, 2), "le_i16");
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

    assert_eq!(i16_plane(&reader, 0, 0), section_i16(4, 3, 0));
    assert_eq!(i16_plane(&reader, 0, 1), section_i16(4, 3, 1));
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
        &MrcFile {
            big_endian: true,
            ..MrcFile::i16(4, 3, 2)
        },
        "be_i16",
    );
    assert_eq!(i16_plane(&reader, 0, 0), section_i16(4, 3, 0));
    assert_eq!(i16_plane(&reader, 0, 1), section_i16(4, 3, 1));
}

#[test]
fn a_mode_6_file_decodes_u16_values() {
    let (_path, reader) = open(&MrcFile::u16(4, 3, 1), "mode6");
    assert_eq!(reader.meta().scene(0).unwrap().dtype, DType::U16);
    assert_eq!(
        u16_plane(&reader, 0, 0),
        vec![0, 1, 2, 3, 10, 11, 12, 13, 20, 21, 22, 23]
    );
}

#[test]
fn a_big_endian_mode_6_file_is_converted_to_native_values() {
    let (_path, reader) = open(
        &MrcFile {
            big_endian: true,
            ..MrcFile::u16(4, 3, 1)
        },
        "be_mode6",
    );
    assert_eq!(
        u16_plane(&reader, 0, 0),
        vec![0, 1, 2, 3, 10, 11, 12, 13, 20, 21, 22, 23]
    );
}

#[test]
fn a_mode_2_file_decodes_exact_f32_values() {
    let (_path, reader) = open(&MrcFile::f32(4, 3, 2), "f32");
    assert_eq!(reader.meta().scene(0).unwrap().dtype, DType::F32);
    let expected: Vec<f32> = section_i16(4, 3, 1)
        .iter()
        .map(|v| *v as f32 + 0.5)
        .collect();
    assert_eq!(f32_plane(&reader, 0, 1), expected);
}

#[test]
fn a_volume_states_its_pixel_sizes_in_micrometres() {
    let file = MrcFile {
        cella: [10.0, 20.0, 40.0],
        mx: 1,
        my: 2,
        mz: 4,
        ispg: 1,
        ..MrcFile::i16(4, 3, 2)
    };
    let (_path, reader) = open(&file, "volume");
    let px = reader
        .meta()
        .scene(0)
        .unwrap()
        .px_size_um
        .expect("a pixel size");
    assert_eq!(px.x, 10.0 * 1e-4);
    assert_eq!(px.y, 10.0 * 1e-4);
    assert_eq!(px.z, Some(40.0 / 4.0 * 1e-4));
    assert_eq!(reader.meta().scene(0).unwrap().dims.z, 2);
}

#[test]
fn a_stack_of_images_states_no_z_calibration() {
    let file = MrcFile {
        cella: [10.0, 20.0, 40.0],
        my: 2,
        ..MrcFile::i16(4, 3, 2)
    };
    let (_path, reader) = open(&file, "stack");
    let px = reader
        .meta()
        .scene(0)
        .unwrap()
        .px_size_um
        .expect("a pixel size");
    assert_eq!(px.z, None, "ISPG 0 is a stack of images, not a volume");
    assert_eq!(px.x, 10.0 * 1e-4);
    assert_eq!(px.y, 10.0 * 1e-4);
}

#[test]
fn a_stack_of_volumes_splits_time_and_z() {
    // An asymmetric split: 6 sections of 3 are 2 time points of 3 sections, so t and z cannot be confused.
    let file = MrcFile {
        ispg: 401,
        mz: 3,
        ..MrcFile::i16(4, 3, 6)
    };
    let (_path, reader) = open(&file, "volumes");
    let scene = reader.meta().scene(0).unwrap();
    assert_eq!(
        scene.dims,
        Dims5 {
            t: 2,
            c: 1,
            z: 3,
            y: 3,
            x: 4
        }
    );
    assert_eq!(i16_plane(&reader, 1, 0), section_i16(4, 3, 3));
    assert_eq!(i16_plane(&reader, 1, 2), section_i16(4, 3, 5));
    assert_eq!(i16_plane(&reader, 0, 2), section_i16(4, 3, 2));
}

#[test]
fn an_extended_header_shifts_the_data() {
    let file = MrcFile {
        extended: 128,
        ..MrcFile::i16(4, 3, 2)
    };
    let (_path, reader) = open(&file, "extended");
    assert_eq!(i16_plane(&reader, 0, 0), section_i16(4, 3, 0));
    assert_eq!(i16_plane(&reader, 0, 1), section_i16(4, 3, 1));
}

#[test]
fn region_equals_cropping_the_plane() {
    let (_path, reader) = open(&MrcFile::i16(4, 3, 2), "region");
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
    let (_path, reader) = open(&MrcFile::i16(4, 3, 2), "coords");
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
fn a_file_without_the_map_signature_is_refused() {
    let message = refusal_text(
        &MrcFile {
            map: Some(*b"MRX "),
            ..MrcFile::i16(4, 3, 1)
        },
        "nomap",
    );
    assert!(message.contains("`MAP `"), "{message}");
}

#[test]
fn an_unknown_byte_order_is_refused() {
    let message = refusal_text(
        &MrcFile {
            machst: Some([0x22, 0x22]),
            ..MrcFile::i16(4, 3, 1)
        },
        "machst",
    );
    assert!(message.contains("MACHST"), "{message}");
}

#[test]
fn mode_0_is_refused() {
    let message = refusal_text(
        &MrcFile {
            mode: 0,
            ..MrcFile::i16(4, 3, 1)
        },
        "mode0",
    );
    assert!(message.contains("MODE 0"), "{message}");
}

#[test]
fn the_modes_without_a_dtype_are_refused_by_name() {
    for mode in [3, 4, 12, 101] {
        let message = refusal_text(
            &MrcFile {
                mode,
                ..MrcFile::i16(4, 3, 1)
            },
            &format!("mode{mode}"),
        );
        assert!(message.contains(&format!("MODE {mode}")), "{message}");
    }
}

#[test]
fn an_axis_order_other_than_one_two_three_is_refused() {
    let message = refusal_text(
        &MrcFile {
            maps: 4,
            ..MrcFile::i16(4, 3, 1)
        },
        "maps",
    );
    assert!(message.contains("MAPC/MAPR/MAPS"), "{message}");
}

#[test]
fn non_positive_extents_are_refused() {
    for axis in 0..3 {
        let mut file = MrcFile::i16(4, 3, 1);
        match axis {
            0 => file.nx = 0,
            1 => file.ny = 0,
            _ => file.nz = 0,
        }
        let message = refusal_text(&file, &format!("extent{axis}"));
        assert!(message.contains("must be positive"), "{message}");
    }
}

#[test]
fn a_negative_extended_header_length_is_refused() {
    let mut file = MrcFile::i16(4, 3, 1);
    file.nsymbt = Some(-1);
    let message = refusal_text(&file, "nsymbt");
    assert!(message.contains("NSYMBT"), "{message}");
}

#[test]
fn a_stack_of_volumes_without_a_whole_number_of_volumes_is_refused() {
    let file = MrcFile {
        ispg: 401,
        mz: 2,
        ..MrcFile::i16(4, 3, 3)
    };
    let message = refusal_text(&file, "indivisible_volumes");
    assert!(message.contains("does not divide NZ 3"), "{message}");
}

#[test]
fn data_past_the_end_of_the_file_is_refused() {
    let file = MrcFile::i16(4, 3, 2);
    let path = scratch("truncated");
    let mut bytes = file.bytes();
    bytes.truncate(bytes.len() - 1);
    std::fs::write(&*path, &bytes).expect("write the truncated MRC");
    match MrcReader::open(&path) {
        Err(Error::Parse(message)) => assert!(message.contains("past the end of the"), "{message}"),
        Err(other) => panic!("expected Parse, got {other}"),
        Ok(_) => panic!("expected Parse, got a reader"),
    }
}

#[test]
fn every_mrc_extension_is_detected_and_a_text_file_is_not() {
    for ext in ["mrc", "mrcs", "rec", "st", "map", "ali", "MRC", "Rec"] {
        let name = format!("stack.{ext}");
        assert_eq!(
            detect_from(b"", Some(&name)).unwrap(),
            Format::Mrc,
            "{name}"
        );
    }
    let file = MrcFile::i16(4, 3, 1);
    let bytes = file.bytes();
    assert!(matches!(
        detect_from(&bytes, Some("stack.txt")),
        Err(Error::Unsupported(_))
    ));
    assert_eq!(detect_from(&bytes, Some("stack.mrc")).unwrap(), Format::Mrc);
}

#[test]
fn a_mixed_little_endian_machst_is_still_little_endian() {
    let file = MrcFile {
        machst: Some([0x44, 0x41]),
        ..MrcFile::i16(4, 3, 2)
    };
    let (_path, reader) = open(&file, "mixed_stamp");
    assert_eq!(i16_plane(&reader, 0, 0), section_i16(4, 3, 0));
    assert_eq!(i16_plane(&reader, 0, 1), section_i16(4, 3, 1));
}

#[test]
fn open_routes_an_mrc_file_to_the_mrc_reader() {
    // `Scratch` owns the file and removes it on drop, so even a failed assertion leaves nothing behind.
    let path = scratch("routed");
    MrcFile::i16(4, 3, 1).write(&path);
    let reader = phasit_imread::open(&path).expect("open through the dispatcher");
    assert_eq!(reader.meta().scene_count(), 1);
    assert_eq!(reader.meta().scene(0).unwrap().dtype, DType::I16);
}
