// SPDX-License-Identifier: Apache-2.0
//! The dataset directory's listing: a file is found by its name whatever the case it is spelled in, and two names
//! that differ in case alone are refused.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::*;

/// A folder with the files named, removed when it is dropped.
struct Folder(PathBuf);

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn folder(name: &str) -> Folder {
    let path = std::env::temp_dir().join(format!("phasit_nd_listing_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create the folder");
    Folder(path)
}

#[test]
fn a_file_is_found_whatever_the_case_of_its_name() {
    let folder = folder("case");
    std::fs::write(folder.0.join("Acq_w1DAPI.TIF"), b"x").expect("write the file");
    let listing = Listing::read(&folder.0, 16).expect("read the folder");
    assert!(listing.find("acq_w1dapi.tif").is_some());
    assert!(listing.find("ACQ_W1DAPI.TIF").is_some());
    assert!(listing.find("acq_w2.TIF").is_none());
}

#[test]
fn a_folder_of_more_files_than_its_cap_is_refused() {
    let folder = folder("cap");
    for index in 0..3 {
        std::fs::write(folder.0.join(format!("file{index}.TIF")), b"x").expect("write the file");
    }
    let error = match Listing::read(&folder.0, 2) {
        Err(error) => error,
        Ok(_) => panic!("a folder past its cap is refused"),
    };
    assert!(error.to_string().contains("more than 2 files"), "{error}");
}

#[test]
fn two_names_that_differ_in_case_alone_are_refused() {
    // The names are compared folded, so the refusal holds on a case-sensitive file system too.
    let mut files = HashMap::new();
    insert(&mut files, "Acq.TIF", Path::new("Acq.TIF")).expect("the first name");
    let error = match insert(&mut files, "acq.tif", Path::new("acq.tif")) {
        Err(error) => error,
        Ok(()) => panic!("two names of one file are refused"),
    };
    assert!(error.to_string().contains("case alone"), "{error}");
}

#[test]
fn two_files_whose_names_differ_in_case_alone_are_refused() {
    let folder = folder("shadow");
    std::fs::write(folder.0.join("Acq.TIF"), b"x").expect("write the file");
    std::fs::write(folder.0.join("acq.tif"), b"x").expect("write the file");
    // A case-insensitive file system folds the second name into the first, so there is nothing to refuse there.
    if std::fs::read_dir(&folder.0).expect("read the folder").count() < 2 {
        return;
    }
    let error = match Listing::read(&folder.0, 16) {
        Err(error) => error,
        Ok(_) => panic!("two files of one name are refused"),
    };
    assert!(error.to_string().contains("case alone"), "{error}");
}
