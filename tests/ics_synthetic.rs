// SPDX-License-Identifier: Apache-2.0
//! The ICS reader on synthetic files: both versions, both line separators, two field separators, every sample type
//! and byte order, axis orders other than x y first, gzip, the calibration and the `region` default.
//! The files are written by `tests/support/ics.rs` and removed by `Scratch`.

mod support;

use phasit_imread::detect_from;
use phasit_imread::ics_reader::IcsReader;
use phasit_imread::types::PixelRect;
use phasit_imread::{DType, Format, ImageReader, PlaneBuf};

use support::ics::{plane_of, sample_at, IcsFile};
use support::scratch::Scratch;

fn scratch(name: &str) -> Scratch {
    Scratch::new(std::env::temp_dir().join(format!("phasit_ics_{name}_{}.ics", std::process::id())))
}

/// A 2.0 file written to its own scratch path.
fn open(file: &IcsFile, name: &str) -> (Scratch, IcsReader) {
    let path = scratch(name);
    file.write(&path);
    let reader = IcsReader::open(&path).expect("open the synthetic ICS");
    (path, reader)
}

fn u16_plane(reader: &IcsReader, t: usize, c: usize, z: usize) -> Vec<u16> {
    match reader.plane(0, t, c, z).expect("read the plane").data {
        PlaneBuf::U16(values) => values,
        other => panic!("expected U16, got {other:?}"),
    }
}

fn i16_plane(reader: &IcsReader, t: usize, c: usize, z: usize) -> Vec<i16> {
    match reader.plane(0, t, c, z).expect("read the plane").data {
        PlaneBuf::I16(values) => values,
        other => panic!("expected I16, got {other:?}"),
    }
}

fn u8_plane(reader: &IcsReader, t: usize, c: usize, z: usize) -> Vec<u8> {
    match reader.plane(0, t, c, z).expect("read the plane").data {
        PlaneBuf::U8(values) => values,
        other => panic!("expected U8, got {other:?}"),
    }
}

fn f32_plane(reader: &IcsReader, t: usize, c: usize, z: usize) -> Vec<f32> {
    match reader.plane(0, t, c, z).expect("read the plane").data {
        PlaneBuf::F32(values) => values,
        other => panic!("expected F32, got {other:?}"),
    }
}

/// The samples a plane of `x` by `y` holds at plane index `plane`, in the crate's own row-major order.
fn plane_u16(x: usize, y: usize, plane: usize) -> Vec<u16> {
    (0..y)
        .flat_map(|row| (0..x).map(move |col| sample_at(plane, row, col) as u16))
        .collect()
}

#[test]
fn an_absent_axis_has_size_one() {
    // x and y only: z, c and t are each 1, so the file is one plane.
    let (_path, reader) = open(&IcsFile::new(&["x", "y"], &[4, 3], 16), "no_z");
    let dims = reader.meta().scene(0).unwrap().dims;
    assert_eq!((dims.x, dims.y, dims.z, dims.c, dims.t), (4, 3, 1, 1, 1));
    assert_eq!(u16_plane(&reader, 0, 0, 0), plane_u16(4, 3, 0));
}

#[test]
fn a_version_2_file_decodes_every_plane() {
    let file = IcsFile::new(&["x", "y", "z", "c", "t"], &[5, 3, 4, 3, 2], 16);
    let (_path, reader) = open(&file, "v2");
    let scene = reader.meta().scene(0).unwrap();
    assert_eq!((scene.dims.x, scene.dims.y, scene.dims.z), (5, 3, 4));
    assert_eq!((scene.dims.c, scene.dims.t), (3, 2));
    assert_eq!(scene.dtype, DType::U16);
    assert_eq!(scene.samples, 1);
    assert_eq!(reader.meta().scene_count(), 1);

    for t in 0..2 {
        for c in 0..3 {
            for z in 0..4 {
                let plane = plane_of(t, c, z, 3, 4);
                assert_eq!(
                    u16_plane(&reader, t, c, z),
                    plane_u16(5, 3, plane),
                    "plane (t={t}, c={c}, z={z})"
                );
            }
        }
    }
}

