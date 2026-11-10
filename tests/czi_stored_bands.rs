// SPDX-License-Identifier: Apache-2.0
//! The band a real slide's own pyramid leaves uncovered: the reads over the margin its grid
//! starts inside the scene, filled with the full resolution's box average.

mod real_data;
mod support;

use phasit_imread::czi_reader::CziReader;
use phasit_imread::types::PixelRect;
use phasit_imread::ImageReader;

use support::czi::plane_bytes;

/// The file's grid starts 190 source pixels into the scene, so the x2 level leaves the left band
/// without a tile: it reads as the full resolution's box average, blank where the full resolution
/// is blank. The first rect starts at the scene's first row and column, whose level pixels are
/// partial.
#[cfg(feature = "jpegxr")]
#[test]
fn the_real_whole_slide_fills_its_left_band() {
    let Some(path) = real_data::real_file("czi/brightfield-jxr.czi") else {
        return;
    };

    let reader = CziReader::open(&path).expect("the Axioscan CZI opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");

    let rect = PixelRect::new(0, 0, 190, 64);
    let band = stored.region(2, 0, 0, 0, rect).expect("the band reads");
    assert_eq!(band.plane.shape, [33, 95], "the band's pixels over the rect");
    let level = stored
        .levels()
        .iter()
        .find(|level| level.factor == 2)
        .expect("the file stores an x2 level");
    let average_of = |rect: PixelRect, plane: &phasit_imread::Plane, shape: [usize; 2]| {
        let full = reader
            .region(0, 0, 0, 0, rect)
            .expect("the full resolution reads");
        let source = plane_bytes(&full);
        let read = plane_bytes(plane);
        // The scene's pixels each level pixel covers. The level's rows and columns sit on the
        // lines of the level's origin, so the pixel that holds the rect's first row or column is
        // partial.
        let span_of = |index: usize, phase: i64, len: usize| -> (usize, usize) {
            let at = index as i64 * 2 + if phase == 0 { 0 } else { phase - 2 };
            (at.max(0) as usize, (at + 2).min(len as i64) as usize)
        };
        let phase_x = (level.origin[0] - rect.x as i64).rem_euclid(2);
        let phase_y = (level.origin[1] - rect.y as i64).rem_euclid(2);
        let average = |row: usize, col: usize, channel: usize| -> u8 {
            let (x0, x1) = span_of(col, phase_x, rect.w);
            let (y0, y1) = span_of(row, phase_y, rect.h);
            let (mut sum, mut count) = (0u32, 0u32);
            for y in y0..y1 {
                for x in x0..x1 {
                    sum += u32::from(source[(y * rect.w + x) * 3 + channel]);
                    count += 1;
                }
            }
            ((sum + count / 2) / count) as u8
        };
        for row in 0..shape[0] {
            for col in 0..shape[1] {
                for channel in 0..3 {
                    let index = (row * shape[1] + col) * 3 + channel;
                    assert!(
                        read[index].abs_diff(average(row, col, channel)) <= 1,
                        "pixel ({row},{col},{channel}): the band reads {}, the downsample reads {}",
                        read[index],
                        average(row, col, channel)
                    );
                }
            }
        }
    };
    average_of(rect, &band.plane, [33, 95]);

    // Where the slide reaches into the band, the fill carries the full resolution's pixels, not
    // zeros.
    let rect = PixelRect::new(0, 60_001, 190, 64);
    let band = stored.region(2, 0, 0, 0, rect).expect("the band reads");
    assert_eq!(band.plane.shape, [32, 95], "the band's pixels over the rect");
    assert!(
        plane_bytes(&band.plane).iter().any(|&byte| byte != 0),
        "the band carries the full resolution's pixels, not zeros"
    );
    average_of(rect, &band.plane, [32, 95]);
}

/// The left band at the mid levels: the rects the old contract could not address at all. The band
/// is pure fill (no tile reaches it), so every pixel is exactly the full resolution's box
/// average over the source pixels it covers.
#[cfg(feature = "jpegxr")]
#[test]
fn the_real_whole_slide_fills_its_left_band_at_the_mid_levels() {
    let Some(path) = real_data::real_file("czi/brightfield-jxr.czi") else {
        return;
    };

    let reader = CziReader::open(&path).expect("the Axioscan CZI opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");
    let rect = PixelRect::new(0, 0, 190, 64);
    let full = reader
        .region(0, 0, 0, 0, rect)
        .expect("the full resolution reads");
    let source = plane_bytes(&full);
    for factor in [4usize, 8, 16, 32] {
        let band = stored
            .region(factor, 0, 0, 0, rect)
            .expect("the band reads at the mid level");
        let read = plane_bytes(&band.plane);
        let [rows, cols] = band.plane.shape;
        let [origin_x, origin_y] = band.origin;
        for row in 0..rows {
            let y0 = (origin_y + row as i64 * factor as i64).max(0) as usize;
            let y1 = (origin_y + (row + 1) as i64 * factor as i64)
                .max(0) as usize;
            let y1 = y1.min(rect.y + rect.h);
            for col in 0..cols {
                let x0 = (origin_x + col as i64 * factor as i64).max(0) as usize;
                let x1 = ((origin_x + (col + 1) as i64 * factor as i64).max(0) as usize)
                    .min(rect.x + rect.w);
                for channel in 0..3 {
                    let (mut sum, mut count) = (0u64, 0u64);
                    for y in y0..y1 {
                        for x in x0..x1 {
                            sum += u64::from(source[(y * rect.w + x) * 3 + channel]);
                            count += 1;
                        }
                    }
                    let average = ((sum + count / 2) / count) as u8;
                    assert_eq!(
                        read[(row * cols + col) * 3 + channel],
                        average,
                        "factor x{factor}, pixel ({row},{col}), channel {channel}: \
                         the fill is the box average"
                    );
                }
            }
        }
    }
}
