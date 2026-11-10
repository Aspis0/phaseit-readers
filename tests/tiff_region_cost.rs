// SPDX-License-Identifier: Apache-2.0
//! A window read is worth it only while cheaper than decoding the whole plane: uncompressed pages are copied out of the
//! file,
//! compressed pages' chunks are decoded per window. The cost rule is decided at open, so tests check the bytes and the
//! path taken.

mod support;

use phasit_imread::tiff_reader::TiffReader;
use phasit_imread::types::PixelRect;
use phasit_imread::{Error, ImageReader, Plane, PlaneBuf};

use support::scratch_dir;
use support::tiff_layout::write_page;
use support::tiff_page::{Layout, Page, Pattern, SampleType};
use support::tiff_striped::{write_striped, StripedPage};
use tiff::encoder::{Compression, DeflateLevel};

const WIDTH: u32 = 17;
const HEIGHT: u32 = 11;

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

fn rects(width: u32, height: u32) -> [PixelRect; 8] {
    let (w, h) = (width as usize, height as usize);
    [
        PixelRect::new(0, 0, w, h),
        PixelRect::new(0, 0, 1, 1),
        PixelRect::new(w - 1, h - 1, 1, 1),
        PixelRect::new(3, 2, 7, 5),
        PixelRect::new(w - 2, h - 2, 2, 2),
        PixelRect::new(8, 0, 1, h),
        PixelRect::new(0, 5, w, 1),
        PixelRect::new(5, 4, 9, 7),
    ]
}

fn check_scene(reader: &TiffReader, width: u32, height: u32) {
    let sm = reader.meta().scene(0).expect("the scene");
    for t in 0..sm.dims.t {
        for c in 0..sm.dims.c {
            for z in 0..sm.dims.z {
                let plane = reader.plane(0, t, c, z).expect("decode the plane");
                for rect in rects(width, height) {
                    let region = reader.region(0, t, c, z, rect).expect("read the region");
                    let cropped = plane.crop(rect).expect("crop the plane");
                    assert_same(&region, &cropped, "a region of a TIFF plane");
                }
            }
        }
    }
}

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
fn a_region_of_a_single_strip_page_equals_the_cropped_plane() {
    // One strip: the uncompressed path copies the window's bytes out of the file.
    let dir = scratch_dir("tiff_region_single_strip");
    let path = dir.join("single_strip.tif");
    write_striped(&path, None, &[striped(HEIGHT)], Compression::Uncompressed);
    let reader = TiffReader::open(&path).expect("open the TIFF");
    assert!(
        reader.supports_region(),
        "an uncompressed page is served a window at a time"
    );
    check_scene(&reader, WIDTH, HEIGHT);
}

#[test]
fn a_region_of_a_compressed_page_with_small_chunks_equals_the_cropped_plane() {
    // One row per strip of a 32-row plane: each chunk is a small part of the plane, so the chunk path is cheaper.
    const TALL: u32 = 32;
    let dir = scratch_dir("tiff_region_small_chunks");
    let path = dir.join("small_chunks.tif");
    let mut page = striped(1);
    page.height = TALL;
    write_striped(
        &path,
        None,
        &[page],
        Compression::Deflate(DeflateLevel::default()),
    );
    let reader = TiffReader::open(&path).expect("open the TIFF");
    assert!(
        reader.supports_region(),
        "a compressed page whose chunks are a small part of the plane is served natively"
    );
    check_scene(&reader, WIDTH, TALL);
}

#[test]
fn a_compressed_page_whose_chunk_is_the_plane_falls_back_to_the_default_path() {
    // One chunk for the whole plane: decoding it per window would repeat the decode, so the default path is kept.
    let dir = scratch_dir("tiff_region_whole_plane_chunk");
    let path = dir.join("whole_plane_chunk.tif");
    write_striped(
        &path,
        None,
        &[striped(HEIGHT)],
        Compression::Deflate(DeflateLevel::default()),
    );
    let reader = TiffReader::open(&path).expect("open the TIFF");
    assert!(
        !reader.supports_region(),
        "a compressed page whose chunk IS the plane is not served natively"
    );
    check_scene(&reader, WIDTH, HEIGHT);
}

#[test]
fn a_region_of_a_signed_16_bit_page_equals_the_cropped_plane() {
    // Signed 16-bit: the window read must byteswap the file's bytes the same way as the plane read.
    let dir = scratch_dir("tiff_region_i16");
    let path = dir.join("i16.tif");
    let mut page = striped(4);
    page.sample_type = SampleType::I16;
    write_striped(&path, None, &[page], Compression::Uncompressed);
    let reader = TiffReader::open(&path).expect("open the TIFF");
    check_scene(&reader, WIDTH, HEIGHT);
}

#[test]
fn a_region_of_an_indexed_page_equals_the_cropped_plane() {
    // Indices are the pixels, and whole-row strips let the window be copied out.
    let dir = scratch_dir("tiff_region_indexed");
    let path = dir.join("indexed.tif");
    write_page(
        &path,
        &Page {
            width: WIDTH,
            height: HEIGHT,
            samples: 1,
            sample_type: SampleType::U8,
            layout: Layout::IndexedStrips { rows_per_strip: 4 },
        },
    );
    let reader = TiffReader::open(&path).expect("open the TIFF");
    assert!(
        reader.supports_region(),
        "an indexed page whose strips hold whole rows is served a window at a time"
    );
    check_scene(&reader, WIDTH, HEIGHT);
}

