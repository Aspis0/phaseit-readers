// SPDX-License-Identifier: Apache-2.0
//! The image files of a dataset: the readers kept open for the next plane, and the geometry every file shares.

use std::fs::File;
use std::path::{Path, PathBuf};

use tiff::encoder::colortype::Gray8;
use tiff::encoder::TiffEncoder;

use super::*;

/// A folder with the image files written, removed when it is dropped.
struct Folder(PathBuf);

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn folder(name: &str) -> Folder {
    let path =
        std::env::temp_dir().join(format!("phasit_nd_sources_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create the folder");
    Folder(path)
}

/// A two-by-two grey TIFF: the smallest file a reader opens.
fn write_tiff(path: &Path) {
    let mut encoder =
        TiffEncoder::new(File::create(path).expect("create the TIFF")).expect("the TIFF encoder");
    encoder
        .write_image::<Gray8>(2, 2, &[1u8, 2, 3, 4])
        .expect("write the image");
}

/// The geometry of the grey TIFF the test writes.
fn geometry() -> Geometry {
    Geometry {
        dtype: DType::U8,
        x: 2,
        y: 2,
        samples: 1,
    }
}

#[test]
fn the_cache_keeps_a_bound_number_of_files_open() {
    let folder = folder("cache");
    let sources = Sources::new(geometry());
    for index in 0..MAX_CACHED_SOURCES + 3 {
        let path = folder.0.join(format!("file{index}.TIF"));
        write_tiff(&path);
        sources.reader(&path).expect("the file is opened");
    }
    let open = sources
        .open
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert!(
        open.len() <= MAX_CACHED_SOURCES,
        "{} files are open, the bound is {MAX_CACHED_SOURCES}",
        open.len()
    );
}

#[test]
fn a_file_of_another_size_is_refused_by_its_name() {
    let folder = folder("size");
    let path = folder.0.join("file.TIF");
    write_tiff(&path);
    let sources = Sources::new(Geometry {
        dtype: DType::U8,
        x: 5,
        y: 5,
        samples: 1,
    });
    let error = match sources.reader(&path) {
        Err(error) => error,
        Ok(_) => panic!("a file of another size is refused"),
    };
    assert!(error.to_string().contains("file.TIF"), "{error}");
}
