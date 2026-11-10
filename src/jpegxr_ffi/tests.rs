use super::decode_jpegxr;
use crate::types::DType;
use std::path::PathBuf;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jpegxr")
}

/// Minimal manifest parser: the only nested brackets are `shape` arrays, so splitting on `}` works.
fn parse_manifest(txt: &str) -> Vec<(String, DType, u32, usize, usize)> {
    let mut out = Vec::new();
    for chunk in txt.split('{').skip(1) {
        let name = str_field(chunk, "name");
        let dtype = str_field(chunk, "dtype");
        let (Some(name), Some(dtype)) = (name, dtype) else {
            continue;
        };
        let dtype = match dtype.as_str() {
            "u8" => DType::U8,
            "u16" => DType::U16,
            "f32" => DType::F32,
            other => panic!("unknown dtype in manifest: {other}"),
        };
        let samples = uint_field(chunk, "samples").unwrap_or(1) as u32;
        let stored_x = uint_field(chunk, "stored_x").expect("stored_x");
        let stored_y = uint_field(chunk, "stored_y").expect("stored_y");
        out.push((name, dtype, samples, stored_x, stored_y));
    }
    out
}

fn str_field(chunk: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let i = chunk.find(&pat)?;
    let rest = &chunk[i + pat.len()..];
    let colon = rest.find(':')?;
    let after = &rest[colon + 1..];
    let q1 = after.find('"')?;
    let after = &after[q1 + 1..];
    let q2 = after.find('"')?;
    Some(after[..q2].to_string())
}

fn uint_field(chunk: &str, key: &str) -> Option<usize> {
    let pat = format!("\"{key}\"");
    let i = chunk.find(&pat)?;
    let rest = &chunk[i + pat.len()..];
    let colon = rest.find(':')?;
    let after = rest[colon + 1..].trim_start();
    let end = after
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(after.len());
    after[..end].parse().ok()
}

/// Decodes every listed tile and checks it against its `.raw` ground truth; returns the count.
fn verify_fixture_dir(dir: &std::path::Path, manifest_name: &str) -> usize {
    let manifest = std::fs::read_to_string(dir.join(manifest_name))
        .unwrap_or_else(|e| panic!("read {manifest_name}: {e}"));
    let entries = parse_manifest(&manifest);
    assert!(!entries.is_empty(), "{manifest_name} parsed no entries");
    let n = entries.len();

    for (name, dtype, samples, stored_x, stored_y) in entries {
        let jxr = std::fs::read(dir.join(format!("{name}.jxr")))
            .unwrap_or_else(|e| panic!("read {name}.jxr: {e}"));
        let expected = std::fs::read(dir.join(format!("{name}.raw")))
            .unwrap_or_else(|e| panic!("read {name}.raw: {e}"));

        let got = decode_jpegxr(&jxr, stored_x, stored_y, dtype, samples)
            .unwrap_or_else(|e| panic!("decode_jpegxr({name}) failed: {e}"));

        assert_eq!(
            got.len(),
            expected.len(),
            "{name}: decoded length {} != ground-truth {}",
            got.len(),
            expected.len()
        );
        assert!(
            got == expected,
            "{name}: decoded bytes differ from ground truth (dtype {dtype:?}, \
             samples {samples}, {stored_x}x{stored_y})"
        );
    }
    n
}

#[test]
fn jpegxr_fixtures_match_ground_truth() {
    verify_fixture_dir(&fixtures_dir(), "manifest.json");

    // Real Zeiss JPEG-XR tiles are opt-in and not committed (derived from a third-party sample), extracted by
    // `tests/parity/extract_jxr_tiles.py`. Absent and not required is a printed skip; absent and required fails.
    let real =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jpegxr_real");
    if crate::real_data::reference_dump(&real.join("manifest_real.json")).is_none() {
        return;
    }
    let n = verify_fixture_dir(&real, "manifest_real.json");
    eprintln!("verified {n} real Axioscan JPEG-XR tiles against imagecodecs");
}
