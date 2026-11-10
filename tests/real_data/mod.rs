// SPDX-License-Identifier: Apache-2.0
//! The real-data folder is named by `PHASIT_REAL_DATA_DIR`, so no local path is published. A test whose file is absent
//! skips, and the reason is printed.
//! `PHASIT_REAL_DATA_REQUIRED=1` (or `true`) turns every such skip into a failure. Unset, empty or `0` means the run
//! does not
//! need the data; any other value panics, so a typo cannot fail open.
//! The `*_from` functions take raw environment values and never read the environment, so tests can check them directly.

#![allow(dead_code)]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

const REAL_DATA_DIR_VAR: &str = "PHASIT_REAL_DATA_DIR";

const REAL_DATA_REQUIRED_VAR: &str = "PHASIT_REAL_DATA_REQUIRED";

const BGR_SAMPLE_VAR: &str = "PHASIT_CZI_BGR_SAMPLE";

#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Found(PathBuf),
    Skip(String),
    Required(String),
}

/// Unset, empty or `0` means not required; `1` or `true` (any case, trimmed) means required; anything else panics.
pub fn parse_required(value: Option<&OsStr>) -> bool {
    let Some(value) = value else {
        return false;
    };
    let value = value.to_string_lossy();
    match value.trim() {
        "" | "0" => false,
        "1" => true,
        other if other.eq_ignore_ascii_case("true") => true,
        other => panic!("{REAL_DATA_REQUIRED_VAR} must be 0 or 1, got {other}"),
    }
}

/// An empty value counts as unset: `Path::new("")` would resolve files against the working directory.
pub fn parse_real_data_dir(value: Option<&OsStr>) -> Option<PathBuf> {
    let value = value?;
    if value.is_empty() {
        return None;
    }
    Some(PathBuf::from(value))
}

/// Pure: the caller does the filesystem lookup, so the decision is testable without an environment.
pub fn decide_real_file(
    dir: Option<&Path>,
    required: bool,
    relative: &str,
    exists: bool,
) -> Decision {
    let Some(dir) = dir else {
        return absent(&format!("{REAL_DATA_DIR_VAR} is not set"), required);
    };
    if !exists {
        return absent(
            &format!("{relative} is not present under {REAL_DATA_DIR_VAR}"),
            required,
        );
    }
    Decision::Found(dir.join(relative))
}

/// The reason names the file, never its folders: these reasons are pasted into shared logs.
pub fn decide_named_file(
    required: bool,
    label: &str,
    path: Option<&Path>,
    exists: bool,
) -> Decision {
    match path {
        None => absent(&format!("{label} is not set"), required),
        Some(path) if !exists => absent(
            &format!("{label} points at {}, which is not present", file_name(path)),
            required,
        ),
        Some(path) => Decision::Found(path.to_path_buf()),
    }
}

pub fn absent(reason: &str, required: bool) -> Decision {
    if required {
        Decision::Required(format!("required real data missing: {reason}"))
    } else {
        Decision::Skip(reason.to_string())
    }
}

/// `None` means the test must skip; the reason is printed either way.
pub fn real_file(relative: &str) -> Option<PathBuf> {
    resolve(real_file_from(
        std::env::var_os(REAL_DATA_DIR_VAR).as_deref(),
        std::env::var_os(REAL_DATA_REQUIRED_VAR).as_deref(),
        relative,
    ))
}

pub fn reference_dump(path: &Path) -> Option<PathBuf> {
    resolve(reference_dump_from(
        std::env::var_os(REAL_DATA_REQUIRED_VAR).as_deref(),
        path,
    ))
}

/// Skips even in a run that requires the owner's data: the sample is third-party and not committed.
pub fn bgr_sample() -> Option<PathBuf> {
    resolve(bgr_sample_from(std::env::var_os(BGR_SAMPLE_VAR).as_deref()))
}

pub fn real_file_from(
    dir: Option<&OsStr>,
    required: Option<&OsStr>,
    relative: &str,
) -> Decision {
    let required = parse_required(required);
    let dir = parse_real_data_dir(dir);
    let exists = dir.as_ref().is_some_and(|d| d.join(relative).exists());
    decide_real_file(dir.as_deref(), required, relative, exists)
}

pub fn reference_dump_from(required: Option<&OsStr>, path: &Path) -> Decision {
    decide_named_file(
        parse_required(required),
        "the reference dump",
        Some(path),
        path.exists(),
    )
}

/// Found or skip, never required: a run that requires the owner's data must not fail over a file the owner never had.
pub fn bgr_sample_from(sample: Option<&OsStr>) -> Decision {
    let path = sample.map(PathBuf::from);
    let exists = path.as_ref().is_some_and(|p| p.exists());
    match path {
        None => Decision::Skip(format!("{BGR_SAMPLE_VAR} is not set")),
        Some(path) if !exists => Decision::Skip(format!(
            "{BGR_SAMPLE_VAR} points at {}, which is not present",
            file_name(&path)
        )),
        Some(path) => Decision::Found(path),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("<unnamed file>")
        .to_string()
}

fn resolve(decision: Decision) -> Option<PathBuf> {
    match decision {
        Decision::Found(path) => Some(path),
        Decision::Skip(reason) => {
            eprintln!("skipping: {reason}");
            None
        }
        Decision::Required(reason) => panic!("{reason}"),
    }
}
