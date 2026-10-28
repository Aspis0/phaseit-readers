// SPDX-License-Identifier: Apache-2.0
//! The LSM reader reads a sub-region without decoding the whole plane.
//!
//! An LSM page stores every channel as its own planar block of strips, so
//! the rows of a rectangle sit in the strips that hold them. Every rectangle
//! the reader accepts must therefore give exactly the same bytes as cropping
//! the whole plane — and a strip read at the wrong place must give DIFFERENT
//! bytes, which is why the fixture's rows carry a pattern of their own.

mod support;

use phasit_imread::lsm_reader::LsmReader;
use phasit_imread::types::PixelRect;
use phasit_imread::{Error, ImageReader, Plane};

use support::lsm::{build, ifd_entry_value, LsmSpec};
use support::scratch_dir;

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

/// The rectangles every plane is checked with.
fn rects(width: usize, height: usize) -> [PixelRect; 6] {
    [
        PixelRect::new(0, 0, width, height), // the whole plane
        PixelRect::new(0, 0, 1, 1),           // one pixel at the origin
        PixelRect::new(width - 1, height - 1, 1, 1), // one pixel at the far corner
        PixelRect::new(1, 1, width - 2, height - 2), // an unaligned window
        PixelRect::new(2, 0, 1, height),      // a single column
        PixelRect::new(0, 1, width, 1),       // a single row
    ]
}

/// The number of pages the spec's counts imply.
fn pages_of(spec: &LsmSpec) -> usize {
    let scenes = (spec.positions.max(0) as usize) * (spec.tiles.max(0) as usize);
    let scenes = if scenes == 0 { 1 } else { scenes };
    scenes * spec.t * spec.z
}

/// Write an LSM file whose rows carry a pattern of their own, and open it.
///
/// WHY patch the data: the writer gives every channel one flat value, so a
/// window read from the wrong row would look the same as the right one.
fn write_and_open(spec: &LsmSpec, name: &str) -> (std::path::PathBuf, LsmReader) {
    let (mut bytes, offsets) = build(spec);
    let plane_bytes = (spec.x * spec.y) as usize;
    let strips_per_channel = spec.strips_per_channel.max(1);
    let strip_bytes = plane_bytes / strips_per_channel;
    for strip in 0..(pages_of(spec) * spec.channels * strips_per_channel) {
        let value = (strip as u32 * 31) % 251;
        for y in 0..(strip_bytes / spec.x as usize) {
            let at = offsets.data + strip * strip_bytes + y * spec.x as usize;
            for x in 0..spec.x as usize {
                bytes[at + x] = ((value + y as u32) % 251) as u8;
            }
        }
    }
    let dir = scratch_dir(name);
    let path = dir.join("pattern.lsm");
    std::fs::write(&path, bytes).expect("write the LSM");
    let reader = LsmReader::open(&path).expect("open the LSM");
    (path, reader)
}

#[test]
fn the_reader_advertises_region_support() {
    let spec = LsmSpec::plain(4, 3, 1);
    let (_path, reader) = write_and_open(&spec, "lsm_region_advertise");
    assert!(
        reader.supports_region(),
        "the LSM reader reads a sub-region without decoding the whole plane"
    );
}

#[test]
fn a_region_of_a_plane_equals_the_cropped_plane() {
    let spec = LsmSpec::plain(5, 3, 2);
    let (_path, reader) = write_and_open(&spec, "lsm_region_rects");
    for z in 0..spec.z {
        let plane = reader.plane(0, 0, 0, z).expect("decode the plane");
        for rect in rects(spec.x as usize, spec.y as usize) {
            let region = reader.region(0, 0, 0, z, rect).expect("read the region");
            let cropped = plane.crop(rect).expect("crop the plane");
            assert_same(&region, &cropped, "a region of an LSM plane");
        }
    }
}

#[test]
fn a_region_of_every_channel_equals_the_cropped_plane() {
    // Every channel is its own block of strips, so a window must follow the
    // channel it addresses.
    let spec = LsmSpec {
        channels: 3,
        ..LsmSpec::plain(4, 3, 1)
    };
    let (_path, reader) = write_and_open(&spec, "lsm_region_channels");
    for c in 0..spec.channels {
        let plane = reader.plane(0, 0, c, 0).expect("decode the plane");
        for rect in rects(spec.x as usize, spec.y as usize) {
            let region = reader.region(0, 0, c, 0, rect).expect("read the region");
            let cropped = plane.crop(rect).expect("crop the plane");
            assert_same(&region, &cropped, "a region of an LSM channel");
        }
    }
}

