// SPDX-License-Identifier: Apache-2.0
//! The TIFF reader reads a sub-region without decoding the whole page.
//!
//! A TIFF page is stored in strips or tiles, so the pixels of a rectangle sit
//! in the chunks that overlap it. Every rectangle the reader accepts must
//! therefore give exactly the same bytes as cropping the whole plane — and a
//! chunk read at the wrong place must give DIFFERENT bytes, which is why the
//! fixtures carry a pattern that changes with every row, column and sample.

mod support;

use phasit_imread::tiff_reader::TiffReader;
use phasit_imread::types::PixelRect;
use phasit_imread::{Error, ImageReader, Plane};

use support::scratch_dir;
use support::tiff_layout::write_page;
use support::tiff_page::{Layout, Page, SampleType};
use support::tiff_striped::{write_striped, StripedPage};
use tiff::encoder::{Compression, DeflateLevel};

/// The fixtures' size: neither the width nor the height divides by the strip
/// or tile size, so the last strip or tile of a page is a partial one.
const WIDTH: u32 = 17;
const HEIGHT: u32 = 11;

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

/// The rectangles every layout is checked with: the whole plane, one pixel at
/// each corner, a single row and a single column, and windows that fall on
/// no strip or tile boundary.
fn rects() -> [PixelRect; 8] {
    let (w, h) = (WIDTH as usize, HEIGHT as usize);
    [
        PixelRect::new(0, 0, w, h), // the whole plane
        PixelRect::new(0, 0, 1, 1), // one pixel at the origin
        PixelRect::new(w - 1, h - 1, 1, 1), // one pixel at the far corner
        PixelRect::new(3, 2, 7, 5), // an unaligned window
        PixelRect::new(w - 2, h - 2, 2, 2), // the last row and column
        PixelRect::new(8, 0, 1, h),  // a single column
        PixelRect::new(0, 5, w, 1),  // a single row
        PixelRect::new(5, 4, 9, 7), // a window over several chunks
    ]
}

/// Check every rectangle of every plane of the scene.
fn check_scene(reader: &TiffReader, scene: usize) {
    let sm = reader.meta().scene(scene).expect("the scene");
    for t in 0..sm.dims.t {
        for c in 0..sm.dims.c {
            for z in 0..sm.dims.z {
                let plane = reader.plane(scene, t, c, z).expect("decode the plane");
                for rect in rects() {
                    let region = reader.region(scene, t, c, z, rect).expect("read the region");
                    let cropped = plane.crop(rect).expect("crop the plane");
                    assert_same(&region, &cropped, "a region of a TIFF plane");
                }
            }
        }
    }
}

/// One striped page of the fixtures' size.
fn striped(rows_per_strip: u32) -> StripedPage {
    StripedPage {
        width: WIDTH,
        height: HEIGHT,
        samples: 1,
        sample_type: SampleType::U8,
        rows_per_strip,
        planar: false,
    }
}

#[test]
fn the_reader_advertises_region_support() {
    let dir = scratch_dir("tiff_region_advertise");
    let path = dir.join("plain.tif");
    write_striped(
        &path,
        None,
        &[striped(4)],
        Compression::Uncompressed,
    );
    let reader = TiffReader::open(&path).expect("open the TIFF");
    assert!(
        reader.supports_region(),
        "the TIFF reader reads a sub-region without decoding the whole page"
    );
}

#[test]
fn a_region_of_a_striped_page_equals_the_cropped_plane() {
    // Four rows per strip: 11 rows are two full strips and a partial one.
    let dir = scratch_dir("tiff_region_striped");
    let path = dir.join("striped.tif");
    write_striped(&path, None, &[striped(4)], Compression::Uncompressed);
    let reader = TiffReader::open(&path).expect("open the TIFF");
    check_scene(&reader, 0);
}

