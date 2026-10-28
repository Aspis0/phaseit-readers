// SPDX-License-Identifier: Apache-2.0
//! An EMPTY rectangle is an empty plane, on every reader.
//!
//! The trait's default `region()` returns `plane()?.crop(rect)`, and cropping
//! an empty rectangle gives an EMPTY plane of shape `[rect.h, rect.w]`. Every
//! reader's own path must return the same thing for the same call — and a
//! reader that counts the rows an empty rectangle covers can underflow on
//! `rect.h - 1`, so the empty case is settled before any counting starts.

mod support;

use phasit_imread::czi_reader::CziReader;
use phasit_imread::lif_reader::LifReader;
use phasit_imread::lsm_reader::LsmReader;
use phasit_imread::nd2_reader::Nd2Reader;
use phasit_imread::tiff_reader::TiffReader;
use phasit_imread::types::PixelRect;
use phasit_imread::{Error, ImageReader, Plane};

use support::czi::{scratch as czi_scratch, write_czi, Tile};
use support::lif::{write_lif, LifImage};
use support::lsm::{write_lsm, LsmSpec};
use support::nd2::{write_nd2, Nd2File};
use support::scratch_dir;
use support::tiff_page::SampleType;
use support::tiff_striped::{write_striped, StripedPage};
use support::{write_grey, GreyPage};
use tiff::encoder::{Compression, DeflateLevel};

/// The empty rectangles every reader is checked with: no width, no height,
/// and neither. Each one is inside every fixture's plane.
fn empty_rects() -> [PixelRect; 3] {
    [
        PixelRect::new(0, 0, 0, 1),
        PixelRect::new(0, 0, 1, 0),
        PixelRect::new(0, 0, 0, 0),
    ]
}

/// Assert that an empty rectangle gives an empty plane, and NOT an error.
///
/// The plane's shape is the rectangle's shape, and its samples are the
/// reader's own.
fn assert_empty_plane(reader: &dyn ImageReader, what: &str) {
    let scene = reader.meta().scene(0).expect("the scene");
    for rect in empty_rects() {
        let region = reader.region(0, 0, 0, 0, rect).unwrap_or_else(|err| {
            panic!("{what}: an empty rectangle is an empty plane, got {err:?}")
        });
        assert_eq!(
            region.shape,
            [rect.h, rect.w],
            "{what}: the empty plane's shape differs"
        );
        assert_eq!(
            region.samples, scene.samples,
            "{what}: the empty plane's samples differ"
        );
        assert!(
            region.data.is_empty(),
            "{what}: the empty plane holds no samples"
        );
    }
}

#[test]
fn an_empty_rectangle_of_a_tiff_plane_is_an_empty_plane() {
    let dir = scratch_dir("region_empty_tiff");
    let path = dir.join("empty.tif");
    write_grey(
        &path,
        None,
        &[GreyPage {
            width: 8,
            height: 6,
            value: 3,
        }],
    );
    let reader = TiffReader::open(&path).expect("open the TIFF");
    assert_empty_plane(&reader, "a TIFF plane");
}

#[test]
fn an_empty_rectangle_of_an_lsm_plane_is_an_empty_plane() {
    let dir = scratch_dir("region_empty_lsm");
    let path = dir.join("empty.lsm");
    write_lsm(&path, &LsmSpec::plain(8, 6, 1));
    let reader = LsmReader::open(&path).expect("open the LSM");
    assert_empty_plane(&reader, "an LSM plane");
}

#[test]
fn an_empty_rectangle_of_a_czi_scene_is_an_empty_plane() {
    let scratch = czi_scratch("region_empty_czi");
    write_czi(&scratch, &[Tile::full(0, 0, 8, 6)]);
    let reader = CziReader::open(&scratch).expect("open the CZI");
    assert_empty_plane(&reader, "a CZI scene");
}

#[test]
fn an_empty_rectangle_of_a_lif_image_is_an_empty_plane() {
    let dir = scratch_dir("region_empty_lif");
    let path = dir.join("empty.lif");
    write_lif(
        &path,
        &[LifImage::gray8("empty", "block", 4, 2, None, None, 1)],
    );
    let reader = LifReader::open(&path).expect("open the LIF");
    assert_empty_plane(&reader, "a LIF image");
}

#[test]
fn an_empty_rectangle_of_an_nd2_plane_is_an_empty_plane() {
    let dir = scratch_dir("region_empty_nd2");
    let path = dir.join("empty.nd2");
    write_nd2(&path, &Nd2File::gray16(8, 6, 1));
    let reader = Nd2Reader::open(&path).expect("open the ND2");
    assert_empty_plane(&reader, "an ND2 plane");
}

#[test]
fn an_empty_rectangle_of_a_compressed_tiff_page_is_an_empty_plane() {
    // A COMPRESSED page is read by decoding the chunks the window touches, so
    // the empty rectangle has to be settled before the reader counts the
    // chunks it would touch.
    let dir = scratch_dir("region_empty_tiff_compressed");
    let path = dir.join("empty_compressed.tif");
    write_striped(
        &path,
        None,
        &[StripedPage {
            width: 17,
            height: 32,
            samples: 1,
            sample_type: SampleType::U8,
            rows_per_strip: 1,
            planar: false,
        }],
        Compression::Deflate(DeflateLevel::default()),
    );
    let reader = TiffReader::open(&path).expect("open the TIFF");
    assert!(
        reader.supports_region(),
        "a compressed page whose chunks are a small part of the plane is served natively"
    );
    assert_empty_plane(&reader, "a compressed TIFF plane");
}

#[test]
fn an_empty_rectangle_still_checks_the_coordinate_first() {
    // The coordinate is wrong AND the rectangle is empty: the coordinate is
    // checked first, so the call is out of range.
    let dir = scratch_dir("region_empty_out_of_range");
    let path = dir.join("empty.tif");
    write_grey(
        &path,
        None,
        &[GreyPage {
            width: 8,
            height: 6,
            value: 3,
        }],
    );
    let reader = TiffReader::open(&path).expect("open the TIFF");
    let err = reader
        .region(0, 7, 0, 0, PixelRect::new(0, 0, 0, 0))
        .expect_err("a call with a wrong coordinate is refused");
    assert!(
        matches!(err, Error::OutOfRange { .. }),
        "a wrong coordinate is out of range, got {err:?}"
    );
}

/// The trait's default `region()` is the reference: an empty rectangle there
/// is an empty plane too.
#[test]
fn the_default_region_read_agrees_with_the_readers() {
    // The default path crops the whole plane, so the two paths are compared
    // on the same file.
    let dir = scratch_dir("region_empty_default");
    let path = dir.join("default.tif");
    write_grey(
        &path,
        None,
        &[GreyPage {
            width: 8,
            height: 6,
            value: 3,
        }],
    );
    let reader = TiffReader::open(&path).expect("open the TIFF");
    let plane = reader.plane(0, 0, 0, 0).expect("the plane");
    for rect in empty_rects() {
        let cropped: Plane = plane.crop(rect).expect("crop the plane");
        let region = reader.region(0, 0, 0, 0, rect).expect("the region");
        assert_eq!(
            region.shape, cropped.shape,
            "an empty rectangle has the cropped plane's shape"
        );
        assert_eq!(
            region.data, cropped.data,
            "an empty rectangle has the cropped plane's bytes"
        );
    }
}