#[test]
fn an_indexed_page_whose_strips_split_a_row_falls_back_to_the_default_path() {
    // The first strip ends mid-row, so the page keeps the default path.
    let dir = scratch_dir("tiff_region_indexed_split");
    let path = dir.join("indexed_split.tif");
    write_page(
        &path,
        &Page {
            width: WIDTH,
            height: HEIGHT,
            samples: 1,
            sample_type: SampleType::U8,
            layout: Layout::IndexedSplitStrip { first_bytes: 100 },
        },
    );
    let reader = TiffReader::open(&path).expect("open the TIFF");
    assert!(
        !reader.supports_region(),
        "an indexed page whose strips do not hold whole rows is not served natively"
    );
    check_scene(&reader, WIDTH, HEIGHT);
}

#[test]
fn an_indexed_page_whose_strips_contradict_the_rows_per_strip_falls_back() {
    // The strips hold five rows where the tag says four: the window path would disagree with the plane, so the default
    // path stays.
    let dir = scratch_dir("tiff_region_indexed_shifted");
    let path = dir.join("indexed_shifted.tif");
    write_page(
        &path,
        &Page {
            width: WIDTH,
            height: HEIGHT,
            samples: 1,
            sample_type: SampleType::U8,
            layout: Layout::IndexedShiftedStrips { rows_per_strip: 4 },
        },
    );
    let reader = TiffReader::open(&path).expect("open the TIFF");
    assert!(
        !reader.supports_region(),
        "an indexed page whose strips contradict RowsPerStrip is not served natively"
    );
    check_scene(&reader, WIDTH, HEIGHT);
}

const OVER_CEILING: u32 = 46_342;

#[test]
fn a_region_of_an_over_ceiling_page_is_read_and_its_plane_is_not() {
    // The plane is over the ceiling and its tiles share one block: it cannot be materialised, but its windows can be
    // read.
    let dir = scratch_dir("tiff_region_over_ceiling");
    let path = dir.join("over_ceiling.tif");
    write_page(
        &path,
        &Page {
            width: OVER_CEILING,
            height: OVER_CEILING,
            samples: 1,
            sample_type: SampleType::U16,
            layout: Layout::SharedTiles {
                tile_width: 512,
                tile_length: 512,
            },
        },
    );
    let reader = TiffReader::open(&path).expect("open the TIFF");
    assert!(
        reader.supports_region(),
        "an uncompressed page is served a window at a time, over the ceiling or not"
    );

    let err = reader
        .plane(0, 0, 0, 0)
        .expect_err("the plane is over the ceiling");
    assert!(
        matches!(err, Error::OversizedAllocation { .. }),
        "the plane of an over-ceiling page is refused as oversized, got {err:?}"
    );

    let region = reader
        .region(0, 0, 0, 0, PixelRect::new(0, 0, 4, 2))
        .expect("read the region of the over-ceiling page");
    let PlaneBuf::U16(pixels) = &region.data else {
        panic!("the region holds 16-bit samples");
    };
    let expected: Vec<u16> = (0..2)
        .flat_map(|y| (0..4).map(move |x| u16::at(x as u32, y as u32, 0)))
        .collect();
    assert!(
        pixels == &expected,
        "the region's bytes are the file's own bytes"
    );
}

#[test]
fn a_region_of_a_compressed_tiled_page_equals_the_cropped_plane() {
    // Compressed tiles hang over the page's edge: the crate clips them, and the window must follow.
    const SIDE: u32 = 36;
    let dir = scratch_dir("tiff_region_compressed_tiles");
    let path = dir.join("compressed_tiles.tif");
    write_page(
        &path,
        &Page {
            width: SIDE,
            height: SIDE,
            samples: 1,
            sample_type: SampleType::U8,
            layout: Layout::CompressedTiles {
                width: 8,
                length: 8,
            },
        },
    );
    let reader = TiffReader::open(&path).expect("open the TIFF");
    assert!(
        reader.supports_region(),
        "a compressed tiled page whose chunks are small next to the plane is served natively"
    );
    check_scene(&reader, SIDE, SIDE);
}

#[test]
fn a_doubly_invalid_call_is_out_of_range() {
    // A wrong coordinate and a wrong rectangle: the coordinate is checked first.
    let dir = scratch_dir("tiff_region_doubly_invalid");
    let path = dir.join("doubly_invalid.tif");
    write_striped(&path, None, &[striped(4)], Compression::Uncompressed);
    let reader = TiffReader::open(&path).expect("open the TIFF");
    let err = reader
        .region(0, 9, 0, 0, PixelRect::new(0, 0, WIDTH as usize + 5, 5))
        .expect_err("a call that is wrong in both ways is refused");
    assert!(
        matches!(err, Error::OutOfRange { .. }),
        "a doubly-invalid call is out of range, got {err:?}"
    );
}

#[test]
fn a_rectangle_outside_the_plane_is_a_decode_error() {
    // A right coordinate and a wrong rectangle: a decode error, the order every reader checks in.
    let dir = scratch_dir("tiff_region_outside");
    let path = dir.join("outside.tif");
    write_striped(&path, None, &[striped(4)], Compression::Uncompressed);
    let reader = TiffReader::open(&path).expect("open the TIFF");
    let err = reader
        .region(0, 0, 0, 0, PixelRect::new(0, 0, WIDTH as usize + 1, 5))
        .expect_err("a rectangle outside the plane is refused");
    assert!(
        matches!(err, Error::Decode(_)),
        "a rectangle outside the plane is a decode error, got {err:?}"
    );
}
