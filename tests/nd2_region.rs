// SPDX-License-Identifier: Apache-2.0
//! A window of an ND2 frame is read without decoding the whole plane: every accepted rectangle must give the bytes of
//! cropping it.
//! Rows sit at a computable offset (`uiWidthBytes`, the row stride, may exceed the pixel width), so windows are copied
//! straight out.

mod support;

use phasit_imread::nd2_reader::Nd2Reader;
use phasit_imread::types::PixelRect;
use phasit_imread::{Error, ImageReader, Plane};

use support::nd2::{write_nd2, Nd2File};
use support::scratch::Scratch;

fn scratch(name: &str) -> Scratch {
    Scratch::new(std::env::temp_dir().join(format!(
        "phasit_nd2_region_{name}_{}",
        std::process::id()
    )))
}

/// Compares without printing the pixels: a failure names the difference, never the data.
fn assert_same(got: &Plane, want: &Plane, what: &str) {
    assert_eq!(got.shape, want.shape, "{what}: the region's shape differs");
    assert_eq!(
        got.samples, want.samples,
        "{what}: the region's samples differ"
    );
    assert!(
        got.data == want.data,
        "{what}: the region's bytes differ from the cropped plane"
    );
}

fn open(file: &Nd2File, name: &str) -> (Scratch, Nd2Reader) {
    let path = scratch(name);
    write_nd2(&path, file);
    let reader = Nd2Reader::open(&path).expect("open the synthetic ND2");
    (path, reader)
}

#[test]
fn the_reader_advertises_region_support() {
    let (_path, reader) = open(&Nd2File::gray16(4, 3, 1), "advertise");
    assert!(
        reader.supports_region(),
        "the ND2 reader reads a sub-region without decoding the whole plane"
    );
}

#[test]
fn a_region_equals_the_cropped_plane() {
    let (_path, reader) = open(&Nd2File::gray16(5, 4, 2), "rects");

    let rects = [
        PixelRect::new(0, 0, 5, 4), // the whole plane
        PixelRect::new(0, 0, 1, 1), // one pixel at the origin
        PixelRect::new(4, 3, 1, 1), // one pixel at the far corner
        PixelRect::new(1, 1, 3, 2), // an edge rect aligned to nothing
        PixelRect::new(3, 2, 2, 2), // a rect touching the last row and column
        PixelRect::new(2, 1, 1, 2), // a single column, off both edges
    ];
    for z in 0..2 {
        let plane = reader.plane(0, 0, 0, z).expect("decode the plane");
        for rect in rects {
            let region = reader.region(0, 0, 0, z, rect).expect("read the region");
            let cropped = plane.crop(rect).expect("crop the plane");
            assert_same(&region, &cropped, "a region of an ND2 plane");
        }
    }
}

#[test]
fn a_region_equals_the_cropped_plane_of_a_padded_frame() {
    // The row stride is wider than the pixels: the padding after each row must never leak into a region.
    let mut file = Nd2File::gray16(3, 3, 1);
    file.width_bytes = 3 * 2 + 4;
    let mut pixels = Vec::new();
    for row in 0..3 {
        for col in 0..3 {
            pixels.extend_from_slice(&((row * 10 + col) as u16).to_le_bytes());
        }
        pixels.extend_from_slice(&[0xFF; 4]); // the row's padding
    }
    file.frames[0].pixels = pixels;

    let (_path, reader) = open(&file, "padded");
    let plane = reader.plane(0, 0, 0, 0).expect("decode the plane");
    let rects = [
        PixelRect::new(0, 0, 3, 3),
        PixelRect::new(1, 1, 2, 2),
        PixelRect::new(2, 0, 1, 3),
        PixelRect::new(0, 2, 3, 1),
    ];
    for rect in rects {
        let region = reader.region(0, 0, 0, 0, rect).expect("read the region");
        let cropped = plane.crop(rect).expect("crop the plane");
        assert_same(&region, &cropped, "a region of a padded ND2 plane");
    }
}

#[test]
fn a_region_equals_the_cropped_plane_of_an_interleaved_frame() {
    // Two components: each is its own plane, and a component's pixels sit at `x*comp*itemsize + c*itemsize` inside a
    // row.
    let mut file = Nd2File::gray16(3, 2, 1);
    file.comp = 2;
    file.width_bytes = 3 * 2 * 2;
    let mut pixels = Vec::new();
    for row in 0..2u16 {
        for col in 0..3u16 {
            pixels.extend_from_slice(&(row * 100 + col * 10).to_le_bytes());
            pixels.extend_from_slice(&(row * 100 + col * 10 + 1).to_le_bytes());
        }
    }
    file.frames[0].pixels = pixels;

    let (_path, reader) = open(&file, "interleaved");
    let rects = [
        PixelRect::new(0, 0, 3, 2),
        PixelRect::new(1, 0, 2, 2),
        PixelRect::new(2, 1, 1, 1),
    ];
    for c in 0..2 {
        let plane = reader.plane(0, 0, c, 0).expect("decode the plane");
        for rect in rects {
            let region = reader.region(0, 0, c, 0, rect).expect("read the region");
            let cropped = plane.crop(rect).expect("crop the plane");
            assert_same(&region, &cropped, "a region of an interleaved ND2 plane");
        }
    }
}

