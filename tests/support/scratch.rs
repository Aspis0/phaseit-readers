// SPDX-License-Identifier: Apache-2.0
//! The scratch files the reader tests write, and the guards that remove them: a test run leaves nothing behind in the
//! temp folder.
//! The `.lifext` companion a LIF test writes goes with the guard too.

use std::path::{Path, PathBuf};

pub struct Scratch {
    path: PathBuf,
}

impl Scratch {
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
        // Drop ignores every error on purpose: a panic in Drop during an unwinding test would abort the run.
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(self.path.with_extension("lifext"));
    }
}

pub struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
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
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

pub fn lif_scratch(name: &str) -> Scratch {
    Scratch::new(
        std::env::temp_dir().join(format!("phasit_lif_refusal_{name}_{}", std::process::id())),
    )
}
