// SPDX-License-Identifier: Apache-2.0
//! The colour contract of the CZI reader, on a committed fixture: a Bgr24 CZI stores B,G,R, and the reader publishes
//! R,G,B.
//! Colour is three interleaved samples of one channel, never extra C channels. The fixture is written by
//! `tests/support/czi.rs`;
//! this test proves the committed file is that builder's output, byte for byte. The optional third-party sample is not
//! evidence here.

mod support;

use std::path::{Path, PathBuf};

use phasit_imread::czi_reader::CziReader;
use phasit_imread::{DType, ImageReader, PlaneBuf};

use support::czi::{scratch, Tile};
use support::czi_write::write_czi;

const WIDTH: u32 = 8;
const HEIGHT: u32 = 6;

/// The CZI `PixelType` id of a Bgr24 tile: three 8-bit samples per pixel.
const PIXEL_TYPE_BGR24: i32 = 3;

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/czi/bgr24-rgb.czi")
}

/// Three distinct values per pixel, so a swapped channel, a shifted offset or a wrong width cannot pass.
fn stored_bgr() -> Vec<u8> {
    let mut bytes = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for pixel in 0..(WIDTH * HEIGHT) {
        bytes.push((pixel * 3 + 1) as u8);
        bytes.push((pixel * 3 + 2) as u8);
        bytes.push((pixel * 3 + 3) as u8);
    }
    bytes
}

fn published_rgb() -> Vec<u8> {
    let mut bytes = stored_bgr();
    for pixel in bytes.as_chunks_mut::<3>().0.iter_mut() {
        pixel.swap(0, 2);
    }
    bytes
}

fn first_difference(left: &[u8], right: &[u8]) -> Option<usize> {
    if left.len() != right.len() {
        return Some(left.len().min(right.len()));
    }
    left.iter().zip(right).position(|(a, b)| a != b)
}

fn built_bytes(tag: &str) -> Vec<u8> {
    let path = scratch(tag);
    write_czi(
        &path,
        &[Tile::with_pixels(
            0,
            PIXEL_TYPE_BGR24,
            WIDTH,
            HEIGHT,
            stored_bgr(),
        )],
    );
    std::fs::read(&*path).expect("read the built fixture")
}

#[test]
fn the_committed_fixture_is_the_builders_own_bytes() {
    let committed = std::fs::read(fixture_path()).expect("read the committed fixture");
    let built = built_bytes("fixture_check");
    if let Some(at) = first_difference(&committed, &built) {
        panic!(
            "the committed fixture is not what the CZI builder writes: \
             byte {at} differs ({} vs {})",
            committed[at], built[at]
        );
    }
    assert!(
        committed.len() < 64 * 1024,
        "the fixture must stay tiny, it is {} bytes",
        committed.len()
    );
}

#[test]
fn a_bgr24_czi_publishes_three_u8_samples_in_rgb_order() {
    let reader = CziReader::open(&fixture_path()).expect("open the Bgr24 fixture");
    let meta = reader.meta();
    assert_eq!(meta.scene_count(), 1, "the fixture holds one scene");

    let scene = meta.scene(0).expect("the fixture's scene");
    assert_eq!(scene.dtype, DType::U8, "Bgr24 carries 8-bit samples");
    assert_eq!(scene.samples, 3, "colour is three interleaved samples");
    assert_eq!(scene.dims.c, 1, "colour must NOT explode into C channels");
    assert_eq!(scene.dims.z, 1);
    assert_eq!(scene.dims.t, 1);
    assert_eq!(scene.dims.y, HEIGHT as usize, "height");
    assert_eq!(scene.dims.x, WIDTH as usize, "width");

    let plane = reader
        .plane(0, 0, 0, 0)
        .expect("decode the fixture's plane");
    assert_eq!(plane.shape, [HEIGHT as usize, WIDTH as usize]);
    assert_eq!(plane.samples(), 3);
    match &plane.data {
        PlaneBuf::U8(bytes) => {
            let expected = published_rgb();
            if let Some(at) = first_difference(bytes, &expected) {
                panic!(
                    "the reader must publish R,G,B — the file stores B,G,R: \
                     sample {at} is {}, not {}",
                    bytes[at], expected[at]
                );
            }
            assert!(
                first_difference(bytes, &stored_bgr()).is_some(),
                "the reader must NOT publish the file's own B,G,R order"
            );
        }
        other => panic!("expected a U8 plane, got {:?}", other.dtype()),
    }
}

/// Run: `cargo test -p phasit-imread --test czi_bgr24_synthetic -- --ignored regenerate_the_committed_fixture`
#[test]
#[ignore]
fn regenerate_the_committed_fixture() {
    std::fs::write(fixture_path(), built_bytes("regenerate")).expect("write the committed fixture");
}
