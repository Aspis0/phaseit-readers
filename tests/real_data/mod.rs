// SPDX-License-Identifier: Apache-2.0
//! Where the real acquisition files live.
//!
//! A published crate must not carry anyone's local paths, so a test that reads
//! a real file cannot name it. The file is read from one environment variable,
//! `PHASIT_REAL_DATA_DIR`, which holds the folder the files live in; the file
//! itself is named by a relative path inside that folder (the layout is in the
//! crate README). A test whose variable is unset, or whose file is absent,
//! skips with a message saying which of the two it was — the same skip the
//! tests had when the file was simply missing.

use std::path::PathBuf;

/// The environment variable that names the folder holding the real files.
pub const REAL_DATA_DIR_VAR: &str = "PHASIT_REAL_DATA_DIR";

/// The real file at `relative` inside the real-data folder.
///
/// `None` means the test must skip: the variable is unset, or the file is not
/// there. The reason is printed either way.
pub fn real_file(relative: &str) -> Option<PathBuf> {
    let Some(dir) = std::env::var_os(REAL_DATA_DIR_VAR) else {
        eprintln!("skipping: {REAL_DATA_DIR_VAR} is not set");
        return None;
    };
    let path = PathBuf::from(dir).join(relative);
    if !path.exists() {
        eprintln!("skipping: {relative} is not present under {REAL_DATA_DIR_VAR}");
        return None;
    }
    Some(path)
}
