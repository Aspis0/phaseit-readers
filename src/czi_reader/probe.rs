//! A measurement probe for the CZI reader: how long a tile decode costs and how many tile decodes a
//! pyramid-sized chunk grid needs. Ignored by default; run with `--release --ignored --nocapture`.

use std::path::PathBuf;
use std::time::Instant;

use super::CziScene;
use crate::czi_reader::CziReader;
use crate::types::{ImageReader, PixelRect};

const REAL_DATA_DIR_VAR: &str = "PHASIT_REAL_DATA_DIR";

fn sample_path() -> Option<PathBuf> {
    let dir = std::env::var_os(REAL_DATA_DIR_VAR)?;
    if dir.is_empty() {
        return None;
    }
    let path = PathBuf::from(dir).join("czi").join("brightfield-jxr.czi");
    path.exists().then_some(path)
}

/// The tiles of the scene's first plane, in the order they are composed.
fn plane_tiles(scene: &CziScene) -> Vec<usize> {
    let key = (scene.c_values[0], scene.z_values[0], scene.t_values[0]);
    scene.tiles.get(&key).cloned().unwrap_or_default()
}

/// How many of the scene's tiles meet `rect`.
fn tiles_meeting(reader: &CziReader, tiles: &[usize], rect: PixelRect) -> Vec<usize> {
    let scene = &reader.scenes[0];
    let x1 = rect.x + rect.w;
    let y1 = rect.y + rect.h;
    tiles
        .iter()
        .copied()
        .filter(|&idx| {
            let sb = &reader.subblocks[idx];
            let tx0 = i64::from(sb.start_x) - i64::from(scene.origin_x);
            let ty0 = i64::from(sb.start_y) - i64::from(scene.origin_y);
            let tx1 = tx0 + sb.stored_x as i64;
            let ty1 = ty0 + sb.stored_y as i64;
            tx0 < x1 as i64 && tx1 > rect.x as i64 && ty0 < y1 as i64 && ty1 > rect.y as i64
        })
        .collect()
}

#[test]
#[ignore = "timing probe, not a correctness test"]
fn czi_probe_decode_costs() {
    let Some(path) = sample_path() else {
        eprintln!("{REAL_DATA_DIR_VAR} is not set: the probe skips");
        return;
    };

    let opened = Instant::now();
    let reader = CziReader::open(&path).expect("open the Axioscan CZI");
    let open_secs = opened.elapsed().as_secs_f64();
    let meta = reader.meta();
    let scene_meta = &meta.scenes[0];
    eprintln!(
        "open {open_secs:.3} s; scene 0 = {}x{} px, {} samples, dtype {:?}, {} scenes",
        scene_meta.dims.x,
        scene_meta.dims.y,
        scene_meta.samples,
        scene_meta.dtype,
        meta.scenes.len()
    );

    let tiles = plane_tiles(&reader.scenes[0]);
    eprintln!("scene 0 holds {} tiles", tiles.len());

    // The tile geometry of a few tiles.
    for &idx in tiles.iter().take(3) {
        let sb = &reader.subblocks[idx];
        eprintln!(
            "  tile {idx}: stored {}x{}, start ({}, {}), {} compressed bytes",
            sb.stored_x,
            sb.stored_y,
            sb.start_x,
            sb.start_y,
            sb.data_size
        );
    }

    // One tile decode, averaged over a few tiles.
    let sample = tiles.len().min(8);
    let started = Instant::now();
    let mut decoded_bytes = 0usize;
    for &idx in tiles.iter().take(sample) {
        let tile = reader.decode_tile(&reader.subblocks[idx]).expect("decode the tile");
        decoded_bytes += tile.len();
    }
    let per_tile = started.elapsed().as_secs_f64() / sample as f64;
    eprintln!(
        "tile decode: {per_tile:.4} s/tile ({sample} tiles, {decoded_bytes} bytes) \
         = {:.1} MB/s per core",
        decoded_bytes as f64 / per_tile / 1e6
    );

    // A chunk-sized region read, and how many tiles it decodes.
    let chunk = 512usize;
    for (rx, ry) in [
        (0usize, 0usize),
        (1000, 1000),
        (40_000, 30_000),
        (70_000, 50_000),
    ] {
        let rect = PixelRect::new(rx, ry, chunk, chunk);
        let started = Instant::now();
        let plane = reader.region(0, 0, 0, 0, rect).expect("read the region");
        let secs = started.elapsed().as_secs_f64();
        let met = tiles_meeting(&reader, &tiles, rect);
        eprintln!(
            "region ({rx}, {ry}) {chunk}x{chunk}: {secs:.4} s, {} tiles met, {} bytes",
            met.len(),
            plane.data.len()
        );
    }

    // The whole level-0 chunk grid: how many tile decodes does it need?
    let nx = scene_meta.dims.x.div_ceil(chunk);
    let ny = scene_meta.dims.y.div_ceil(chunk);
    let mut decodes = 0u64;
    let mut multi = 0u64;
    for ty in 0..ny {
        for tx in 0..nx {
            let rect = PixelRect::new(tx * chunk, ty * chunk, chunk, chunk);
            let met = tiles_meeting(&reader, &tiles, rect);
            decodes += met.len() as u64;
            if met.len() > 1 {
                multi += 1;
            }
        }
    }
    let chunks = (nx * ny) as u64;
    eprintln!(
        "level 0 grid: {chunks} chunks of {chunk}x{chunk}; {decodes} tile decodes \
         ({:.2} per chunk, {multi} chunks need more than one tile)",
        decodes as f64 / chunks as f64
    );
    eprintln!(
        "level 0 at {per_tile:.4} s/tile = {:.1} s of decode (one core), \
         plus {decodes} x {} bytes of tile output",
        decodes as f64 * per_tile,
        reader.subblocks[tiles[0]].stored_x * reader.subblocks[tiles[0]].stored_y * 3
    );

    // How well the tile decode scales over threads: the same tiles, decoded in parallel.
    for threads in [1usize, 2, 4, 8, 16] {
        let started = Instant::now();
        let rounds = 2usize;
        let batch = &tiles[..tiles.len().min(64)];
        std::thread::scope(|scope| {
            for _ in 0..threads {
                let reader = &reader;
                scope.spawn(move || {
                    for _ in 0..rounds {
                        for &idx in batch {
                            let _ = reader.decode_tile(&reader.subblocks[idx]);
                        }
                    }
                });
            }
        });
        let secs = started.elapsed().as_secs_f64();
        let decodes = rounds * batch.len();
        eprintln!(
            "parallel decode: {threads} threads -> {decodes} tiles in {secs:.3} s \
             = {:.1} tiles/s ({:.2} per thread)",
            decodes as f64 / secs,
            decodes as f64 / secs / threads as f64
        );
    }
}