#[test]
fn a_region_of_a_tile_scan_scene_equals_the_cropped_plane() {
    // A file with several tile positions holds several scenes, each with its
    // own pages.
    let spec = LsmSpec {
        positions: 2,
        tiles: 3,
        ..LsmSpec::plain(4, 3, 1)
    };
    let (_path, reader) = write_and_open(&spec, "lsm_region_scenes");
    let scenes = reader.meta().scenes.len();
    assert_eq!(scenes, 6, "six tile positions");
    for scene in 0..scenes {
        let plane = reader.plane(scene, 0, 0, 0).expect("decode the plane");
        for rect in rects(spec.x as usize, spec.y as usize) {
            let region = reader.region(scene, 0, 0, 0, rect).expect("read the region");
            let cropped = plane.crop(rect).expect("crop the plane");
            assert_same(&region, &cropped, "a region of an LSM tile-scan scene");
        }
    }
}

#[test]
fn a_region_of_a_multi_strip_page_equals_the_cropped_plane() {
    // Three strips per channel: the window's rows are gathered out of ALL of
    // them, and a row read from the wrong strip does not match.
    let spec = LsmSpec {
        strips_per_channel: 3,
        ..LsmSpec::plain(4, 6, 1)
    };
    let (_path, reader) = write_and_open(&spec, "lsm_region_multi_strip");
    assert!(
        reader.supports_region(),
        "a page whose strips hold whole rows is served a window at a time"
    );
    let plane = reader.plane(0, 0, 0, 0).expect("decode the plane");
    for rect in rects(spec.x as usize, spec.y as usize) {
        let region = reader.region(0, 0, 0, 0, rect).expect("read the region");
        let cropped = plane.crop(rect).expect("crop the plane");
        assert_same(&region, &cropped, "a region of a multi-strip LSM plane");
    }
}

#[test]
fn a_region_of_a_multi_strip_page_of_every_channel_equals_the_cropped_plane() {
    // Every channel is its own block of strips, and every block has several.
    let spec = LsmSpec {
        channels: 2,
        strips_per_channel: 2,
        ..LsmSpec::plain(4, 4, 1)
    };
    let (_path, reader) = write_and_open(&spec, "lsm_region_multi_strip_channels");
    for c in 0..spec.channels {
        let plane = reader.plane(0, 0, c, 0).expect("decode the plane");
        for rect in rects(spec.x as usize, spec.y as usize) {
            let region = reader.region(0, 0, c, 0, rect).expect("read the region");
            let cropped = plane.crop(rect).expect("crop the plane");
            assert_same(&region, &cropped, "a region of a multi-strip LSM channel");
        }
    }
}