#[test]
fn a_version_1_file_reads_through_either_file() {
    let file = IcsFile {
        version_text: "1.0".to_string(),
        ..IcsFile::new(&["x", "y", "z", "c", "t"], &[5, 3, 2, 2, 2], 16)
    };
    // The pair is a directory of its own, so the guard removes both files even when an assertion fails.
    let dir = support::scratch::ScratchDir::new(
        std::env::temp_dir().join(format!("phasit_ics_v1_pair_{}", std::process::id())),
    );
    let _ = std::fs::remove_dir_all(&*dir);
    std::fs::create_dir_all(&*dir).expect("create the scratch folder");
    let ics = dir.join("stack.ics");
    let ids = dir.join("stack.ids");
    file.write_pair(&ics, &ids);

    for path in [&ics, &ids] {
        let reader = IcsReader::open(path).expect("open the 1.0 pair through either file");
        for t in 0..2 {
            for c in 0..2 {
                for z in 0..2 {
                    let plane = plane_of(t, c, z, 2, 2);
                    assert_eq!(u16_plane(&reader, t, c, z), plane_u16(5, 3, plane));
                }
            }
        }
    }
}

#[test]
fn cr_lf_line_separators_and_another_field_separator_parse() {
    let mut file = IcsFile::new(&["x", "y", "z"], &[4, 3, 2], 16);
    file.crlf = true;
    let (_path, reader) = open(&file, "crlf");
    assert_eq!(u16_plane(&reader, 0, 0, 1), plane_u16(4, 3, 1));

    let mut spaced = IcsFile::new(&["x", "y", "z"], &[4, 3, 2], 16);
    spaced.field = b' ';
    let (_path, reader) = open(&spaced, "space");
    assert_eq!(u16_plane(&reader, 0, 0, 1), plane_u16(4, 3, 1));
}

#[test]
fn the_four_sample_types_decode_their_own_values() {
    let u8_file = IcsFile {
        bits: 8,
        byte_order: None,
        ..IcsFile::new(&["x", "y", "z"], &[4, 3, 2], 8)
    };
    let (_path, reader) = open(&u8_file, "u8");
    assert_eq!(reader.meta().scene(0).unwrap().dtype, DType::U8);
    let bytes: Vec<u8> = plane_u16(4, 3, 1).iter().map(|v| *v as u8).collect();
    assert_eq!(u8_plane(&reader, 0, 0, 1), bytes);

    let i16_file = IcsFile {
        sign: "signed".to_string(),
        ..IcsFile::new(&["x", "y", "z"], &[4, 3, 2], 16)
    };
    let (_path, reader) = open(&i16_file, "i16");
    assert_eq!(reader.meta().scene(0).unwrap().dtype, DType::I16);
    let words: Vec<i16> = plane_u16(4, 3, 1).iter().map(|v| *v as i16).collect();
    assert_eq!(i16_plane(&reader, 0, 0, 1), words);

    let f32_file = IcsFile::new(&["x", "y", "z"], &[4, 3, 2], 32);
    let (_path, reader) = open(&f32_file, "f32");
    assert_eq!(reader.meta().scene(0).unwrap().dtype, DType::F32);
    let expected: Vec<f32> = plane_u16(4, 3, 1).iter().map(|v| *v as f32 + 0.5).collect();
    assert_eq!(f32_plane(&reader, 0, 0, 1), expected);
}

#[test]
fn a_big_endian_file_is_converted_to_native_values() {
    let file = IcsFile {
        byte_order: Some("2 1".to_string()),
        ..IcsFile::new(&["x", "y", "z"], &[4, 3, 2], 16)
    };
    let (_path, reader) = open(&file, "be");
    assert_eq!(u16_plane(&reader, 0, 0, 0), plane_u16(4, 3, 0));
    assert_eq!(u16_plane(&reader, 0, 0, 1), plane_u16(4, 3, 1));
}

