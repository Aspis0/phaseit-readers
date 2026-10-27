// SPDX-License-Identifier: Apache-2.0
//! The TIFF variants the reader has to survive, on files that state exactly
//! what they are.
//!
//! Every fixture in `tests/fixtures/tiff/` is written by
//! `gen_tiff_fixtures.py` and described by the manifest beside it: the dims,
//! the dtype, the samples per pixel, the palette and the pixel bytes the
//! reader must publish. A fixture that reads as anything else is a reader
//! that guesses where the file states a fact.

use std::fs;
use std::path::{Path, PathBuf};

use phasit_imread::tiff_reader::TiffReader;
use phasit_imread::{DType, Dims5, ImageReader, Plane, PlaneBuf};
use serde_json::Value;

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tiff")
}

fn manifest() -> Value {
    let path = fixture_dir().join("manifest.json");
    let text = fs::read_to_string(&path).expect("read the fixture manifest");
    serde_json::from_str(&text).expect("parse the fixture manifest")
}

/// The plane's samples as little-endian bytes — the form the fixtures store.
fn plane_bytes(plane: &Plane) -> Vec<u8> {
    match &plane.data {
        PlaneBuf::U8(values) => values.clone(),
        PlaneBuf::U16(values) => values.iter().flat_map(|v| v.to_le_bytes()).collect(),
        PlaneBuf::I16(values) => values.iter().flat_map(|v| v.to_le_bytes()).collect(),
        PlaneBuf::F32(values) => values.iter().flat_map(|v| v.to_le_bytes()).collect(),
    }
}

fn decode_hex(hex: &str) -> Vec<u8> {
    let bytes = hex.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks(2) {
        let text = std::str::from_utf8(pair).expect("hex text");
        out.push(u8::from_str_radix(text, 16).expect("hex digit"));
    }
    out
}

fn dims_of(entry: &Value) -> Dims5 {
    // A single-scene fixture states its dims under "dims"; a multi-scene
    // fixture states them per scene, in the scene entry itself.
    let source = match entry.get("dims") {
        Some(dims) => dims,
        None => entry,
    };
    Dims5 {
        t: source["t"].as_u64().expect("t") as usize,
        c: source["c"].as_u64().expect("c") as usize,
        z: source["z"].as_u64().expect("z") as usize,
        y: source["y"].as_u64().expect("y") as usize,
        x: source["x"].as_u64().expect("x") as usize,
    }
}

fn dtype_of(name: &str) -> DType {
    match name {
        "U8" => DType::U8,
        "U16" => DType::U16,
        "I16" => DType::I16,
        "F32" => DType::F32,
        other => panic!("unknown dtype {other}"),
    }
}

#[test]
fn every_fixture_reads_exactly_as_its_manifest_says() {
    let manifest = manifest();
    let fixtures = manifest["fixtures"]
        .as_array()
        .expect("the manifest lists fixtures");
    assert!(!fixtures.is_empty(), "the manifest lists no fixture");

    for entry in fixtures {
        let file = entry["file"].as_str().expect("file");
        let path = fixture_dir().join(file);
        let refusals: &[Value] = match entry["refused"].as_array() {
            Some(list) => list,
            None => &[],
        };

        // A fixture that states no readable scene must be refused whole, and
        // the refusal must name the scene and the reason the manifest states.
        if entry["scenes"].as_u64().expect("scenes") == 0 {
            let error = match TiffReader::open(&path) {
                Ok(_) => panic!("{file}: the fixture must be refused"),
                Err(error) => error,
            };
            let message = error.to_string();
            for refusal in refusals {
                let name = refusal["name"].as_str().expect("name");
                let reason = refusal["reason"].as_str().expect("reason");
                assert!(message.contains(name), "{file}: the refusal names the scene");
                assert!(message.contains(reason), "{file}: the refusal names the reason");
            }
            continue;
        }

        let reader = TiffReader::open(&path).expect("open the fixture");
        let meta = reader.meta();

        let scenes = entry["scenes"].as_u64().expect("scenes") as usize;
        assert_eq!(meta.scene_count(), scenes, "{file}: scene count");

        // The scenes the manifest says are refused must be refused by name,
        // and the readable ones must keep their own index.
        assert_eq!(
            reader.refused_scenes().len(),
            refusals.len(),
            "{file}: refused scene count"
        );
        for refusal in refusals {
            let index = refusal["image_index"].as_u64().expect("image_index") as usize;
            let refused = reader
                .refused_scenes()
                .iter()
                .find(|scene| scene.image_index == index)
                .unwrap_or_else(|| panic!("{file}: the scene {index} must be refused"));
            assert_eq!(refused.name, refusal["name"].as_str().expect("name"));
            assert!(
                refused.reason.contains(refusal["reason"].as_str().expect("reason")),
                "{file}: the refusal names the reason: {}",
                refused.reason
            );
        }

        // The per-scene facts: a single-scene fixture states them once, a
        // multi-scene fixture states them per scene.
        let scene_entries: Vec<&Value> = if let Some(list) = entry["sceneDims"].as_array() {
            list.iter().collect()
        } else {
            vec![entry]
        };
        assert_eq!(scene_entries.len(), scenes, "{file}: scene entries");

        for (index, expected) in scene_entries.iter().enumerate() {
            let scene = meta.scene(index).expect("the scene");
            assert_eq!(scene.dims, dims_of(expected), "{file}: scene {index} dims");
            let dtype = dtype_of(expected["dtype"].as_str().expect("dtype"));
            assert_eq!(scene.dtype, dtype, "{file}: scene {index} dtype");
            let samples = expected["samples"].as_u64().expect("samples") as u32;
            assert_eq!(scene.samples, samples, "{file}: scene {index} samples");
        }

        // The palette of an indexed scene, when the manifest states one.
        if let Some(entries) = entry["paletteEntries"].as_u64() {
            let palette = meta.scene(0).expect("the scene").palette.as_ref().expect(
                "an indexed scene publishes its palette beside the index values",
            );
            assert_eq!(palette.len(), entries as usize, "{file}: palette length");
            // The fixture's palette is a grey ramp: index i -> (i, i, i).
            for (index, colour) in palette.iter().enumerate() {
                let expected = index as u8;
                assert_eq!(*colour, [expected, expected, expected], "{file}: palette[{index}]");
            }
        } else {
            assert!(
                meta.scene(0).expect("the scene").palette.is_none(),
                "{file}: a scene that states no palette publishes none"
            );
        }

        // The calibration, when the manifest states one.
        if let Some(px) = entry["pxSizeUm"].as_object() {
            let published = meta.scene(0).expect("the scene").px_size_um.expect("a pixel size");
            let want = |key: &str| px[key].as_f64().expect(key);
            assert!(
                (published.x as f64 - want("x")).abs() < 1e-4,
                "{file}: pixel size x"
            );
            assert!(
                (published.y as f64 - want("y")).abs() < 1e-4,
                "{file}: pixel size y"
            );
            assert!(
                (published.z.expect("the Z step") as f64 - want("z")).abs() < 1e-4,
                "{file}: pixel size z"
            );
        }

        // The pixels themselves: every plane the manifest names, byte for byte.
        for plane in entry["planes"].as_array().expect("planes") {
            let scene = plane["scene"].as_u64().unwrap_or(0) as usize;
            let t = plane["t"].as_u64().expect("t") as usize;
            let c = plane["c"].as_u64().expect("c") as usize;
            let z = plane["z"].as_u64().expect("z") as usize;
            let expected = decode_hex(plane["hex"].as_str().expect("hex"));
            let read = reader
                .plane(scene, t, c, z)
                .unwrap_or_else(|e| panic!("{file}: plane ({scene},{t},{c},{z}): {e}"));
            assert_eq!(
                plane_bytes(&read),
                expected,
                "{file}: plane ({scene},{t},{c},{z}) bytes"
            );
        }
    }
}

