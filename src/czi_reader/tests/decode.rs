use crate::error::Error;
use crate::types::{DType, Dims5, ImageReader, PlaneBuf};

use super::super::bytes::segment_id_is;
use super::super::codec::{bgr_to_rgb, deinterleave_hilo16, pixel_type_to_dtype, zstd1_decode, zstd_raw_decode};
use super::super::metadata::parse_metadata;
use super::super::mosaic::blit_tile;
use super::support::*;

#[test]
fn pixel_type_mapping() {
    assert_eq!(pixel_type_to_dtype(0).unwrap(), (DType::U8, 1));
    assert_eq!(pixel_type_to_dtype(1).unwrap(), (DType::U16, 1));
    assert_eq!(pixel_type_to_dtype(2).unwrap(), (DType::F32, 1));
    assert_eq!(pixel_type_to_dtype(3).unwrap(), (DType::U8, 3));
    assert_eq!(pixel_type_to_dtype(4).unwrap(), (DType::U16, 3));
    assert!(pixel_type_to_dtype(10).is_err());
    assert!(pixel_type_to_dtype(13).is_err());
}

#[test]
fn bgr_to_rgb_swaps_b_and_r() {
    assert_eq!(bgr_to_rgb(vec![1u8, 2, 3, 4, 5, 6], 1), vec![3, 2, 1, 6, 5, 4]);
    // Bgr48: whole 2-byte samples move, so B and R swap while G stays.
    assert_eq!(
        bgr_to_rgb(vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06], 2),
        vec![0x05, 0x06, 0x03, 0x04, 0x01, 0x02]
    );
    assert_eq!(bgr_to_rgb(vec![9, 8], 1), vec![9, 8]);
}

#[test]
fn segment_id_matches_null_padded() {
    let mut buf = vec![0u8; 16];
    buf[..10].copy_from_slice(b"ZISRAWFILE");
    assert!(segment_id_is(&buf, 0, b"ZISRAWFILE"));
    assert!(!segment_id_is(&buf, 0, b"ZISRAWDIRECTORY"));
    buf[10] = 0x41;
    assert!(!segment_id_is(&buf, 0, b"ZISRAWFILE"));
}

#[test]
fn hilo_deinterleave_roundtrips_to_le() {
    let raw = [0x01, 0x03, 0x02, 0x04];
    assert_eq!(deinterleave_hilo16(&raw), vec![0x01, 0x02, 0x03, 0x04]);
}

// Byte-exact imagecodecs fixtures, the codec czifile uses: zstd0 is compression id 5, zstd1 and zstd1hilo are id 6.
// Each `.bin` is decoded as decode_tile dispatches it and compared with the `.raw` ground truth.
#[test]
fn zstd_fixtures_match_ground_truth() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/zstd");
    let manifest = std::fs::read_to_string(format!("{dir}/manifest.json"))
        .expect("read zstd manifest.json");
    let entries = parse_manifest(&manifest);
    assert!(!entries.is_empty(), "manifest parsed no entries");

    let mut hilo_seen = 0usize;
    for (name, variant, itemsize) in &entries {
        let bin = std::fs::read(format!("{dir}/{name}.bin"))
            .unwrap_or_else(|e| panic!("read {name}.bin: {e}"));
        let raw = std::fs::read(format!("{dir}/{name}.raw"))
            .unwrap_or_else(|e| panic!("read {name}.raw: {e}"));
        let expected = raw.len();
        let out = match variant.as_str() {
            "zstd0" => zstd_raw_decode(&bin, expected),
            "zstd1" | "zstd1hilo" => zstd1_decode(&bin, expected, *itemsize),
            other => panic!("unknown variant {other} for {name}"),
        }
        .unwrap_or_else(|e| panic!("decode {name} ({variant}): {e}"));
        assert_eq!(out, raw, "{name} ({variant}) decoded bytes != ground truth");
        if variant == "zstd1hilo" {
            hilo_seen += 1;
        }
    }
    assert!(hilo_seen >= 1, "no Hi/Lo fixture was exercised");
}

#[test]
fn the_entities_of_an_attribute_and_of_a_text_node_are_resolved() {
    // The attribute and the text node both state entities; the reader must resolve them.
    let xml = br#"<ImageDocument>
      <Dimensions><Channel Name="A&amp;B &#233;"/></Dimensions>
      <Distance Id="X"><Value>2&#46;5e-7</Value></Distance>
      <Distance Id="Y"><Value>5&#46;0e-7</Value></Distance>
    </ImageDocument>"#;
    let (names, calibration) =
        parse_metadata(&metadata_segment(xml), 0).expect("the metadata segment is read");
    assert_eq!(names, vec!["A&B é".to_string()]);
    let px = calibration.px_size_um.expect("the file states a pixel size");
    assert_eq!(px.x, 0.25);
    assert_eq!(px.y, 0.5);
}