#[test]
fn axis_orders_other_than_x_y_first_gather_every_plane() {
    for (axes, sizes) in [
        (vec!["ch", "x", "y", "z", "t"], vec![3, 5, 3, 4, 2]),
        (vec!["x", "y", "t", "ch", "z"], vec![5, 3, 2, 3, 4]),
        (vec!["t", "z", "ch", "y", "x"], vec![2, 4, 3, 3, 5]),
        // x is the fastest axis but y is not the second, so the rows are not adjacent: gather them.
        (vec!["x", "ch", "y", "z"], vec![5, 3, 3, 4]),
    ] {
        let file = IcsFile::new(&axes, &sizes, 16);
        let name = format!("order_{}", axes.first().unwrap());
        let (_path, reader) = open(&file, &name);
        let dims = reader.meta().scene(0).unwrap().dims;
        // The two sizes the four orders share; an order may leave t or ch out, and it is then 1.
        assert_eq!((dims.x, dims.y, dims.z), (5, 3, 4), "{name}");
        for t in 0..dims.t {
            for c in 0..dims.c {
                for z in 0..dims.z {
                    let plane = plane_of(t, c, z, dims.c, dims.z);
                    assert_eq!(
                        u16_plane(&reader, t, c, z),
                        plane_u16(5, 3, plane),
                        "{name} plane (t={t}, c={c}, z={z})"
                    );
                }
            }
        }
    }
}

#[test]
fn a_gzip_file_matches_its_uncompressed_twin() {
    let axes = ["x", "y", "z", "c", "t"];
    let sizes = [5, 3, 4, 3, 2];
    let plain = IcsFile::new(&axes, &sizes, 16);
    let (_plain_path, plain_reader) = open(&plain, "gz_plain");
    let gzipped = IcsFile {
        compression: "gzip".to_string(),
        ..IcsFile::new(&axes, &sizes, 16)
    };
    let (_path, reader) = open(&gzipped, "gz");
    for t in 0..2 {
        for c in 0..3 {
            for z in 0..4 {
                let plane = u16_plane(&plain_reader, t, c, z);
                assert_eq!(u16_plane(&reader, t, c, z), plane, "(t={t}, c={c}, z={z})");
            }
        }
    }

    // A truncated stream is an error, never a short plane: cut the pixel data to a third of its length.
    let mut bytes = gzipped.bytes();
    let pixels = gzipped.pixel_bytes();
    let header_len = bytes.len() - pixels.len();
    let keep = header_len + pixels.len() / 3;
    bytes.truncate(keep);
    let path = scratch("gz_truncated");
    std::fs::write(&*path, &bytes).expect("write the truncated ICS");
    let truncated = IcsReader::open(&path).expect("a gzip file has no length to check at open");
    let error = truncated
        .plane(0, 1, 2, 3)
        .expect_err("a truncated stream errors");
    assert!(
        error.to_string().contains("gzip"),
        "the error names the gzip stream: {error}"
    );
    assert!(keep > header_len, "the truncation kept the header");
}

#[test]
fn the_calibration_comes_from_scale_and_units() {
    for (unit, factor, name) in [
        ("micrometers", 1.0, "um"),
        ("µm", 1.0, "micro"),
        ("nanometers", 0.001, "nm"),
        ("millimeters", 1000.0, "mm"),
    ] {
        let file = IcsFile {
            units: vec![unit.to_string(), unit.to_string()],
            scale: vec![0.25, 0.5],
            ..IcsFile::new(&["x", "y", "z"], &[4, 3, 2], 16)
        };
        let (_path, reader) = open(&file, &format!("cal_{name}"));
        let px = reader.meta().scene(0).unwrap().px_size_um.expect("a size");
        assert!((px.x - 0.25 * factor).abs() < 1e-9, "{name}: {}", px.x);
        assert!((px.y - 0.5 * factor).abs() < 1e-9, "{name}: {}", px.y);
        assert!(px.z.is_none(), "{name}: no unit is stated for z");
    }
}

#[test]
fn an_unknown_unit_and_no_units_line_leave_the_axes_uncalibrated() {
    let unknown = IcsFile {
        units: vec!["furlongs".to_string(), "furlongs".to_string()],
        ..IcsFile::new(&["x", "y", "z"], &[4, 3, 2], 16)
    };
    let (_path, reader) = open(&unknown, "unknown_unit");
    let scene = reader.meta().scene(0).unwrap();
    assert!(scene.px_size_um.is_none(), "an unknown unit is not guessed");
    assert_eq!(scene.notes.len(), 1);

    let no_units = IcsFile {
        drop: vec![("parameter", "units")],
        ..IcsFile::new(&["x", "y", "z"], &[4, 3, 2], 16)
    };
    let (_path, reader) = open(&no_units, "no_units");
    assert!(reader.meta().scene(0).unwrap().px_size_um.is_none());
}

