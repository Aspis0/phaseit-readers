// SPDX-License-Identifier: Apache-2.0
//! The scratch files the reader tests write, and the guard that removes them.
//!
//! A test writes its synthetic container into the temp folder and drops the
//! guard at the end of the test: the file, or the folder of files, and the
//! `.lifext` companion a LIF test puts next to it, go away with the guard, so
//! a test run leaves nothing behind in the temp folder.

use std::path::{Path, PathBuf};

/// A scratch file of this test run, removed when the guard is dropped.
pub struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// A guard over the scratch file at `path`.
    pub fn new(path: PathBuf) -> Scratch {
        Scratch { path }
    }
}

impl std::ops::Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Every error is ignored on purpose, a file still held open included: a
        // panic in Drop during an unwinding test would abort the whole run. The
        // companion is only written by some tests, so its absence is the usual case.
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(self.path.with_extension("lifext"));
    }
}

/// A scratch folder of this test run, removed with everything in it when the guard is dropped.
pub struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    /// A guard over the scratch folder at `path`.
    pub fn new(path: PathBuf) -> ScratchDir {
        ScratchDir { path }
    }
}

impl std::ops::Deref for ScratchDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        // Every error is ignored on purpose, as for `Scratch`: a panic in Drop would abort the run.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A scratch LIF file of this test run.
pub fn lif_scratch(name: &str) -> Scratch {
    Scratch::new(
        std::env::temp_dir().join(format!("phasit_lif_refusal_{name}_{}", std::process::id())),
    )
}
