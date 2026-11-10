use crate::error::Error;
use crate::types::ImageReader;

use super::chunks::{parse_chunkmap_blob, JP2_MAGIC, SIG_CHUNKMAP_END};
use super::clx::{clx_type, ClxValue};
use super::clx_parse::parse_clx_variant;
use super::coords::{axis_count, scene_count, seq_index};
use super::experiment::{load_experiment, Axis, Loop};
use super::Nd2Reader;
use std::io::Write;

fn utf16_name(s: &str) -> (u8, Vec<u8>) {
    let mut units: Vec<u16> = s.encode_utf16().collect();
    units.push(0);
    let name_length = units.len() as u8;
    let mut bytes = Vec::with_capacity(units.len() * 2);
    for u in units {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    (name_length, bytes)
}

fn rec_scalar(data_type: u8, name: &str, value: &[u8]) -> Vec<u8> {
    let (nl, nb) = utf16_name(name);
    let mut r = vec![data_type, nl];
    r.extend(nb);
    r.extend_from_slice(value);
    r
}

fn rec_u32(name: &str, v: u32) -> Vec<u8> {
    rec_scalar(clx_type::UINT32, name, &v.to_le_bytes())
}

fn rec_f64(name: &str, v: f64) -> Vec<u8> {
    rec_scalar(clx_type::DOUBLE, name, &v.to_le_bytes())
}

fn rec_str(name: &str, s: &str) -> Vec<u8> {
    let mut value: Vec<u8> = Vec::new();
    for u in s.encode_utf16() {
        value.extend_from_slice(&u.to_le_bytes());
    }
    value.extend_from_slice(&[0, 0]);
    rec_scalar(clx_type::STRING, name, &value)
}

fn rec_level(name: &str, children: &[Vec<u8>]) -> Vec<u8> {
    let (nl, nb) = utf16_name(name);
    let item_count = children.len();
    let child_bytes: Vec<u8> = children.iter().flatten().copied().collect();
    let length = (2 + nb.len() + 12 + child_bytes.len()) as u64;
    let mut r = vec![clx_type::LEVEL, nl];
    r.extend(nb);
    r.extend_from_slice(&(item_count as u32).to_le_bytes());
    r.extend_from_slice(&length.to_le_bytes());
    r.extend(child_bytes);
    r.extend(vec![0u8; item_count * 8]);
    r
}

#[test]
fn clx_level_with_scalars_roundtrips() {
    let root = rec_level(
        "Root",
        &[
            rec_u32("W", 2868),
            rec_f64("D", 0.275),
            rec_str("S", "Alx546"),
        ],
    );
    let parsed = parse_clx_variant(&root).unwrap();
    let inner = parsed.get_level("Root").expect("Root level");
    assert_eq!(inner.get_u32("W"), Some(2868));
    assert_eq!(inner.get_f64("D"), Some(0.275));
    assert_eq!(inner.get_str("S"), Some("Alx546"));
    assert_eq!(inner.len(), 3);
}

#[test]
fn clx_nested_level_arithmetic() {
    let root = rec_level(
        "SLxExperiment",
        &[
            rec_u32("eType", 4),
            rec_level("uLoopPars", &[rec_u32("uiCount", 25), rec_f64("dZStep", 0.275)]),
        ],
    );
    let parsed = parse_clx_variant(&root).unwrap();
    let exp = parsed.get_level("SLxExperiment").unwrap();
    assert_eq!(exp.get_i64("eType"), Some(4));
    let lp = exp.get_level("uLoopPars").unwrap();
    assert_eq!(lp.get_usize("uiCount"), Some(25));
    assert_eq!(lp.get_f64("dZStep"), Some(0.275));
}

#[test]
fn clx_empty_name_items_collapse_to_list() {
    let root = rec_level(
        "ppNextLevelEx",
        &[
            rec_level("", &[rec_u32("eType", 4)]),
            rec_level("", &[rec_u32("eType", 4)]),
        ],
    );
    let parsed = parse_clx_variant(&root).unwrap();
    match parsed.get("ppNextLevelEx") {
        Some(ClxValue::List(items)) => {
            assert_eq!(items.len(), 2);
            assert!(items.iter().all(|v| v.as_level().is_some()));
        }
        other => panic!("expected a List, got {other:?}"),
    }
}

#[test]
fn clx_compress_roundtrips() {
    let tree = rec_level("Root", &[rec_u32("W", 7), rec_str("S", "hi")]);
    let plain = parse_clx_variant(&tree).unwrap();

    let mut encoder =
        flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&tree).unwrap();
    let compressed = encoder.finish().unwrap();

    let mut input = vec![clx_type::COMPRESS, 0];
    input.extend_from_slice(&[0u8; 10]);
    input.extend_from_slice(&compressed);

    let inflated = parse_clx_variant(&input).unwrap();
    assert_eq!(inflated, plain);
    assert_eq!(inflated.get_level("Root").unwrap().get_u32("W"), Some(7));
}

fn map_entry(blob: &mut Vec<u8>, name: &[u8], offset: u64, size: u64) {
    blob.extend_from_slice(name);
    blob.extend_from_slice(&offset.to_le_bytes());
    blob.extend_from_slice(&size.to_le_bytes());
}