#[test]
fn region_equals_cropping_the_plane() {
    let file = IcsFile::new(&["ch", "x", "y", "z", "t"], &[3, 5, 3, 4, 2], 16);
    let (_path, reader) = open(&file, "region");
    let rect = PixelRect::new(1, 2, 3, 1);
    for t in 0..2 {
        for c in 0..3 {
            for z in 0..4 {
                let whole = reader.plane(0, t, c, z).expect("read the plane");
                let window = reader.region(0, t, c, z, rect).expect("read the region");
                assert_eq!(
                    window,
                    whole.crop(rect).expect("crop the plane"),
                    "(t={t}, c={c}, z={z})"
                );
            }
        }
    }
    assert!(
        !reader.supports_region(),
        "`region` stays the trait default"
    );
}

#[test]
fn every_ics_extension_is_detected_and_the_content_wins() {
    for name in ["stack.ics", "stack.ICS", "stack.ids", "stack.Ids"] {
        assert_eq!(detect_from(b"", Some(name)).unwrap(), Format::Ics, "{name}");
    }
    // A `.dat`-named ICS is detected by the bytes 2..13 of the probe.
    let file = IcsFile::stack(4, 3, 1, 1, 1);
    let bytes = file.bytes();
    assert_eq!(detect_from(&bytes, Some("stack.dat")).unwrap(), Format::Ics);
    assert_eq!(detect_from(&bytes, Some("stack.ics")).unwrap(), Format::Ics);
    // A text file named `.ics` is routed by its extension and refused by the ICS reader with the ICS sentence.
    let path = scratch("notes_as_ics");
    std::fs::write(&*path, b"just some notes\n").expect("write the text file");
    match IcsReader::open(&path) {
        Err(error) => assert!(error.to_string().contains("ICS"), "{error}"),
        Ok(_) => panic!("a text file opened as ICS"),
    }
}

#[test]
fn a_dv_or_mrc_file_renamed_ics_is_refused_with_the_ics_sentence() {
    let dv = support::dv::DvFile::i16(4, 3, 1, 1, 1, 0);
    let mrc = support::mrc::MrcFile::i16(4, 3, 1);
    for (bytes, name) in [(dv.bytes(), "dv"), (mrc.bytes(), "mrc")] {
        let path = scratch(&format!("{name}_as_ics"));
        std::fs::write(&*path, &bytes).expect("write the renamed file");
        match IcsReader::open(&path) {
            Err(error) => {
                // The ICS sentence: no `ics_version` in the probe, or no `end` in what follows.
                let text = error.to_string();
                assert!(text.contains("ICS"), "{name}: {text}");
                assert!(
                    text.contains("ics_version") || text.contains("`end`"),
                    "{name}: {text}"
                );
            }
            Ok(_) => panic!("a {name} file opened as ICS"),
        }
    }
}

#[test]
fn open_routes_an_ics_file_to_the_ics_reader() {
    let path = scratch("routed");
    IcsFile::stack(4, 3, 1, 1, 1).write(&path);
    let reader = phasit_imread::open(&path).expect("open through the dispatcher");
    assert_eq!(reader.meta().scene_count(), 1);
    assert_eq!(reader.meta().scene(0).unwrap().dims.x, 4);
    assert_eq!(reader.meta().scene(0).unwrap().dtype, DType::U16);
}

#[test]
fn an_integer_without_a_sign_line_is_unsigned() {
    // The format's default: `sign` absent means unsigned, so 0xFFFF is 65535 and not -1.
    let mut file = IcsFile::new(&["x", "y"], &[2, 1], 16);
    file.drop = vec![("representation", "sign")];
    let mut bytes = file.header();
    assert!(
        !String::from_utf8_lossy(&bytes).contains("sign"),
        "the line is gone"
    );
    bytes.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
    let path = scratch("default_sign");
    std::fs::write(&*path, &bytes).expect("write the unsigned ICS");
    let reader = IcsReader::open(&path).expect("open the ICS without a sign line");
    assert_eq!(reader.meta().scene(0).unwrap().dtype, DType::U16);
    match reader.plane(0, 0, 0, 0).expect("read the plane").data {
        PlaneBuf::U16(values) => assert_eq!(values, vec![65535, 65535]),
        other => panic!("expected U16, got {other:?}"),
    }
}