#[test]
fn a_region_outside_the_plane_is_refused() {
    let spec = LsmSpec::plain(5, 3, 1);
    let (_path, reader) = write_and_open(&spec, "lsm_region_bad_rect");
    for rect in [
        PixelRect::new(5, 0, 1, 1),
        PixelRect::new(0, 3, 1, 1),
        PixelRect::new(0, 0, 6, 1),
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
    let spec = LsmSpec::plain(5, 3, 1);
    let (_path, reader) = write_and_open(&spec, "lsm_region_bad_coordinate");

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

/// The width and height of a plane over the decode ceiling: 46341 x 46342
/// 16-bit pixels is 4.295 GB, past the 4 GiB ceiling. The width and the
/// height are 16-bit fields, so this is as wide as an LSM page can be.
///
/// The height is EVEN, so the page's two strips hold a whole number of rows
/// each — the layout a real file has, and the one the window path needs.
const OVER_CEILING_WIDTH: u16 = 46_341;
const OVER_CEILING_HEIGHT: u16 = 46_342;

/// A file whose plane is over the decode ceiling.
///
/// WHY patch the file: the writer writes the pixels of every plane, and a
/// 4.3 GB plane is more than a test may write. The file's own tables are
/// patched instead — the geometry, the sample type, and the strips the page
/// states — so the file says the same thing a real over-ceiling file says.
fn over_ceiling_file() -> Vec<u8> {
    let spec = LsmSpec::plain(4, 3, 1);
    let (mut bytes, offsets) = build(&spec);
    let row_bytes = u64::from(OVER_CEILING_WIDTH) * 2;
    let strip_bytes = (u64::from(OVER_CEILING_HEIGHT) / 2 * row_bytes) as u32;

    // The CZ-LSM-Info struct: DimensionX at +8, DimensionY at +12 (32-bit
    // fields).
    bytes[offsets.cz + 8..offsets.cz + 12]
        .copy_from_slice(&u32::from(OVER_CEILING_WIDTH).to_le_bytes());
    bytes[offsets.cz + 12..offsets.cz + 16]
        .copy_from_slice(&u32::from(OVER_CEILING_HEIGHT).to_le_bytes());

    // The page's IFD: the geometry, the sample type, and the two strips the
    // page holds. The width, the height and the bits per sample are 16-bit
    // fields.
    let width = ifd_entry_value(&bytes, 0, 256).expect("the ImageWidth entry");
    let height = ifd_entry_value(&bytes, 0, 257).expect("the ImageLength entry");
    let bits = ifd_entry_value(&bytes, 0, 258).expect("the BitsPerSample entry");
    bytes[width..width + 2].copy_from_slice(&OVER_CEILING_WIDTH.to_le_bytes());
    bytes[height..height + 2].copy_from_slice(&OVER_CEILING_HEIGHT.to_le_bytes());
    bytes[bits..bits + 2].copy_from_slice(&16u16.to_le_bytes());

    let strip_offsets_at = bytes.len();
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    let strip_counts_at = bytes.len();
    bytes.extend_from_slice(&strip_bytes.to_le_bytes());
    bytes.extend_from_slice(&strip_bytes.to_le_bytes());
    let offsets_entry = ifd_entry_value(&bytes, 0, 273).expect("the StripOffsets entry");
    let counts_entry = ifd_entry_value(&bytes, 0, 279).expect("the StripByteCounts entry");
    bytes[offsets_entry - 4..offsets_entry].copy_from_slice(&2u32.to_le_bytes());
    bytes[offsets_entry..offsets_entry + 4]
        .copy_from_slice(&(strip_offsets_at as u32).to_le_bytes());
    bytes[counts_entry - 4..counts_entry].copy_from_slice(&2u32.to_le_bytes());
    bytes[counts_entry..counts_entry + 4].copy_from_slice(&(strip_counts_at as u32).to_le_bytes());
    bytes
}

#[test]
fn a_file_whose_plane_is_over_the_ceiling_is_published() {
    // The decode ceiling guards ONE materialisation, not the file's
    // existence: the scene is PUBLISHED, `plane()` refuses the
    // materialisation, and `region()` serves windows of it — the CZI, LIF
    // and ND2 rule.
    //
    // The pixels of a 4.9 GB plane are more than a test may write, so this
    // test pins the DECISION: the file opens, and `plane()` refuses the
    // materialisation with the oversized error, not with a parse error. The
    // shared decision itself is pinned in `tests/plane_ceiling.rs`.
    let dir = scratch_dir("lsm_region_over_ceiling");
    let path = dir.join("over_ceiling.lsm");
    std::fs::write(&path, over_ceiling_file()).expect("write the LSM");
    let reader = LsmReader::open(&path).expect("the file is published");
    assert_eq!(reader.meta().scene_count(), 1, "the file is published");

    match reader.plane(0, 0, 0, 0) {
        Err(Error::OversizedAllocation { .. }) => {}
        Err(other) => panic!("the plane must be refused as oversized, got {other}"),
        Ok(_) => panic!("the plane must not be materialised"),
    }
}

#[test]
fn a_region_of_an_over_ceiling_page_is_not_refused_for_the_ceiling() {
    // The window path of a page whose plane is over the ceiling does NOT
    // refuse the window for the ceiling: the ceiling guards what is
    // MATERIALISED, and a window is not the plane.
    //
    // This fixture's strips are not in the file — a 4.3 GB plane is more than
    // a test may write — so the read fails for THAT reason, and the failure
    // says so. What is pinned here is the DECISION: the window path is taken,
    // and the window is not refused as oversized.
    let dir = scratch_dir("lsm_region_over_ceiling_window");
    let path = dir.join("over_ceiling.lsm");
    std::fs::write(&path, over_ceiling_file()).expect("write the LSM");
    let reader = LsmReader::open(&path).expect("the file is published");
    let err = reader
        .region(0, 0, 0, 0, PixelRect::new(0, 0, 2, 2))
        .expect_err("the fixture's strips are not in the file");
    assert!(
        !matches!(err, Error::OversizedAllocation { .. }),
        "the window of an over-ceiling page is not refused for the ceiling, got {err:?}"
    );
}
