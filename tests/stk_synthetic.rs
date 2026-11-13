// SPDX-License-Identifier: Apache-2.0
//! The MetaMorph STK reader on synthetic files: both byte orders, both sample widths, several contiguous strips, the
//! UIC1 calibration, every plane compared with the samples the builder writes, and the detection routes. The files are
//! written by `tests/support/stk.rs` and removed by `Scratch`.

mod support;

use phasit_imread::stk_reader::StkReader;
use phasit_imread::types::{Dims5, PixelRect};
use phasit_imread::{detect_from, DType, Error, Format, ImageReader, PlaneBuf, PxSizeUm};

use support::scratch::Scratch;
use support::stk::{sample_at, StkFile, Uic1Value, Uic2Type};

fn scratch(name: &str, extension: &str) -> Scratch {
    Scratch::new(std::env::temp_dir().join(format!(
        "phasit_stk_{name}_{}.{}",
        std::process::id(),
        extension
    )))
}

fn open(file: &StkFile, name: &str) -> (Scratch, StkReader) {
    let path = scratch(name, "stk");
    file.write(&path);
    let reader = StkReader::open(&path).expect("open the synthetic STK");
    (path, reader)
}

/// The samples the builder writes for plane `plane`, x fastest.
fn expected(width: u32, height: u32, plane: usize) -> Vec<u16> {
    (0..height)
        .flat_map(|y| (0..width).map(move |x| sample_at(plane, y, x)))
        .collect()
}

fn u8_plane(reader: &StkReader, z: usize) -> Vec<u8> {
    match reader.plane(0, 0, 0, z).expect("read the plane").data {
        PlaneBuf::U8(values) => values,
        other => panic!("expected U8, got {other:?}"),
    }
}

fn u16_plane(reader: &StkReader, z: usize) -> Vec<u16> {
    match reader.plane(0, 0, 0, z).expect("read the plane").data {
        PlaneBuf::U16(values) => values,
        other => panic!("expected U16, got {other:?}"),
    }
}

/// The samples of a plane of a reader this test does not own the type of.
fn plane_u16(reader: &dyn ImageReader, z: usize) -> Vec<u16> {
    match reader.plane(0, 0, 0, z).expect("read the plane").data {
        PlaneBuf::U16(values) => values,
        other => panic!("expected U16, got {other:?}"),
    }
}

fn plane_u8(reader: &dyn ImageReader, z: usize) -> Vec<u8> {
    match reader.plane(0, 0, 0, z).expect("read the plane").data {
        PlaneBuf::U8(values) => values,
        other => panic!("expected U8, got {other:?}"),
    }
}

#[test]
fn a_little_endian_u8_stack_decodes_every_plane() {
    let file = StkFile::u8(5, 3, 4);
    let (_path, reader) = open(&file, "le_u8");
    let scene = reader.meta().scene(0).unwrap();
    assert_eq!(
        scene.dims,
        Dims5 {
            t: 1,
            c: 1,
            z: 4,
            y: 3,
            x: 5
        }
    );
    assert_eq!(scene.dtype, DType::U8);
    assert_eq!(scene.samples, 1);
    for z in 0..4 {
        let want: Vec<u8> = expected(5, 3, z).iter().map(|value| *value as u8).collect();
        assert_eq!(u8_plane(&reader, z), want, "plane {z}");
        assert_eq!(reader.plane(0, 0, 0, z).unwrap().shape, [3, 5]);
    }
}

#[test]
fn a_big_endian_u16_stack_decodes_every_plane() {
    let file = StkFile {
        big_endian: true,
        ..StkFile::u16(5, 3, 4)
    };
    let (_path, reader) = open(&file, "be_u16");
    assert_eq!(reader.meta().scene(0).unwrap().dtype, DType::U16);
    for z in 0..4 {
        assert_eq!(u16_plane(&reader, z), expected(5, 3, z), "plane {z}");
    }
}

#[test]
fn plane_0_in_several_contiguous_strips_reads_the_same_samples() {
    let file = StkFile {
        rows_per_strip: Some(1),
        ..StkFile::u16(5, 3, 2)
    };
    let (_path, reader) = open(&file, "strips");
    assert_eq!(u16_plane(&reader, 0), expected(5, 3, 0));
    assert_eq!(u16_plane(&reader, 1), expected(5, 3, 1));
}

#[test]
fn a_uic2_rational_plane_table_states_the_same_plane_count() {
    let file = StkFile {
        uic2_type: Uic2Type::Rational,
        ..StkFile::u16(5, 3, 4)
    };
    let (_path, reader) = open(&file, "uic2_rational");
    assert_eq!(reader.meta().scene(0).unwrap().dims.z, 4);
    assert_eq!(u16_plane(&reader, 3), expected(5, 3, 3));
}

