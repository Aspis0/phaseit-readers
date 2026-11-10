// SPDX-License-Identifier: Apache-2.0
//! The real LIF, ND2, TIFF and LSM files: every published scene's region is exactly the cropped plane.
//! Every `.lif`, `.nd2`, `.tif` and `.lsm` under `PHASIT_REAL_DATA_DIR` is opened, and several rectangles per plane are
//! compared in-process; the pixels are never printed.
//! EVERY published scene is compared: a skipped plane would be a hole in the coverage, so a plane that cannot be
//! materialised fails the run.
//! The run skips cleanly when the folder is unset, and fails when `PHASIT_REAL_DATA_REQUIRED=1` says it needs the data.

mod real_data;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use phasit_imread::lif_reader::LifReader;
use phasit_imread::lsm_reader::LsmReader;
use phasit_imread::nd2_reader::Nd2Reader;
use phasit_imread::tiff_reader::TiffReader;
use phasit_imread::types::PixelRect;
use phasit_imread::{Dims5, ImageReader};

use real_data::{absent, parse_real_data_dir, parse_required, Decision};

#[derive(Default)]
struct Counters {
    files: usize,
    scenes: usize,
    planes: usize,
    rectangles: usize,
    region_capable: usize,
    default: usize,
    layouts: BTreeMap<String, usize>,
}

fn real_data_dir() -> Option<PathBuf> {
    let required = parse_required(std::env::var_os("PHASIT_REAL_DATA_REQUIRED").as_deref());
    let dir = parse_real_data_dir(std::env::var_os("PHASIT_REAL_DATA_DIR").as_deref());
    match dir {
        Some(dir) => Some(dir),
        None => match absent("PHASIT_REAL_DATA_DIR is not set", required) {
            Decision::Skip(reason) => {
                eprintln!("skipping: {reason}");
                None
            }
            Decision::Required(reason) => panic!("{reason}"),
            Decision::Found(_) => unreachable!("the folder is not set"),
        },
    }
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect_files(&path, out);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("lif") | Some("nd2") | Some("tif") | Some("tiff") | Some("lsm")
        ) {
            out.push(path);
        }
    }
}

/// WHY: the reader does not publish a file's layout, so the test reads the tags that state it.
fn classify(path: &Path) -> String {
    if path.extension().and_then(|e| e.to_str()) == Some("lsm") {
        return "planar strips, uncompressed".to_string();
    }
    let Ok(file) = std::fs::File::open(path) else {
        return "unreadable".to_string();
    };
    let Ok(mut dec) = tiff::decoder::Decoder::new(std::io::BufReader::new(file)) else {
        return "unreadable".to_string();
    };
    let layout = match dec.get_chunk_type() {
        tiff::decoder::ChunkType::Tile => "tiles",
        tiff::decoder::ChunkType::Strip => "strips",
    };
    let compression = dec
        .get_tag(tiff::tags::Tag::Compression)
        .ok()
        .and_then(|value| value.into_u16().ok())
        .unwrap_or_default();
    format!("{layout}, compression {compression}")
}

fn probe_rects(dims: Dims5) -> Vec<PixelRect> {
    let x = dims.x.max(1);
    let y = dims.y.max(1);
    let w = x.min(3);
    let h = y.min(2);
    let mid_x = x / 3;
    let mid_y = y / 3;
    vec![
        PixelRect::new(0, 0, x, y),
        PixelRect::new(0, 0, 1, 1),
        PixelRect::new(x - 1, y - 1, 1, 1),
        PixelRect::new(x - w, y - h, w, h),
        PixelRect::new(
            mid_x,
            mid_y,
            w.min(x - mid_x).max(1),
            h.min(y - mid_y).max(1),
        ),
    ]
}

/// Compares every rectangle of a few planes of every scene in-process; the bytes are never printed.
fn compare_reader(reader: &dyn ImageReader, counters: &mut Counters) {
    counters.files += 1;
    if reader.supports_region() {
        counters.region_capable += 1;
    } else {
        counters.default += 1;
    }

    let meta = reader.meta();
    for scene in 0..meta.scene_count() {
        counters.scenes += 1;
        let dims = meta.scene(scene).expect("the scene exists").dims;

        let mut coords = vec![(0usize, 0usize, 0usize)];
        if dims.c > 1 {
            coords.push((0, dims.c - 1, 0));
        }
        if dims.z > 1 {
            coords.push((0, 0, dims.z - 1));
        }

        for (t, c, z) in coords {
            let plane = reader
                .plane(scene, t, c, z)
                .unwrap_or_else(|e| panic!("decode plane {scene},{t},{c},{z}: {e}"));
            counters.planes += 1;
            for rect in probe_rects(dims) {
                let region = reader
                    .region(scene, t, c, z, rect)
                    .unwrap_or_else(|e| panic!("read region {scene},{t},{c},{z}: {e}"));
                let cropped = plane
                    .crop(rect)
                    .unwrap_or_else(|e| panic!("crop the plane: {e}"));
                assert_eq!(
                    region.shape, cropped.shape,
                    "the region's shape differs from the cropped plane's"
                );
                assert_eq!(
                    region.samples, cropped.samples,
                    "the region's samples differ from the cropped plane's"
                );
                assert!(
                    region.data == cropped.data,
                    "the region's bytes differ from the cropped plane's"
                );
                counters.rectangles += 1;
            }
        }
    }
}

#[test]
fn every_real_file_region_is_the_cropped_plane() {
    let Some(dir) = real_data_dir() else {
        return;
    };
    let mut files = Vec::new();
    collect_files(&dir, &mut files);

    let mut counters = Counters::default();
    for path in &files {
        let layout = classify(path);
        *counters.layouts.entry(layout).or_insert(0) += 1;
        match path.extension().and_then(|e| e.to_str()) {
            Some("lif") => {
                let reader = LifReader::open(path)
                    .unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
                compare_reader(&reader, &mut counters);
            }
            Some("nd2") => {
                let reader = Nd2Reader::open(path)
                    .unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
                compare_reader(&reader, &mut counters);
            }
            Some("tif") | Some("tiff") => {
                let reader = TiffReader::open(path)
                    .unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
                compare_reader(&reader, &mut counters);
            }
            Some("lsm") => {
                let reader = LsmReader::open(path)
                    .unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
                compare_reader(&reader, &mut counters);
            }
            _ => {}
        }
    }

    assert!(
        counters.files > 0,
        "no LIF, ND2, TIFF or LSM file was found under PHASIT_REAL_DATA_DIR"
    );
    eprintln!(
        "region parity: {} files ({} region-capable, {} default), {} scenes, {} planes, \
         {} rectangles compared",
        counters.files,
        counters.region_capable,
        counters.default,
        counters.scenes,
        counters.planes,
        counters.rectangles,
    );
    for (layout, files) in &counters.layouts {
        eprintln!("  {layout}: {files} file(s)");
    }
}
