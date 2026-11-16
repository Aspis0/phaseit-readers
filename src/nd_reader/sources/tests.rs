// SPDX-License-Identifier: Apache-2.0
//! The image files of a dataset: the readers kept open for the next plane, and the geometry every file shares.

use std::cell::Cell;
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

/// The error an open gives a process out of file handles.
fn out_of_handles_error() -> Error {
    Error::Io(std::io::Error::from_raw_os_error(OUT_OF_HANDLES[0]))
}

/// A handle shortage on the first open gives back the least recently used file's handle, and the file opens again.
#[test]
fn a_handle_shortage_evicts_the_least_recently_used_file_and_retries_once() {
    let folder = folder("handles");
    let [a, b] = ["a.TIF", "b.TIF"].map(|name| {
        let path = folder.0.join(name);
        write_tiff(&path);
        path
    });
    let sources = Sources::new(geometry(), MIN_OPEN_MEMBERS);
    sources.reader(&a).expect("a is opened");
    let attempts = Cell::new(0);
    let opener = |path: &Path| {
        attempts.set(attempts.get() + 1);
        if attempts.get() == 1 {
            Err(out_of_handles_error())
        } else {
            open_reader(path)
        }
    };
    sources
        .reader_with(&b, opener)
        .expect("b is opened once a gives up its handle");
    assert_eq!(attempts.get(), 2, "one retry");
    let open = sources
        .open
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let kept: Vec<&Path> = open.iter().map(|(path, _)| path.as_path()).collect();
    assert_eq!(kept, [b.as_path()], "a was the least recently used file");
}

/// A shortage that persists through the retry is reported as a shortage, not as a file that cannot be read.
#[test]
fn a_shortage_that_persists_says_the_process_ran_out_of_handles() {
    let folder = folder("exhausted");
    let [a, b] = ["a.TIF", "b.TIF"].map(|name| {
        let path = folder.0.join(name);
        write_tiff(&path);
        path
    });
    let sources = Sources::new(geometry(), MIN_OPEN_MEMBERS);
    sources.reader(&a).expect("a is opened");
    let attempts = Cell::new(0);
    let error = match sources.reader_with(&b, |_| {
        attempts.set(attempts.get() + 1);
        Err(out_of_handles_error())
    }) {
        Err(error) => error,
        Ok(_) => panic!("a file that cannot be opened is refused"),
    };
    assert_eq!(attempts.get(), 2, "one retry, no more");
    let text = error.to_string();
    assert!(text.contains("ran out of file handles"), "{text}");
    assert!(!text.contains("could not be read"), "{text}");
}

/// Only a handle shortage is retried: a file that will not open is named as unreadable at once.
#[test]
fn a_file_that_will_not_open_is_not_retried() {
    let folder = folder("unreadable");
    let [a, missing] = ["a.TIF", "missing.TIF"].map(|name| folder.0.join(name));
    write_tiff(&a);
    let sources = Sources::new(geometry(), MIN_OPEN_MEMBERS);
    sources.reader(&a).expect("a is opened");
    let attempts = Cell::new(0);
    let error = match sources.reader_with(&missing, |candidate: &Path| {
        attempts.set(attempts.get() + 1);
        open_reader(candidate)
    }) {
        Err(error) => error,
        Ok(_) => panic!("a file that does not exist is refused"),
    };
    assert_eq!(attempts.get(), 1, "no retry for a file that will not open");
    let text = error.to_string();
    assert!(
        text.contains("missing.TIF") && text.contains("could not be read"),
        "{text}"
    );
}
