// SPDX-License-Identifier: Apache-2.0
//! The wall time of the reads over the levels a real file stores itself.
//!
//! The measurements run only when `PHASIT_LEVEL_TIMING` is set: they read a whole slide. Every
//! read starts at the scene's first source row and column, so the left band and the partial
//! first level pixels are part of what is timed.

mod real_data;

use std::time::Instant;

use phasit_imread::czi_reader::CziReader;
use phasit_imread::types::PixelRect;
use phasit_imread::ImageReader;

/// The reads the timing measures: the factors of the strip, and the whole face.
const FACTORS: [usize; 4] = [16, 32, 64, 256];

#[test]
fn the_reads_over_the_stored_levels_are_timed() {
    if std::env::var("PHASIT_LEVEL_TIMING").is_err() {
        return;
    }
    let Some(path) = real_data::real_file("czi/brightfield-jxr.czi") else {
        return;
    };

    let reader = CziReader::open(&path).expect("the Axioscan CZI opens");
    let scene = reader.meta().scene(0).expect("scene 0");
    let (width, height) = (scene.dims.x, scene.dims.y);
    println!("scene {width}x{height} samples {}", scene.samples);
    let stored = reader
        .stored_pyramid(0)
        .expect("the file's own pyramid is offered");

    // The whole face at the coarsest level.
    let face = PixelRect::new(0, 0, width, height);
    let started = Instant::now();
    let plane = stored
        .region(256, 0, 0, 0, face)
        .expect("the whole face reads at x256");
    println!(
        "FACE x256 shape {:?} ms {}",
        plane.plane.shape,
        started.elapsed().as_millis()
    );

    // The 4096-wide full-height strip at every level the strip is read at: the strip starts at
    // the scene's first row and column, whose level pixels are partial.
    let strip = PixelRect::new(0, 0, 4096.min(width), height);
    for factor in FACTORS {
        let started = Instant::now();
        let plane = stored
            .region(factor, 0, 0, 0, strip)
            .expect("the strip reads");
        println!(
            "STRIP x{factor} shape {:?} ms {}",
            plane.plane.shape,
            started.elapsed().as_millis()
        );
    }

    // Every level's pixels, read in the writer's blocks, fanned out one block per worker the way
    // the builder writes them (`into_par_iter` over the same blocks): what writing that level
    // from the file's own pyramid costs in reads.
    let chunk = 512usize;
    let side = 6 * chunk;
    let workers = std::thread::available_parallelism().map_or(1, |n| n.get());
    for level in stored.levels() {
        let factor = level.factor;
        let cols = width.div_ceil(factor);
        let rows = height.div_ceil(factor);
        let mut rects = Vec::new();
        for x0 in (0..cols).step_by(side) {
            for y0 in (0..rows).step_by(side) {
                let w = side.min(cols - x0);
                let h = side.min(rows - y0);
                rects.push(PixelRect::new(
                    x0 * factor,
                    y0 * factor,
                    w * factor,
                    h * factor,
                ));
            }
        }
        let blocks = rects.len();
        let next = std::sync::atomic::AtomicUsize::new(0);
        let started = Instant::now();
        let rects = &rects;
        let next = &next;
        let stored = &stored;
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(move || loop {
                    let idx = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if idx >= rects.len() {
                        break;
                    }
                    let read = stored
                        .region(factor, 0, 0, 0, rects[idx])
                        .expect("the block reads");
                    std::hint::black_box(&read);
                });
            }
        });
        println!(
            "BLOCKS x{factor} {blocks} blocks {workers} workers ms {}",
            started.elapsed().as_millis()
        );
    }
}