#[test]
fn chunkmap_blob_parses_entries_and_stops_at_signature() {
    let mut blob = Vec::new();
    map_entry(&mut blob, b"ImageAttributesLV!", 100, 580);
    map_entry(&mut blob, b"ImageDataSeq|0!", 200, 16450856);
    map_entry(&mut blob, b"ImageMetadataLV!", 300, 300);
    blob.extend_from_slice(SIG_CHUNKMAP_END);
    blob.extend_from_slice(&[0xAA; 8]);

    let map = parse_chunkmap_blob(&blob).unwrap();
    assert_eq!(map.len(), 3);
    assert_eq!(map.get(b"ImageAttributesLV!".as_slice()), Some(&(100, 580)));
    assert_eq!(map.get(b"ImageDataSeq|0!".as_slice()), Some(&(200, 16450856)));
    assert_eq!(map.get(b"ImageMetadataLV!".as_slice()), Some(&(300, u64::MAX)));
}

#[test]
fn seq_index_is_row_major_product() {
    let loops = vec![
        Loop { axis: Axis::P, count: 2 },
        Loop { axis: Axis::T, count: 3 },
        Loop { axis: Axis::Z, count: 4 },
    ];
    assert_eq!(scene_count(&loops), 2);
    assert_eq!(axis_count(&loops, Axis::T), 3);
    assert_eq!(axis_count(&loops, Axis::Z), 4);
    assert_eq!(seq_index(&loops, 0, 0, 0).unwrap(), 0);
    assert_eq!(seq_index(&loops, 0, 0, 1).unwrap(), 1);
    assert_eq!(seq_index(&loops, 0, 1, 0).unwrap(), 4);
    assert_eq!(seq_index(&loops, 1, 0, 0).unwrap(), 12);
    assert_eq!(seq_index(&loops, 1, 2, 3).unwrap(), 23);
}

#[test]
fn single_zstack_defaults_absent_axes_to_one() {
    let loops = vec![Loop { axis: Axis::Z, count: 25 }];
    assert_eq!(scene_count(&loops), 1);
    assert_eq!(axis_count(&loops, Axis::T), 1);
    assert_eq!(axis_count(&loops, Axis::Z), 25);
    assert_eq!(seq_index(&loops, 0, 0, 12).unwrap(), 12);
}

#[test]
fn spectloop_is_skipped_and_zstack_survives() {
    // Mirrors the real sample: SpectLoop is skipped, leaving ZStackLoop(25) as the only axis.
    let zstack = rec_level(
        "",
        &[
            rec_u32("eType", 4),
            rec_level("uLoopPars", &[rec_u32("uiCount", 25)]),
        ],
    );
    let exp = rec_level(
        "SLxExperiment",
        &[
            rec_u32("eType", 6),
            rec_level(
                "uLoopPars",
                &[rec_level("pPlanes", &[rec_u32("uiCount", 1)])],
            ),
            rec_level("ppNextLevelEx", &[zstack]),
        ],
    );
    let parsed = parse_clx_variant(&exp).unwrap();
    let loops = load_experiment(parsed.get_level("SLxExperiment").unwrap()).unwrap();
    assert_eq!(loops, vec![Loop { axis: Axis::Z, count: 25 }]);
}

#[test]
fn custom_loop_is_unsupported() {
    let exp = rec_level(
        "SLxExperiment",
        &[
            rec_u32("eType", 7),
            rec_level("uLoopPars", &[rec_u32("uiCount", 3)]),
        ],
    );
    let parsed = parse_clx_variant(&exp).unwrap();
    let err = load_experiment(parsed.get_level("SLxExperiment").unwrap());
    assert!(matches!(err, Err(Error::Unsupported(_))));
}

#[test]
fn unhandled_loop_type_errors_not_silently_dropped() {
    // ZStackLoopAccurate is in range but unhandled: it must fail loud, not yield an empty loop list.
    let exp = rec_level(
        "SLxExperiment",
        &[
            rec_u32("eType", 10),
            rec_level("uLoopPars", &[rec_u32("uiCount", 25)]),
        ],
    );
    let parsed = parse_clx_variant(&exp).unwrap();
    assert!(matches!(
        load_experiment(parsed.get_level("SLxExperiment").unwrap()),
        Err(Error::Unsupported(_))
    ));
}

#[test]
fn oversized_loop_count_is_refused() {
    // A crafted XYPosLoop count is refused before it can size a Vec<SceneMeta> and abort.
    let exp = rec_level(
        "SLxExperiment",
        &[
            rec_u32("eType", 2),
            rec_level("uLoopPars", &[rec_u32("uiCount", u32::MAX)]),
        ],
    );
    let parsed = parse_clx_variant(&exp).unwrap();
    assert!(matches!(
        load_experiment(parsed.get_level("SLxExperiment").unwrap()),
        Err(Error::Unsupported(_))
    ));
}

// The ND2 container is written by tests/support/nd2.rs; the CLx builders below test private functions.

fn temp_path() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "phasit_synth_{:?}_{}_{}.nd2",
        std::thread::current().id(),
        std::process::id(),
        n
    ))
}

#[test]
fn legacy_jp2_magic_is_unsupported() {
    let path = temp_path();
    let mut bytes = JP2_MAGIC.to_le_bytes().to_vec();
    bytes.resize(64, 0);
    std::fs::write(&path, &bytes).unwrap();
    let r = Nd2Reader::open(&path);
    let _ = std::fs::remove_file(&path);
    assert!(matches!(r, Err(Error::Unsupported(_))));
}