#[test]
fn an_indexed_scene_keeps_its_index_values_and_publishes_the_palette() {
    // Bio-Formats keeps the index plane and the LUT apart; so does this reader.
    let path = fixture_dir().join("palette-8bit.tif");
    let reader = TiffReader::open(&path).expect("open the fixture");
    let scene = reader.meta().scene(0).expect("the scene");
    assert_eq!(scene.samples, 1, "an indexed pixel is one sample");
    assert!(scene.palette.is_some(), "the palette is published");

    let plane = reader.plane(0, 0, 0, 0).expect("the plane");
    match &plane.data {
        PlaneBuf::U8(values) => {
            assert_eq!(values[0], 0);
            assert_eq!(values[1], 1);
            assert_eq!(values[2], 2);
            assert_eq!(values[3], 3);
        }
        other => panic!("expected U8 index values, got {:?}", other.dtype()),
    }
}

#[test]
fn an_rgb_page_is_one_channel_of_three_components() {
    // Not three gray channels: the samples of a pixel stay together.
    let path = fixture_dir().join("rgb-per-sampleformat.tif");
    let reader = TiffReader::open(&path).expect("open the fixture");
    let scene = reader.meta().scene(0).expect("the scene");
    assert_eq!(scene.dims.c, 1, "one channel");
    assert_eq!(scene.samples, 3, "three components");

    let plane = reader.plane(0, 0, 0, 0).expect("the plane");
    assert_eq!(plane.samples, 3);
    assert_eq!(plane.data.len(), 3 * 4 * 3, "every pixel keeps its three samples");
}

#[test]
fn a_multi_scene_ome_tiff_gives_one_scene_per_image() {
    let path = fixture_dir().join("two-series.ome.tif");
    let reader = TiffReader::open(&path).expect("open the fixture");
    let meta = reader.meta();
    assert_eq!(meta.scene_count(), 2, "one scene per <Image>");
    assert_eq!(meta.scene(0).expect("scene 0").dims.x, 4);
    assert_eq!(meta.scene(1).expect("scene 1").dims.x, 2);
    assert_eq!(meta.scene(0).expect("scene 0").dtype, DType::U16);
    assert_eq!(meta.scene(1).expect("scene 1").dtype, DType::U8);

    // The second scene's plane is the SECOND page of the file, not the first.
    let first = reader.plane(0, 0, 0, 0).expect("scene 0 plane");
    let second = reader.plane(1, 0, 0, 0).expect("scene 1 plane");
    assert_ne!(first.data, second.data, "the two scenes hold different pixels");
    match &second.data {
        PlaneBuf::U8(values) => assert_eq!(values[0], 0),
        other => panic!("expected U8, got {:?}", other.dtype()),
    }
}
