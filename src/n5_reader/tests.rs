// SPDX-License-Identifier: Apache-2.0
//! Unit tests of the N5 dataset path: a dataset is opened by its directory or by its `attributes.json`, and the
//! reader is named after the directory either way.

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

use super::{dataset, directory, N5Reader};
use crate::ImageReader;

/// A scratch dataset removed by `Drop`, even when an assertion fails.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let path =
            std::env::temp_dir().join(format!("phasit_n5_unit_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the scratch folder");
        Scratch(path)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn attributes(&self) -> PathBuf {
        let path = self.join("attributes.json");
        let mut file = File::create(&path).expect("create the attributes");
        write!(
            file,
            r#"{{"dimensions":[4,3],"blockSize":[2,2],"dataType":"uint8"}}"#
        )
        .expect("write the attributes");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl std::ops::Deref for Scratch {
    type Target = std::path::Path;

    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

#[test]
fn a_directory_and_its_attributes_file_name_the_same_dataset() {
    let dir = Scratch::new("dataset");
    let file = dir.attributes();
    assert_eq!(
        dataset(&file).expect("the file is a dataset"),
        (dir.to_path_buf(), file.clone())
    );
    assert_eq!(dataset(&dir).expect("the directory is a dataset").1, file);
}

#[test]
fn a_path_that_is_not_an_attributes_file_names_that_file_as_the_dataset() {
    // The detector routes only `attributes.json` here; the reader still names what it was given.
    let dir = Scratch::new("other");
    for name in ["image.tif", "Attributes.json", "attributes.json.bak"] {
        let path = dir.join(name);
        std::fs::write(&path, b"{}").expect("write the file");
        let (root, attributes) = dataset(&path).expect("the path is taken as the attributes");
        assert_eq!((root, attributes), (dir.to_path_buf(), path), "{name}");
    }
}

#[test]
fn a_dataset_opened_either_way_is_named_after_its_directory() {
    let dir = Scratch::new("named");
    let file = dir.attributes();
    let by_file = N5Reader::open(&file).expect("open the attributes file");
    let by_directory = N5Reader::open(&dir).expect("open the directory");
    // The scratch folder is named after the case, so the dataset's name is that folder's own.
    let named = directory(&dir);
    assert!(named.starts_with("phasit_n5_unit_named"), "{named}");
    assert_eq!(by_file.meta().name, named);
    assert_eq!(by_directory.meta().name, named);
    assert_eq!(by_file.meta().scene(0).expect("the scene").name, named);
}
