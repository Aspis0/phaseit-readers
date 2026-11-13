// SPDX-License-Identifier: Apache-2.0
//! The block files of an N5 dataset under one root: `g0/g1/…` is one directory per axis but the last, which names
//! the file. A block the dataset does not hold is N5's zero block; one it holds is read by the block module.

use std::path::{Path, PathBuf};

pub(super) struct Blocks {
    root: PathBuf,
}

impl Blocks {
    pub fn new(root: &Path) -> Self {
        Blocks {
            root: root.to_path_buf(),
        }
    }

    /// The block at `grid`, or `None` when the dataset holds no file there (N5's zero block).
    pub fn file(&self, grid: &[usize]) -> Option<PathBuf> {
        let path = self.root.join(block_path(grid));
        path.is_file().then_some(path)
    }
}

/// `g0/g1/…`: one directory per axis but the last, which names the file.
fn block_path(grid: &[usize]) -> PathBuf {
    let (last, directories) = grid
        .split_last()
        .expect("a grid position of at least one axis");
    let mut path = PathBuf::new();
    for axis in directories {
        path.push(axis.to_string());
    }
    path.push(last.to_string());
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grid_position_is_one_directory_per_axis_but_the_last() {
        assert_eq!(block_path(&[0]), PathBuf::from("0"));
        assert_eq!(block_path(&[1, 2]), PathBuf::from("1").join("2"));
        assert_eq!(
            block_path(&[3, 2, 1]),
            PathBuf::from("3").join("2").join("1")
        );
        assert_eq!(
            block_path(&[10, 0, 4]),
            PathBuf::from("10").join("0").join("4")
        );
    }

    #[test]
    fn a_block_the_dataset_holds_is_found_and_a_missing_one_is_not() {
        let dir = std::env::temp_dir().join(format!("phasit_n5_store_{}", std::process::id()));
        let folder = dir.join("1").join("0");
        std::fs::create_dir_all(&folder).expect("create the block folder");
        std::fs::write(folder.join("2"), [1u8, 2, 3, 4]).expect("write the block");
        let blocks = Blocks::new(&dir);
        assert_eq!(
            blocks.file(&[1, 0, 2]).as_deref(),
            Some(folder.join("2").as_path())
        );
        assert!(
            blocks.file(&[0, 0, 0]).is_none(),
            "a missing block is not an error here"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
