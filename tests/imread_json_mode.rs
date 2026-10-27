// SPDX-License-Identifier: Apache-2.0
//! The JSON mode of `imread`, on the fixtures the crate commits.
//!
//! The mode is the record a sweep is written from: one line per file, with the
//! format, the outcome of the open, every scene and the planes the mode
//! decoded. The fixtures state what they hold (`tests/fixtures/tiff/manifest.json`),
//! so the record can be checked against them — field by field, and hash by
//! hash.
//!
//! The hashes are stated by an implementation that is not this one (Python's
//! `hashlib`), so the mode's hash is checked against the world and not against
//! itself.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// The SHA-256 of every committed fixture plane, as Python's `hashlib` states
/// it: `{file}: {scene:t:c:z}` -> the hash of the plane's bytes.
const PLANE_HASHES: &[(&str, &str)] = &[
    (
        "palette-8bit.tif:0:0:0:0",
        "fff3a9bcdd37363d703c1c4f9512533686157868f0d4f16a0f02d0f1da24f9a2",
    ),
    (
        "rgb-per-sampleformat.tif:0:0:0:0",
        "2db299374a88da15bce285970e06d97cfc843c4edf02deced10de88d8c61c510",
    ),
    (
        "rgba-8bit.tif:0:0:0:0",
        "245fb020e027c049983edccf44b8533c5b29d33b27fa478a47887c8886640d41",
    ),
    (
        "imagej-hyperstack.tif:0:0:0:0",
        "fff3a9bcdd37363d703c1c4f9512533686157868f0d4f16a0f02d0f1da24f9a2",
    ),
    (
        "imagej-hyperstack.tif:0:0:1:0",
        "d7d3fb834e688ef3cea106bfd502c20f2e114e380e50998baaa5fd3f4e68e6c8",
    ),
    (
        "imagej-hyperstack.tif:0:0:0:1",
        "56bd9580f1ab2a53b77eeaa703b9a35863b638d8c7292eba2ad40b53a90dad42",
    ),
    (
        "imagej-hyperstack.tif:0:0:1:1",
        "c6e76a54720a34308cb021ae14b42cb5f6e63d1ea566726770c425538c31bd83",
    ),
    (
        "int16-gray.tif:0:0:0:0",
        "9ffa5029c7b46a5c53f90efb872ef2762241a56910be034e7ea70d76f8509360",
    ),
    (
        "float32-gray.tif:0:0:0:0",
        "2a5f5cae478ad9bf82ca8e245a1a5424e5da993fa98d72aad0023296454f7684",
    ),
    (
        "two-series.ome.tif:0:0:0:0",
        "14927cc0c5f5e59e675d6f8ef42dda466a58b61027827cf22d41068e419d7869",
    ),
    (
        "two-series.ome.tif:1:0:0:0",
        "ba5202aba1ea8fd56c0383fa803f084843f4165e6df2a4c958875f1933c22034",
    ),
    (
        "partly-refused-ome.tif:0:0:0:0",
        "903e095ba03ecfc9e8be2055e24844257bfa09fd7df67d124d4b3e01ce7a145a",
    ),
];

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tiff")
}

fn manifest() -> Value {
    let path = fixture_dir().join("manifest.json");
    let text = std::fs::read_to_string(&path).expect("read the fixture manifest");
    serde_json::from_str(&text).expect("parse the fixture manifest")
}

/// The record the mode printed for one file.
fn record(path: &Path) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_imread"))
        .arg("json")
        .arg(path)
        .output()
        .expect("run the imread binary");
    assert!(
        output.status.success(),
        "imread json {} failed: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).expect("the record is UTF-8");
    let line = text.lines().next().expect("the record is one line");
    serde_json::from_str(line).expect("the record is JSON")
}

/// The record of one fixture.
fn fixture(name: &str) -> Value {
    record(&fixture_dir().join(name))
}

/// The plane of a record, addressed as the mode addresses it.
fn plane_of(record: &Value, scene: usize, c: usize, z: usize) -> &Value {
    record["planes"]
        .as_array()
        .expect("the record has planes")
        .iter()
        .find(|plane| plane["scene"] == scene && plane["c"] == c && plane["z"] == z)
        .unwrap_or_else(|| panic!("the record has no plane {scene}:{c}:{z}"))
}

