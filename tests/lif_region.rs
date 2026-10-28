// SPDX-License-Identifier: Apache-2.0
//! The LIF reader reads a sub-region without decoding the whole plane.
//!
//! A LIF frame is stored contiguously (the reader refuses an image whose X or
//! Y dimension is not contiguous), so the rows of a rectangle sit at computable
//! offsets in the image's memory block. Every rectangle the reader accepts
//! must therefore give exactly the same bytes as cropping the whole plane.

mod support;

use phasit_imread::lif_reader::LifReader;
use phasit_imread::types::PixelRect;
use phasit_imread::{Error, ImageReader, Plane, PlaneBuf};

use support::lif::{write_lif, LifImage};
use support::scratch::Scratch;

/// A scratch LIF file of this test run.
fn scratch(name: &str) -> Scratch {
    Scratch::new(std::env::temp_dir().join(format!(
        "phasit_lif_region_{name}_{}",
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

/// Write a LIF holding one image and open it.
fn open(image: &LifImage, name: &str) -> (Scratch, LifReader) {
    let path = scratch(name);
    write_lif(&path, std::slice::from_ref(image));
    let reader = LifReader::open(&path).expect("open the synthetic LIF");
    (path, reader)
}

#[test]
fn the_reader_advertises_region_support() {
    let image = LifImage::gray8("image", "block", 4, 3, None, None, 1);
    let (_path, reader) = open(&image, "advertise");
    assert!(
        reader.supports_region(),
        "the LIF reader reads a sub-region without decoding the whole plane"
    );
}

#[test]
fn a_region_equals_the_cropped_plane() {
    let image = LifImage::gray8("image", "block", 5, 4, None, None, 1);
    let (_path, reader) = open(&image, "rects");

    // The fixture is what the test thinks it is: the plane is the pattern.
    let plane = reader.plane(0, 0, 0, 0).expect("decode the plane");
    assert_eq!(plane.shape, [4, 5]);
    assert!(
        plane.data == PlaneBuf::U8(image.frame(0, 0, 0).to_vec()),
        "the fixture's plane does not hold the expected pattern"
    );

    let rects = [
        PixelRect::new(0, 0, 5, 4), // the whole plane
        PixelRect::new(0, 0, 1, 1), // one pixel at the origin
        PixelRect::new(4, 3, 1, 1), // one pixel at the far corner
        PixelRect::new(1, 1, 3, 2), // an edge rect aligned to nothing
        PixelRect::new(3, 2, 2, 2), // a rect touching the last row and column
        PixelRect::new(2, 1, 1, 2), // a single column, off both edges
    ];
    for rect in rects {
        let region = reader.region(0, 0, 0, 0, rect).expect("read the region");
        let cropped = plane.crop(rect).expect("crop the plane");
        assert_same(&region, &cropped, "a region of a U8 LIF plane");
    }
}

#[test]
fn a_region_equals_the_cropped_plane_across_channels_and_z() {
    // Two channels and two Z planes: the channel and the Z plane each move
    // the frame inside the memory block, so the region must follow them.
    let image = LifImage::gray8("image", "block", 4, 3, Some(2), None, 2);
    let (_path, reader) = open(&image, "channels");

    let rects = [
        PixelRect::new(0, 0, 4, 3),
        PixelRect::new(1, 0, 2, 3),
        PixelRect::new(3, 2, 1, 1),
    ];
    for c in 0..2 {
        for z in 0..2 {
            let plane = reader.plane(0, 0, c, z).expect("decode the plane");
            for rect in rects {
                let region = reader.region(0, 0, c, z, rect).expect("read the region");
                let cropped = plane.crop(rect).expect("crop the plane");
                assert_same(&region, &cropped, "a region of a multi-channel LIF plane");
            }
        }
    }
}

#[test]
fn a_region_equals_the_cropped_plane_across_time_points() {
    // Two time points and two Z planes: the T step moves the whole slab.
    let image = LifImage::gray8("image", "block", 4, 3, Some(2), Some(2), 1);
    let (_path, reader) = open(&image, "timepoints");

    for t in 0..2 {
        for z in 0..2 {
            let plane = reader.plane(0, t, 0, z).expect("decode the plane");
            let rect = PixelRect::new(1, 1, 2, 2);
            let region = reader.region(0, t, 0, z, rect).expect("read the region");
            let cropped = plane.crop(rect).expect("crop the plane");
            assert_same(&region, &cropped, "a region of a LIF plane across T and Z");
        }
    }
}

#[test]
fn a_u16_region_equals_the_cropped_plane() {
    // Sixteen-bit samples: the region converts the bytes to native endianness
    // exactly like the whole plane does.
    let mut image = LifImage::gray8("image", "block", 4, 3, None, None, 1);
    image.itemsize = 2;
    image.pixels = (0..12u16)
        .flat_map(|v| (v * 7 + 1).to_le_bytes())
        .collect();
    let (_path, reader) = open(&image, "u16");

    let plane = reader.plane(0, 0, 0, 0).expect("decode the plane");
    let rect = PixelRect::new(1, 1, 2, 2);
    let region = reader.region(0, 0, 0, 0, rect).expect("read the region");
    let cropped = plane.crop(rect).expect("crop the plane");
    assert_same(&region, &cropped, "a region of a U16 LIF plane");
}

#[test]
fn a_full_width_region_below_the_first_row_equals_the_cropped_plane() {
    // A FULL-WIDTH rectangle that does NOT start at row 0: the reader reads the
    // rect's rows as one contiguous span, so the row offset has to be in
    // there. A reader that dropped the `rect.y` term would return the first
    // rows instead.
    let image = LifImage::gray8("image", "block", 5, 4, None, None, 1);
    let (_path, reader) = open(&image, "full_width_below");

    let plane = reader.plane(0, 0, 0, 0).expect("decode the plane");
    let rects = [
        PixelRect::new(0, 1, 5, 3), // full width, from the second row
        PixelRect::new(0, 2, 5, 2), // full width, the last two rows
        PixelRect::new(0, 3, 5, 1), // full width, the last row
    ];
    for rect in rects {
        let region = reader.region(0, 0, 0, 0, rect).expect("read the region");
        let cropped = plane.crop(rect).expect("crop the plane");
        assert_same(&region, &cropped, "a full-width region below the first row");
    }
}

#[test]
fn an_f32_region_equals_the_cropped_plane() {
    // Thirty-two-bit float samples: the region converts the bytes to native
    // endianness exactly like the whole plane does.
    let mut image = LifImage::gray8("image", "block", 4, 3, None, None, 1);
    image.itemsize = 4;
    image.pixels = (0..12usize)
        .map(|v| v as f32 * 0.5 + 1.0)
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let (_path, reader) = open(&image, "f32");

    let plane = reader.plane(0, 0, 0, 0).expect("decode the plane");
    let rect = PixelRect::new(1, 1, 2, 2);
    let region = reader.region(0, 0, 0, 0, rect).expect("read the region");
    let cropped = plane.crop(rect).expect("crop the plane");
    assert_same(&region, &cropped, "a region of an F32 LIF plane");
}

#[test]
fn an_invalid_coordinate_is_out_of_range() {
    let image = LifImage::gray8("image", "block", 4, 3, None, None, 1);
    let (_path, reader) = open(&image, "coordinate");

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
    let image = LifImage::gray8("image", "block", 4, 3, None, None, 1);
    let (_path, reader) = open(&image, "rect");

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