#[test]
fn a_region_equals_the_cropped_plane_of_a_u8_frame() {
    // Eight-bit samples: the byte arithmetic must follow the file's `uiBpcInMemory`, not two bytes.
    let mut file = Nd2File::gray16(3, 2, 1);
    file.itemsize = 1;
    file.width_bytes = 3;
    file.frames[0].pixels = (0..6u8).collect();

    let (_path, reader) = open(&file, "u8");
    let plane = reader.plane(0, 0, 0, 0).expect("decode the plane");
    let rects = [
        PixelRect::new(0, 0, 3, 2),
        PixelRect::new(1, 0, 2, 1),
        PixelRect::new(2, 1, 1, 1),
    ];
    for rect in rects {
        let region = reader.region(0, 0, 0, 0, rect).expect("read the region");
        let cropped = plane.crop(rect).expect("crop the plane");
        assert_same(&region, &cropped, "a region of a U8 ND2 plane");
    }
}

#[test]
fn a_region_equals_the_cropped_plane_of_an_f32_frame() {
    let mut file = Nd2File::gray16(3, 2, 1);
    file.itemsize = 4;
    file.width_bytes = 3 * 4;
    file.frames[0].pixels = (0..6usize)
        .map(|v| v as f32 * 0.5)
        .flat_map(|v| v.to_le_bytes())
        .collect();

    let (_path, reader) = open(&file, "f32");
    let plane = reader.plane(0, 0, 0, 0).expect("decode the plane");
    let rects = [
        PixelRect::new(0, 0, 3, 2),
        PixelRect::new(1, 1, 2, 1),
        PixelRect::new(2, 0, 1, 2),
    ];
    for rect in rects {
        let region = reader.region(0, 0, 0, 0, rect).expect("read the region");
        let cropped = plane.crop(rect).expect("crop the plane");
        assert_same(&region, &cropped, "a region of an F32 ND2 plane");
    }
}

#[test]
fn a_region_equals_the_cropped_plane_of_an_interleaved_padded_frame() {
    // Two components and a padded stride: the interleaved gather must skip each row's padding.
    let mut file = Nd2File::gray16(3, 2, 1);
    file.comp = 2;
    file.width_bytes = 3 * 2 * 2 + 4; // 12 content bytes + 4 padding bytes
    let mut pixels = Vec::new();
    for row in 0..2u16 {
        for col in 0..3u16 {
            pixels.extend_from_slice(&(row * 100 + col * 10).to_le_bytes());
            pixels.extend_from_slice(&(row * 100 + col * 10 + 1).to_le_bytes());
        }
        pixels.extend_from_slice(&[0xFF; 4]); // the row's padding
    }
    file.frames[0].pixels = pixels;

    let (_path, reader) = open(&file, "interleaved_padded");
    let rects = [
        PixelRect::new(0, 0, 3, 2),
        PixelRect::new(1, 0, 2, 2),
        PixelRect::new(2, 1, 1, 1),
        PixelRect::new(0, 1, 3, 1),
    ];
    for c in 0..2 {
        let plane = reader.plane(0, 0, c, 0).expect("decode the plane");
        for rect in rects {
            let region = reader.region(0, 0, c, 0, rect).expect("read the region");
            let cropped = plane.crop(rect).expect("crop the plane");
            assert_same(&region, &cropped, "a region of an interleaved padded ND2 plane");
        }
    }
}

#[test]
fn a_region_of_a_short_frame_is_refused_exactly_like_the_plane() {
    // A short frame is refused by `plane()`, and `region()` must refuse it the same way: callers must tell a short
    // frame from a whole one.
    let mut file = Nd2File::gray16(4, 3, 1);
    file.frames[0].pixels.truncate(8);

    let (_path, reader) = open(&file, "short_frame");
    let plane_error = reader.plane(0, 0, 0, 0).expect_err("the plane is refused");
    let region_error = reader
        .region(0, 0, 0, 0, PixelRect::new(0, 0, 1, 1))
        .expect_err("the region is refused");
    assert_eq!(
        region_error.to_string(),
        plane_error.to_string(),
        "the region and the plane refuse a short frame the same way"
    );
    assert!(
        region_error.to_string().contains("pixel bytes"),
        "the reason names the short frame: {region_error}"
    );
}

