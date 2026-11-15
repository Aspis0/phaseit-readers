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
    const BOUND: usize = 4;
    let folder = folder("cache");
    let sources = Sources::new(geometry(), BOUND);
    for index in 0..BOUND + 3 {
        let path = folder.0.join(format!("file{index}.TIF"));
        write_tiff(&path);
        sources.reader(&path).expect("the file is opened");
    }
    let open = sources
        .open
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert_eq!(open.len(), BOUND, "the bound is {BOUND}");
}

/// A file read again is the most recently used: with a bound of two, reading `a` again before `c` evicts `b`.
#[test]
fn the_least_recently_used_file_is_evicted_first() {
    let folder = folder("lru");
    let [a, b, c] = ["a.TIF", "b.TIF", "c.TIF"].map(|name| {
        let path = folder.0.join(name);
        write_tiff(&path);
        path
    });
    let sources = Sources::new(geometry(), 2);
    for path in [&a, &b, &a, &c] {
        sources.reader(path).expect("the file is opened");
    }
    let open = sources
        .open
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let kept: Vec<&Path> = open.iter().map(|(path, _)| path.as_path()).collect();
    assert_eq!(kept, [a.as_path(), c.as_path()], "b was read longest ago");
}

#[test]
fn the_bound_follows_the_files_within_the_floor_and_the_ceiling() {
    assert_eq!(
        open_bound(3),
        MIN_OPEN_MEMBERS,
        "a few files keep the floor"
    );
    assert_eq!(
        open_bound(100),
        100,
        "one open file per file of the dataset"
    );
    assert_eq!(
        open_bound(10_000),
        MAX_ND_OPEN_MEMBERS,
        "the handle share caps the bound"
    );
}

#[test]
fn a_note_is_given_only_when_more_files_than_the_bound_exist() {
    assert_eq!(cache_note(MAX_ND_OPEN_MEMBERS, MAX_ND_OPEN_MEMBERS), None);
    let note = cache_note(MAX_ND_OPEN_MEMBERS + 1, MAX_ND_OPEN_MEMBERS).expect("a note");
    assert!(
        note.contains("257 image files, at most 256 stay open"),
        "{note}"
    );
}

#[test]
fn a_file_of_another_size_is_refused_by_its_name() {
    let folder = folder("size");
    let path = folder.0.join("file.TIF");
    write_tiff(&path);
    let sources = Sources::new(
        Geometry {
            dtype: DType::U8,
            x: 5,
            y: 5,
            samples: 1,
        },
        MIN_OPEN_MEMBERS,
    );
    let error = match sources.reader(&path) {
        Err(error) => error,
        Ok(_) => panic!("a file of another size is refused"),
    };
    assert!(error.to_string().contains("file.TIF"), "{error}");
}