/// The samples of a fixture plane, decoded as the dtype the manifest states.
fn samples_of(hex: &str, dtype: &str) -> Vec<f64> {
    let raw = hex_to_bytes(hex);
    match dtype {
        "U8" => raw.iter().map(|b| *b as f64).collect(),
        "U16" => raw
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]) as f64)
            .collect(),
        "I16" => raw
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]) as f64)
            .collect(),
        "F32" => raw
            .chunks_exact(4)
            .map(|quad| f32::from_le_bytes([quad[0], quad[1], quad[2], quad[3]]) as f64)
            .collect(),
        other => panic!("unknown dtype {other}"),
    }
}

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    let text = hex.as_bytes();
    text.chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).expect("hex text");
            u8::from_str_radix(pair, 16).expect("hex digit")
        })
        .collect()
}

/// What a plane of a record must state.
struct Expectation {
    scene: usize,
    c: usize,
    z: usize,
    sha256: String,
    min: f64,
    max: f64,
    mean: f64,
}

/// The check one plane of a record is held to.
fn check_plane(record: &Value, expected: &Expectation) -> Result<(), String> {
    let plane = plane_of(record, expected.scene, expected.c, expected.z);
    if plane["sha256"] != expected.sha256.as_str() {
        return Err(format!(
            "plane {}:{}:{} hashes {} and not {}",
            expected.scene, expected.c, expected.z, plane["sha256"], expected.sha256
        ));
    }
    for (field, value) in [
        ("min", expected.min),
        ("max", expected.max),
        ("mean", expected.mean),
    ] {
        let stated = plane[field].as_f64().unwrap_or(f64::NAN);
        if (stated - value).abs() > 1e-9 {
            return Err(format!(
                "plane {}:{}:{} states {field} {stated} and not {value}",
                expected.scene, expected.c, expected.z
            ));
        }
    }
    Ok(())
}

#[test]
fn an_opened_fixture_states_its_format_and_its_scene() {
    let record = fixture("palette-8bit.tif");
    assert_eq!(record["status"], "opened");
    assert_eq!(record["format"], "Tiff");
    assert_eq!(record["error"], Value::Null);
    assert_eq!(record["scene_count"], 1);

    let scene = &record["scenes"][0];
    assert_eq!(scene["index"], 0);
    assert_eq!(scene["file_index"], 0);
    assert_eq!(scene["name"], "palette-8bit");
    assert_eq!(scene["dims"]["t"], 1);
    assert_eq!(scene["dims"]["c"], 1);
    assert_eq!(scene["dims"]["z"], 1);
    assert_eq!(scene["dims"]["y"], 3);
    assert_eq!(scene["dims"]["x"], 4);
    assert_eq!(scene["dtype"], "U8");
    assert_eq!(scene["samples"], 1);
    assert_eq!(scene["px_size_um"], Value::Null);
    assert_eq!(scene["channels"].as_array().expect("channels").len(), 1);
}

#[test]
fn a_multiscene_fixture_states_every_scene() {
    let record = fixture("two-series.ome.tif");
    assert_eq!(record["status"], "opened");
    assert_eq!(record["format"], "OmeTiff");
    assert_eq!(record["scene_count"], 2);

    let first = &record["scenes"][0];
    assert_eq!(first["name"], "first scene");
    assert_eq!(first["file_index"], 0);
    assert_eq!(first["dtype"], "U16");
    assert_eq!(first["px_size_um"]["x"], 0.5);
    assert_eq!(first["channels"][0]["name"], "DAPI");

    let second = &record["scenes"][1];
    assert_eq!(second["name"], "second scene");
    assert_eq!(second["file_index"], 1);
    assert_eq!(second["dtype"], "U8");
    assert_eq!(second["dims"]["y"], 2);
}