#[test]
fn a_cdata_section_states_its_text_as_it_is() {
    let xml = br#"<ImageDocument>
      <Dimensions><Channel Name="A"/></Dimensions>
      <Distance Id="X"><Value><![CDATA[2.5e-7]]></Value></Distance>
      <Distance Id="Y"><Value><![CDATA[5.0e-7]]></Value></Distance>
    </ImageDocument>"#;
    let (_, calibration) =
        parse_metadata(&metadata_segment(xml), 0).expect("the metadata segment is read");
    let px = calibration.px_size_um.expect("the file states a pixel size");
    assert_eq!(px.x, 0.25);
    assert_eq!(px.y, 0.5);
}

#[test]
fn an_entity_that_cannot_be_resolved_is_read_as_it_was_written() {
    // An unknown entity is taken as written: it must neither panic nor take the segment down.
    let xml = br#"<ImageDocument>
      <Dimensions><Channel Name="A&bogus;B"/></Dimensions>
    </ImageDocument>"#;
    let (names, _) =
        parse_metadata(&metadata_segment(xml), 0).expect("the metadata segment is read");
    assert_eq!(names, vec!["A&bogus;B".to_string()]);
}

#[test]
fn synthetic_bgr24_single_tile_decodes_rgb() {
    let bgr = vec![
        1, 2, 3,
        4, 5, 6,
        7, 8, 9,
        10, 11, 12,
    ];
    let tiles = vec![TileSpec {
        pixel_type: 3,
        xs: 0,
        xsz: 2,
        ys: 0,
        ysz: 2,
        c: 0,
        z: 0,
        m: 0,
        pixels: bgr,
    }];
    let reader = open_synth(&build_czi(&tiles));
    let scene = reader.meta().scene(0).unwrap();
    // Color is NOT exploded into C: C stays 1, samples-per-pixel is 3.
    assert_eq!(scene.dims, Dims5 { t: 1, c: 1, z: 1, y: 2, x: 2 });
    assert_eq!(scene.dtype, DType::U8);
    assert_eq!(scene.samples, 3);

    let p = reader.plane(0, 0, 0, 0).unwrap();
    assert_eq!(p.shape, [2, 2]);
    assert_eq!(p.samples(), 3);
    match p.data {
        PlaneBuf::U8(v) => {
            assert_eq!(v.len(), 2 * 2 * 3, "interleaved RGB byte length");
            assert_eq!(v, vec![3, 2, 1, 6, 5, 4, 9, 8, 7, 12, 11, 10]);
        }
        other => panic!("expected U8, got {:?}", other.dtype()),
    }
}

// A stride bug (width * itemsize, not width * samples * itemsize) would scramble these colour rows.
#[test]
fn synthetic_bgr24_mosaic_uses_samples_aware_stride() {
    let tile_a = vec![
        10, 11, 12, 13, 14, 15,
        16, 17, 18, 19, 20, 21,
    ];
    let tile_b = vec![
        30, 31, 32, 33, 34, 35,
        36, 37, 38, 39, 40, 41,
    ];
    let tiles = vec![
        TileSpec { pixel_type: 3, xs: 0, xsz: 2, ys: 0, ysz: 2, c: 0, z: 0, m: 0, pixels: tile_a },
        TileSpec { pixel_type: 3, xs: 2, xsz: 2, ys: 0, ysz: 2, c: 0, z: 0, m: 1, pixels: tile_b },
    ];
    let reader = open_synth(&build_czi(&tiles));
    let scene = reader.meta().scene(0).unwrap();
    assert_eq!(scene.dims, Dims5 { t: 1, c: 1, z: 1, y: 2, x: 4 });
    assert_eq!(scene.samples, 3);

    let p = reader.plane(0, 0, 0, 0).unwrap();
    assert_eq!(p.shape, [2, 4]);
    assert_eq!(p.samples(), 3);
    match p.data {
        PlaneBuf::U8(v) => {
            assert_eq!(v.len(), 2 * 4 * 3);
            assert_eq!(
                v,
                vec![
                    12, 11, 10, 15, 14, 13, 32, 31, 30, 35, 34, 33,
                    18, 17, 16, 21, 20, 19, 38, 37, 36, 41, 40, 39,
                ]
            );
        }
        other => panic!("expected U8, got {:?}", other.dtype()),
    }
}

// blit_tile takes i64 offsets: a column beyond i32::MAX is out of bounds, without a panic.
#[test]
fn blit_tile_rejects_offset_past_i32_max() {
    let mut composed = vec![0u8; 4];
    let tile = vec![0u8; 4];
    let res = blit_tile(
        &mut composed,
        2,
        2,
        2,
        1,
        &tile,
        2,
        2,
        i64::from(i32::MAX) + 1,
        0,
    );
    assert!(matches!(res, Err(Error::Decode(_))));
}
