// SPDX-License-Identifier: Apache-2.0
//! The dataset directory's listing: an image file is looked up by its whole name, case-insensitively, because a
//! Windows acquisition writes its names in a spelling the file system kept and the dataset states another.
//! The names are folded once, so a lookup is one hash hit and never a scan of the whole directory.

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// The files of the dataset's own directory, keyed by their name folded to lower case.
pub(super) struct Listing {
    files: HashMap<String, PathBuf>,
}

impl Listing {
    /// The files of `directory`; a subdirectory is not an image file.
    pub(super) fn read(directory: &Path, cap: usize) -> Result<Listing> {
        let mut files = HashMap::new();
        for entry in std::fs::read_dir(directory)? {
            let path = entry?.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if files.len() >= cap {
                return Err(Error::Unsupported(format!(
                    "MetaMorph .nd: the dataset directory holds more than {cap} files, so its listing is not read"
                )));
            }
            insert(&mut files, name, &path)?;
        }
        Ok(Listing { files })
    }

    /// The path of the file named `name`, whatever the case it is spelled in.
    pub(super) fn find(&self, name: &str) -> Option<&Path> {
        self.files
            .get(&name.to_ascii_lowercase())
            .map(PathBuf::as_path)
    }
}

/// One file of the listing: its name is folded into the map, and two names that fold to one key are refused,
/// because the dataset names one of them and the other would shadow it.
fn insert(files: &mut HashMap<String, PathBuf>, name: &str, path: &Path) -> Result<()> {
    if let Some(shadow) = files.insert(name.to_ascii_lowercase(), path.to_path_buf()) {
        return Err(Error::Unsupported(format!(
            "MetaMorph .nd: the dataset directory holds two files whose names differ in case alone: {:?} and \
             {name:?}",
            shadow.file_name().unwrap_or_default()
        )));
    }
    Ok(())
}