#[test]
fn a_file_whose_plane_is_over_the_ceiling_is_published() {
    // The ceiling guards one materialisation, not the file: the file opens and `plane()` refuses with the oversized
    // error (see `tests/plane_ceiling.rs`).
    let mut file = Nd2File::gray16(2, 2, 1);
    file.width = 70_000;
    file.height = 70_000;
    file.itemsize = 1;
    file.width_bytes = 70_000;
    file.frames[0].pixels = vec![0u8; 8]; // far too few bytes for the plane

    let (_path, reader) = open(&file, "over_ceiling");
    assert_eq!(reader.meta().scene_count(), 1, "the file is published");

    match reader.plane(0, 0, 0, 0) {
        Err(Error::OversizedAllocation { .. }) => {}
        Err(other) => panic!("the plane must be refused as oversized, got {other}"),
        Ok(_) => panic!("the plane must not be materialised"),
    }

    // The region path is not blocked by the ceiling: it reports the frame's missing bytes, a different fault.
    let region_error = reader
        .region(0, 0, 0, 0, PixelRect::new(0, 0, 1, 1))
        .expect_err("the frame is too short for the plane it declares");
    assert!(
        !matches!(region_error, Error::OversizedAllocation { .. }),
        "the region path is not blocked by the ceiling: {region_error}"
    );
}

#[test]
fn a_compressed_file_is_refused_with_the_value_it_states() {
    // Only the uncompressed value is decodable; every other value is refused by name.
    for value in [0i64, 1, 3, 7] {
        let mut file = Nd2File::gray16(2, 2, 1);
        file.e_compression = Some(value);
        let path = scratch("compressed");
        write_nd2(&path, &file);

        match Nd2Reader::open(&path) {
            Err(Error::Unsupported(message)) => {
                assert!(
                    message.contains(&format!("eCompression {value}")),
                    "the reason names the value {value}: {message}"
                );
            }
            Err(other) => panic!("the file must not open, got {other}"),
            Ok(_) => panic!("the file must not open"),
        }
    }
}

#[test]
fn a_file_that_states_no_compression_is_refused() {
    // An absent `eCompression` says nothing about how frames are stored, so the file is refused.
    let mut file = Nd2File::gray16(2, 2, 1);
    file.e_compression = None;
    let path = scratch("no_compression");
    write_nd2(&path, &file);

    match Nd2Reader::open(&path) {
        Err(Error::Unsupported(message)) => {
            assert!(
                message.contains("no eCompression"),
                "the reason names what the file did not state: {message}"
            );
        }
        Err(other) => panic!("the file must not open, got {other}"),
        Ok(_) => panic!("the file must not open"),
    }
}

#[test]
fn an_invalid_coordinate_is_out_of_range() {
    let (_path, reader) = open(&Nd2File::gray16(4, 3, 1), "coordinate");

    assert!(matches!(
        reader.region(0, 1, 0, 0, PixelRect::new(0, 0, 1, 1)),
        Err(Error::OutOfRange { t: 1, .. })
    ));
    assert!(matches!(
        reader.region(0, 0, 1, 0, PixelRect::new(0, 0, 1, 1)),
        Err(Error::OutOfRange { c: 1, .. })
    ));
    assert!(matches!(
        reader.region(0, 0, 0, 1, PixelRect::new(0, 0, 1, 1)),
        Err(Error::OutOfRange { z: 1, .. })
    ));
    assert!(matches!(
        reader.region(1, 0, 0, 0, PixelRect::new(0, 0, 1, 1)),
        Err(Error::SceneOutOfRange { scene: 1, .. })
    ));

    // The coordinate is checked before the rect, as the trait default does.
    assert!(matches!(
        reader.region(0, 9, 0, 0, PixelRect::new(99, 99, 1, 1)),
        Err(Error::OutOfRange { .. })
    ));
}

#[test]
fn a_rect_outside_the_plane_is_a_decode_error() {
    let (_path, reader) = open(&Nd2File::gray16(4, 3, 1), "rect");

    assert!(matches!(
        reader.region(0, 0, 0, 0, PixelRect::new(3, 0, 2, 1)),
        Err(Error::Decode(_))
    ));
    assert!(matches!(
        reader.region(0, 0, 0, 0, PixelRect::new(0, 2, 1, 2)),
        Err(Error::Decode(_))
    ));
    // An overflowing rect is refused, never wrapped.
    assert!(matches!(
        reader.region(0, 0, 0, 0, PixelRect::new(usize::MAX, 0, 2, 1)),
        Err(Error::Decode(_))
    ));
    assert!(matches!(
        reader.region(0, 0, 0, 0, PixelRect::new(0, usize::MAX, 1, 2)),
        Err(Error::Decode(_))
    ));
}
