// SPDX-License-Identifier: Apache-2.0
//! The dataset directory's listing: an image file is looked up by its whole name, case-insensitively, because a
//! Windows acquisition writes its names in a spelling the file system kept and the dataset states another.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::Result;

/// The files of the dataset's own directory, keyed by their name as it is on disk.
pub(super) struct Listing {
    files: HashMap<String, PathBuf>,
}

impl Listing {
    /// The files of `directory`; a subdirectory is not an image file.
    pub(super) fn read(directory: &Path) -> Result<Listing> {
        let mut files = HashMap::new();
        for entry in std::fs::read_dir(directory)? {
            let path = entry?.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            files.insert(name.to_string(), path);
        }
        Ok(Listing { files })
    }

    /// The path of the file named `name`: the name as it is, or the file whose name differs from it in case alone.
    pub(super) fn find(&self, name: &str) -> Option<&Path> {
        if let Some(path) = self.files.get(name) {
            return Some(path.as_path());
        }
        self.files
            .iter()
            .find(|(at, _)| at.eq_ignore_ascii_case(name))
            .map(|(_, path)| path.as_path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder with the files named, removed when it is dropped.
    struct Folder(PathBuf);

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn folder(name: &str) -> Folder {
        let path =
            std::env::temp_dir().join(format!("phasit_nd_listing_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the folder");
        Folder(path)
    }

    #[test]
    fn a_file_is_found_whatever_the_case_of_its_name() {
        let folder = folder("case");
        std::fs::write(folder.0.join("Acq_w1DAPI.TIF"), b"x").expect("write the file");
        let listing = Listing::read(&folder.0).expect("read the folder");
        assert!(listing.find("acq_w1dapi.tif").is_some());
        assert!(listing.find("ACQ_W1DAPI.TIF").is_some());
        assert!(listing.find("acq_w2.TIF").is_none());
    }
}