#[test]
fn the_calibration_is_published_in_micrometres() {
    let cases = [
        ("um", Uic1Value::Rational(1, 2), Uic1Value::Rational(1, 4)),
        (
            "micron",
            Uic1Value::Rational(1, 2),
            Uic1Value::Rational(1, 4),
        ),
        (
            "nm",
            Uic1Value::Rational(500, 1),
            Uic1Value::Rational(250, 1),
        ),
    ];
    for (units, x, y) in cases {
        let file = StkFile {
            uic1: Some(vec![
                (4, x),
                (5, y),
                (6, Uic1Value::Text(units.to_string())),
            ]),
            ..StkFile::u16(5, 3, 2)
        };
        let (_path, reader) = open(&file, "calibration");
        let scene = reader.meta().scene(0).unwrap();
        assert_eq!(
            scene.px_size_um,
            Some(PxSizeUm {
                z: None,
                y: 0.25,
                x: 0.5
            }),
            "{units}"
        );
        assert_eq!(scene.notes, Vec::<String>::new(), "{units}");
    }
}

#[test]
fn the_calibration_is_omitted_for_a_unit_this_reader_does_not_convert() {
    let file = StkFile {
        uic1: Some(vec![
            (4, Uic1Value::Rational(1, 2)),
            (5, Uic1Value::Rational(1, 4)),
            (6, Uic1Value::Text("inches".to_string())),
        ]),
        ..StkFile::u16(5, 3, 2)
    };
    let (_path, reader) = open(&file, "unknown_unit");
    let scene = reader.meta().scene(0).unwrap();
    assert_eq!(scene.px_size_um, None);
    assert_eq!(
        scene.notes.len(),
        1,
        "the scene says why it has no pixel size"
    );
}

#[test]
fn the_calibration_is_omitted_without_a_units_entry() {
    let file = StkFile {
        uic1: Some(vec![
            (4, Uic1Value::Rational(1, 2)),
            (5, Uic1Value::Rational(1, 4)),
        ]),
        ..StkFile::u16(5, 3, 2)
    };
    let (_path, reader) = open(&file, "no_units");
    assert_eq!(reader.meta().scene(0).unwrap().px_size_um, None);
}

#[test]
fn the_calibration_is_omitted_without_uic1() {
    let file = StkFile {
        uic1: None,
        ..StkFile::u16(5, 3, 2)
    };
    let (_path, reader) = open(&file, "no_uic1");
    assert_eq!(reader.meta().scene(0).unwrap().px_size_um, None);
}

#[test]
fn an_entry_outside_the_file_is_ignored() {
    let file = StkFile {
        uic1: Some(vec![
            (4, Uic1Value::Rational(1, 2)),
            (5, Uic1Value::PastEnd),
            (6, Uic1Value::Text("um".to_string())),
        ]),
        ..StkFile::u16(5, 3, 2)
    };
    let (_path, reader) = open(&file, "entry_past_end");
    assert_eq!(reader.meta().scene(0).unwrap().px_size_um, None);
    assert_eq!(u16_plane(&reader, 1), expected(5, 3, 1));
}

#[test]
fn region_equals_cropping_the_plane() {
    let (_path, reader) = open(&StkFile::u16(5, 3, 4), "region");
    let rect = PixelRect::new(2, 1, 3, 2);
    assert_eq!(
        reader.region(0, 0, 0, 2, rect).unwrap(),
        reader.plane(0, 0, 0, 2).unwrap().crop(rect).unwrap()
    );
    assert!(!reader.supports_region(), "region is the trait default");
}

#[test]
fn a_coordinate_out_of_range_is_an_error() {
    let (_path, reader) = open(&StkFile::u16(5, 3, 4), "range");
    assert!(matches!(
        reader.plane(0, 0, 0, 4),
        Err(Error::OutOfRange { z: 4, .. })
    ));
    assert!(matches!(
        reader.plane(0, 1, 0, 0),
        Err(Error::OutOfRange { t: 1, .. })
    ));
    assert!(matches!(
        reader.plane(0, 0, 1, 0),
        Err(Error::OutOfRange { c: 1, .. })
    ));
    assert!(matches!(
        reader.plane(1, 0, 0, 0),
        Err(Error::SceneOutOfRange { scene: 1, count: 1 })
    ));
}

#[test]
fn every_stk_name_is_detected_and_a_tif_named_stk_opens_as_an_stack() {
    for extension in ["stk", "STK"] {
        let name = format!("stack.{extension}");
        assert_eq!(
            detect_from(b"II\x2A\x00", Some(&name)).unwrap(),
            Format::Stk,
            "{name}"
        );
        // The name alone is not an STK: the format is a classic TIFF, so a header with no TIFF magic is not one.
        assert!(
            matches!(detect_from(b"", Some(&name)), Err(Error::Unsupported(_))),
            "{name}"
        );
    }
    // The 16-byte probe cannot reach the first IFD, so the UIC2 decides only when the file is opened.
    let path = scratch("tif_named", "tif");
    StkFile::u16(5, 3, 4).write(&path);
    let bytes = std::fs::read(&*path).unwrap();
    assert_eq!(
        detect_from(&bytes, Some("stack.tif")).unwrap(),
        Format::Tiff
    );
    let reader = phasit_imread::open(&path).expect("the .tif-named STK opens");
    assert_eq!(reader.meta().scene(0).unwrap().dims.z, 4);
    assert_eq!(plane_u16(reader.as_ref(), 3), expected(5, 3, 3));
}