#[test]
fn a_region_of_a_compressed_page_equals_the_cropped_plane() {
    // The same layout, compressed: the chunks are decoded, not copied.
    for (name, compression) in [
        ("deflate", Compression::Deflate(DeflateLevel::default())),
        ("lzw", Compression::Lzw),
    ] {
        let dir = scratch_dir(&format!("tiff_region_{name}"));
        let path = dir.join(format!("{name}.tif"));
        write_striped(&path, None, &[striped(4)], compression);
        let reader = TiffReader::open(&path).expect("open the TIFF");
        check_scene(&reader, 0);
    }
}

#[test]
fn a_region_of_a_tiled_page_equals_the_cropped_plane() {
    // 8x8 tiles on a 17x11 page: the tiles do not divide the plane, so the
    // last row and column of tiles are partial ones.
    let dir = scratch_dir("tiff_region_tiled");
    let path = dir.join("tiled.tif");
    write_page(
        &path,
        &Page {
            width: WIDTH,
            height: HEIGHT,
            samples: 1,
            sample_type: SampleType::U8,
            layout: Layout::Tiles {
                width: 8,
                length: 8,
            },
        },
    );
    let reader = TiffReader::open(&path).expect("open the TIFF");
    check_scene(&reader, 0);
}

#[test]
fn a_region_of_a_tiled_16_bit_page_equals_the_cropped_plane() {
    let dir = scratch_dir("tiff_region_tiled16");
    let path = dir.join("tiled16.tif");
    write_page(
        &path,
        &Page {
            width: WIDTH,
            height: HEIGHT,
            samples: 1,
            sample_type: SampleType::U16,
            layout: Layout::Tiles {
                width: 8,
                length: 8,
            },
        },
    );
    let reader = TiffReader::open(&path).expect("open the TIFF");
    check_scene(&reader, 0);
}

#[test]
fn a_region_of_an_rgb_page_equals_the_cropped_plane() {
    // Three samples in one pixel: the samples stay together in the window.
    let dir = scratch_dir("tiff_region_rgb");
    let path = dir.join("rgb.tif");
    let mut page = striped(4);
    page.samples = 3;
    write_striped(&path, None, &[page], Compression::Uncompressed);
    let reader = TiffReader::open(&path).expect("open the TIFF");
    check_scene(&reader, 0);
}

#[test]
fn a_region_of_a_16_bit_page_equals_the_cropped_plane() {
    let dir = scratch_dir("tiff_region_u16");
    let path = dir.join("u16.tif");
    let mut page = striped(4);
    page.sample_type = SampleType::U16;
    write_striped(&path, None, &[page], Compression::Uncompressed);
    let reader = TiffReader::open(&path).expect("open the TIFF");
    check_scene(&reader, 0);
}

#[test]
fn a_region_of_a_float_page_equals_the_cropped_plane() {
    let dir = scratch_dir("tiff_region_f32");
    let path = dir.join("f32.tif");
    let mut page = striped(4);
    page.sample_type = SampleType::F32;
    write_striped(&path, None, &[page], Compression::Uncompressed);
    let reader = TiffReader::open(&path).expect("open the TIFF");
    check_scene(&reader, 0);
}

#[test]
fn a_region_of_a_planar_page_equals_the_cropped_plane() {
    // One sample per pixel in its own plane: the layout is the same as the
    // chunky one, and the reader serves it.
    let dir = scratch_dir("tiff_region_planar");
    let path = dir.join("planar.tif");
    let mut page = striped(4);
    page.planar = true;
    write_striped(&path, None, &[page], Compression::Uncompressed);
    let reader = TiffReader::open(&path).expect("open the TIFF");
    check_scene(&reader, 0);
}