#[test]
fn every_fixture_plane_hashes_and_measures_as_its_bytes_state() {
    for entry in manifest()["fixtures"].as_array().expect("fixtures") {
        let file = entry["file"].as_str().expect("the fixture's name");
        let record = fixture(file);
        if entry["scenes"].as_u64().expect("the scene count") == 0 {
            // The fixture states that the reader refuses it; its own test says
            // so, and it has no plane to check.
            assert_eq!(record["status"], "refused", "{file} must be refused");
            continue;
        }
        assert_eq!(record["status"], "opened", "{file} must open");
        for plane in entry
            .get("planes")
            .and_then(|planes| planes.as_array())
            .unwrap_or(&Vec::new())
        {
            let scene = plane["scene"].as_u64().unwrap_or(0) as usize;
            let c = plane["c"].as_u64().expect("c") as usize;
            let z = plane["z"].as_u64().expect("z") as usize;
            let dtype = dtype_of(entry, scene);
            let samples = samples_of(plane["hex"].as_str().expect("hex"), dtype);
            let key = format!("{file}:{scene}:0:{c}:{z}");
            let sha256 = PLANE_HASHES
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, hash)| (*hash).to_string())
                .unwrap_or_else(|| panic!("no stated hash for {key}"));
            let sum: f64 = samples.iter().sum();
            let expectation = Expectation {
                scene,
                c,
                z,
                sha256,
                min: samples.iter().cloned().fold(f64::INFINITY, f64::min),
                max: samples.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
                mean: sum / samples.len() as f64,
            };
            check_plane(&record, &expectation)
                .unwrap_or_else(|failure| panic!("{file}: {failure}"));
        }
    }
}

/// The dtype of a fixture's scene: a single-scene fixture states it once, a
/// multi-scene one states it per scene.
fn dtype_of(entry: &Value, scene: usize) -> &str {
    match entry.get("sceneDims") {
        Some(dims) => dims[scene]["dtype"].as_str().expect("the dtype"),
        None => entry["dtype"].as_str().expect("the dtype"),
    }
}

#[test]
fn a_refused_fixture_reports_refused_with_a_reason() {
    let record = fixture("refused-ome.tif");
    assert_eq!(record["status"], "refused");
    assert_eq!(record["error"]["kind"], "refused");
    let message = record["error"]["message"].as_str().expect("a message");
    assert!(
        message.contains("not modelled by phasit-imread"),
        "the message names the cause: {message}"
    );
    let refused = record["refused_scenes"].as_array().expect("refused scenes");
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0]["image_index"], 0);
    assert_eq!(refused[0]["name"], "unreadable scene");
    assert!(refused[0]["reason"]
        .as_str()
        .expect("a reason")
        .contains("not modelled by phasit-imread"));
    assert_eq!(record["scenes"].as_array().expect("scenes").len(), 0);
}

#[test]
fn a_partly_refused_fixture_names_the_refused_scene_and_keeps_the_readable_one() {
    let record = fixture("partly-refused-ome.tif");
    assert_eq!(record["status"], "opened");
    assert_eq!(record["scene_count"], 1);
    assert_eq!(record["scenes"][0]["name"], "readable scene");
    assert_eq!(record["scenes"][0]["file_index"], 0);

    let refused = record["refused_scenes"].as_array().expect("refused scenes");
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0]["image_index"], 1);
    assert_eq!(refused[0]["name"], "unreadable scene");
}

#[test]
fn a_missing_file_reports_an_error() {
    let path = fixture_dir().join("there-is-no-such-fixture.tif");
    let record = record(&path);
    assert_eq!(record["status"], "error");
    assert_eq!(record["error"]["kind"], "io");
    assert!(record["error"]["message"]
        .as_str()
        .expect("a message")
        .contains("cannot find"));
    assert_eq!(record["scenes"].as_array().expect("scenes").len(), 0);
}

#[test]
fn the_plane_check_is_not_vacuous() {
    // The break log: the same check, with one byte of the stated hash
    // changed. The check must fail on it, and say why.
    let record = fixture("palette-8bit.tif");
    let broken = Expectation {
        scene: 0,
        c: 0,
        z: 0,
        sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
        min: 0.0,
        max: 11.0,
        mean: 5.5,
    };
    let failure = check_plane(&record, &broken).expect_err("the broken hash must fail");
    eprintln!("break log: {failure}");
    assert!(
        failure.contains("hashes"),
        "the failure names the field: {failure}"
    );
}
