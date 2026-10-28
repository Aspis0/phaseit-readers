// SPDX-License-Identifier: Apache-2.0
//! The ND2 reader reads a sub-region without decoding the whole plane.
//!
//! An ND2 frame's rows sit at a computable byte offset (the row stride is
//! `uiWidthBytes`, which may exceed the pixels' own width), so the rows of a
//! rectangle can be read straight out of the frame. Every rectangle the reader
//! accepts must therefore give exactly the same bytes as cropping the whole
//! plane.

mod support;

use phasit_imread::nd2_reader::Nd2Reader;
use phasit_imread::types::PixelRect;
use phasit_imread::{Error, ImageReader, Plane};

use support::nd2::{write_nd2, Nd2File};
use support::scratch::Scratch;

/// A scratch ND2 file of this test run.
fn scratch(name: &str) -> Scratch {
    Scratch::new(std::env::temp_dir().join(format!(
        "phasit_nd2_region_{name}_{}",
        std::process::id()
    )))
}

/// Assert that a region and the cropped plane are the same — WITHOUT printing
/// the pixels: the message names the difference, never the data.
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

/// Write an ND2 and open it.
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
    // The row stride is wider than the pixels: the padding sits AFTER a row's
    // pixels and must never leak into a region.
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
    // Two components per pixel: each component is its own plane, and a
    // component's pixels sit `x*comp*itemsize + c*itemsize` inside a row.
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
    // Eight-bit samples: the item size is not 2, so the reader's byte-per-
    // sample arithmetic must follow the file's `uiBpcInMemory`.
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
    // Thirty-two-bit float samples: four bytes per sample.
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
    // Two components per pixel AND a row stride wider than the pixels: the
    // interleaved gather has to skip the padding of every row it reads.
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
    // The frame holds 8 of the 24 bytes it declares. `plane()` refuses it, and
    // `region()` must refuse it the same way: a caller that only reads regions
    // has to be able to tell a short frame from a whole one.
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
    // The file states a 70000 x 70000 U8 plane: 4.9 GB, past the reader's
    // 4 GiB decode ceiling. The ceiling guards ONE materialisation, not the
    // file's existence, so the file still OPENS and publishes its scene.
    //
    // The frame cannot hold 4.9 GB here (that is more than a test may write),
    // so this test pins the DECISION — the file opens, and `plane()` refuses
    // the materialisation with the oversized error, not with a parse error.
    // The shared decision itself is pinned in `tests/plane_ceiling.rs`, and
    // the CZI reader's over-ceiling scene (whose tiles make the file small)
    // shows a region being served.
    let mut file = Nd2File::gray16(2, 2, 1);
    file.width = 70_000;
    file.height = 70_000;
    file.itemsize = 1;
    file.width_bytes = 70_000;
    file.frames[0].pixels = vec![0u8; 8]; // far too few bytes for the plane

    let (_path, reader) = open(&file, "over_ceiling");
    assert_eq!(reader.meta().scene_count(), 1, "the file is published");

    // The plane is refused as oversized — the ceiling, not the frame.
    match reader.plane(0, 0, 0, 0) {
        Err(Error::OversizedAllocation { .. }) => {}
        Err(other) => panic!("the plane must be refused as oversized, got {other}"),
        Ok(_) => panic!("the plane must not be materialised"),
    }

    // The region path is NOT blocked by the ceiling: it reads the frame and
    // reports the file's MISSING bytes, which is a different fault.
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
    // Only the uncompressed value is decodable. Every other value is refused
    // BY NAME, so the reason says what the file said.
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
    // A file with no `eCompression` record says nothing about how its frames
    // are stored, so it is refused: both real samples state the uncompressed
    // value, and an absent value is not evidence of anything.
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

    // A coordinate outside the scene is `OutOfRange`, never a panic.
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

    // The coordinate is checked BEFORE the rect: a call that is wrong in both
    // ways reports the coordinate, like the trait default does.
    assert!(matches!(
        reader.region(0, 9, 0, 0, PixelRect::new(99, 99, 1, 1)),
        Err(Error::OutOfRange { .. })
    ));
}

#[test]
fn a_rect_outside_the_plane_is_a_decode_error() {
    let (_path, reader) = open(&Nd2File::gray16(4, 3, 1), "rect");

    // A rect past the plane's extent is a clean `Decode` error.
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