#[test]
fn a_planar_multi_sample_page_is_refused() {
    // The page states that each sample sits in its own plane, with three
    // samples per pixel: this reader cannot address that, and says so
    // instead of reading the interleaved bytes as if they were planar.
    let dir = scratch_dir("tiff_region_planar_rgb");
    let path = dir.join("planar_rgb.tif");
    write_page(
        &path,
        &Page {
            width: WIDTH,
            height: HEIGHT,
            samples: 3,
            sample_type: SampleType::U8,
            layout: Layout::PlanarStrips { rows_per_strip: 4 },
        },
    );
    let err = match TiffReader::open(&path) {
        Ok(_) => panic!("a planar RGB page is refused"),
        Err(err) => err,
    };
    assert!(
        matches!(err, Error::Unsupported(_)),
        "a planar multi-sample page is refused as unsupported, got {err:?}"
    );
}

#[test]
fn a_region_of_a_second_page_equals_the_cropped_plane() {
    // Two pages are two Z planes of one scene, so the second page's chunks
    // must be read as the second plane.
    let dir = scratch_dir("tiff_region_pages");
    let path = dir.join("pages.tif");
    write_striped(
        &path,
        None,
        &[striped(4), striped(7)],
        Compression::Uncompressed,
    );
    let reader = TiffReader::open(&path).expect("open the TIFF");
    check_scene(&reader, 0);
}

#[test]
fn a_region_of_an_ome_plane_equals_the_cropped_plane() {
    let dir = scratch_dir("tiff_region_ome");
    let path = dir.join("ome.ome.tif");
    let xml = r#"<?xml version="1.0"?>
<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06" UUID="urn:uuid:f1">
  <Image Name="a"><Pixels DimensionOrder="XYCZT" Type="uint8" SizeX="17" SizeY="11" SizeZ="2" SizeC="1" SizeT="1">
    <TiffData IFD="0" FirstZ="0"/>
    <TiffData IFD="1" FirstZ="1"/>
  </Pixels></Image>
</OME>"#;
    write_striped(
        &path,
        Some(xml),
        &[striped(4), striped(4)],
        Compression::Uncompressed,
    );
    let reader = TiffReader::open(&path).expect("open the OME-TIFF");
    check_scene(&reader, 0);
}

#[test]
fn a_region_outside_the_plane_is_refused() {
    let dir = scratch_dir("tiff_region_bad_rect");
    let path = dir.join("plain.tif");
    write_striped(&path, None, &[striped(4)], Compression::Uncompressed);
    let reader = TiffReader::open(&path).expect("open the TIFF");

    // Past the right edge, past the bottom edge, and a rect whose extent
    // overflows the arithmetic.
    for rect in [
        PixelRect::new(WIDTH as usize, 0, 1, 1),
        PixelRect::new(0, HEIGHT as usize, 1, 1),
        PixelRect::new(0, 0, WIDTH as usize + 1, 1),
        PixelRect::new(usize::MAX, 0, 2, 1),
    ] {
        let err = reader
            .region(0, 0, 0, 0, rect)
            .expect_err("a rect outside the plane is refused");
        assert!(
            matches!(err, Error::Decode(_)),
            "a rect outside the plane is refused as a decode error, got {err:?}"
        );
    }
}

#[test]
fn a_region_of_a_plane_that_is_not_there_is_refused() {
    let dir = scratch_dir("tiff_region_bad_coordinate");
    let path = dir.join("plain.tif");
    write_striped(&path, None, &[striped(4)], Compression::Uncompressed);
    let reader = TiffReader::open(&path).expect("open the TIFF");

    // A scene that is not there is a scene out of range; a coordinate inside
    // a scene that exists is a coordinate out of range.
    let err = reader
        .region(1, 0, 0, 0, PixelRect::new(0, 0, 1, 1))
        .expect_err("a scene that is not there is refused");
    assert!(
        matches!(err, Error::SceneOutOfRange { .. }),
        "a scene that is not there is refused as a scene out of range, got {err:?}"
    );
    for (scene, t, c, z) in [(0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1)] {
        let err = reader
            .region(scene, t, c, z, PixelRect::new(0, 0, 1, 1))
            .expect_err("a plane that is not there is refused");
        assert!(
            matches!(err, Error::OutOfRange { .. }),
            "a plane that is not there is refused as out of range, got {err:?}"
        );
    }
}