#[test]
fn a_plain_tiff_is_not_routed_to_the_stk_reader() {
    let path = scratch("plain", "tif");
    support::write_grey(
        &path,
        None,
        &[
            support::GreyPage {
                width: 4,
                height: 3,
                value: 7,
            },
            support::GreyPage {
                width: 4,
                height: 3,
                value: 9,
            },
        ],
    );
    let reader = phasit_imread::open(&path).expect("the plain TIFF opens");
    assert_eq!(reader.meta().scene(0).unwrap().dims.z, 2);
    assert_eq!(plane_u8(reader.as_ref(), 1), vec![9u8; 12]);
}

#[test]
fn an_stk_without_uic2_is_refused_with_the_stk_sentence() {
    let path = scratch("no_uic2", "stk");
    support::write_grey(
        &path,
        None,
        &[support::GreyPage {
            width: 4,
            height: 3,
            value: 7,
        }],
    );
    match phasit_imread::open(&path) {
        Err(Error::Unsupported(message)) => assert!(message.contains("UIC2"), "{message}"),
        Err(other) => panic!("expected Unsupported, got {other:?}"),
        Ok(_) => panic!("the file opened"),
    }
}

#[test]
fn a_uic1_table_that_repeats_one_long_unit_name_opens_at_once() {
    // The audit's hostile file: 16 384 pairs, every one an id-6 entry pointing at one 1 MiB name. A per-pair copy of
    // the name prices the open by pairs x name; reading it once, capped, does not.
    let file = StkFile {
        uic1: Some(vec![(6, Uic1Value::Text("a".repeat(1 << 20)))]),
        uic1_repeat: 1 << 14,
        ..StkFile::u16(5, 3, 2)
    };
    let path = scratch("units_spam", "stk");
    file.write(&path);

    let started = std::time::Instant::now();
    let reader = StkReader::open(&path).expect("open the hostile UIC1");
    let elapsed = started.elapsed();
    let scene = reader.meta().scene(0).unwrap();
    assert_eq!(scene.dims.z, 2);
    assert_eq!(
        scene.px_size_um, None,
        "a 1 MiB name is no unit this reader knows"
    );
    println!("open of the hostile UIC1 took {elapsed:?}");
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "the open took {elapsed:?}"
    );
}

/// A one-plane OME-XML description: enough for the TIFF reader to name a scene.
const OME_XML: &str = r#"<?xml version="1.0"?>
<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
  <Image Name="grey"><Pixels DimensionOrder="XYCZT" Type="uint8" SizeX="2" SizeY="2" SizeZ="1" SizeC="1" SizeT="1"/></Image>
</OME>"#;

#[test]
fn detect_format_and_open_agree_on_every_route() {
    // A .tif-named STK: the probe cannot see the first IFD, so both must name an STK, not one plane of four.
    let stk = scratch("routes_stk", "tif");
    StkFile::u16(5, 3, 4).write(&stk);
    assert_eq!(phasit_imread::detect_format(&stk).unwrap(), Format::Stk);
    assert_eq!(
        phasit_imread::open(&stk)
            .unwrap()
            .meta()
            .scene(0)
            .unwrap()
            .dims
            .z,
        4
    );

    // The .stk name, a plain two-page TIFF and an OME-TIFF keep what the probe says.
    let named = scratch("routes_named", "stk");
    StkFile::u16(5, 3, 2).write(&named);
    assert_eq!(phasit_imread::detect_format(&named).unwrap(), Format::Stk);
    assert_eq!(
        phasit_imread::open(&named)
            .unwrap()
            .meta()
            .scene(0)
            .unwrap()
            .dims
            .z,
        2
    );

    let plain = scratch("routes_plain", "tif");
    support::write_grey(
        &plain,
        None,
        &[
            support::GreyPage {
                width: 4,
                height: 3,
                value: 7,
            },
            support::GreyPage {
                width: 4,
                height: 3,
                value: 9,
            },
        ],
    );
    assert_eq!(phasit_imread::detect_format(&plain).unwrap(), Format::Tiff);
    assert_eq!(
        phasit_imread::open(&plain)
            .unwrap()
            .meta()
            .scene(0)
            .unwrap()
            .dims
            .z,
        2
    );

    let ome = scratch("routes_ome", "ome.tif");
    support::write_grey(
        &ome,
        Some(OME_XML),
        &[support::GreyPage {
            width: 2,
            height: 2,
            value: 5,
        }],
    );
    assert_eq!(phasit_imread::detect_format(&ome).unwrap(), Format::OmeTiff);
    assert_eq!(phasit_imread::open(&ome).unwrap().meta().scene_count(), 1);
}
