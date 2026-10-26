// SPDX-License-Identifier: Apache-2.0
//! Where the real acquisition files live, and what a test does when they are not there.
//!
//! A published crate must not carry anyone's local paths, so a test that reads
//! a real file cannot name it. The file is read from one environment variable,
//! `PHASIT_REAL_DATA_DIR`, which holds the folder the files live in; the file
//! itself is named by a relative path inside that folder (the layout is in the
//! crate README). A test whose variable is unset, or whose file is absent,
//! skips with a message saying which of the two it was — the same skip the
//! tests had when the file was simply missing.
//!
//! A skip is only honest when the run did NOT ask for the real data. Set
//! `PHASIT_REAL_DATA_REQUIRED=1` (or `true`) and every one of those skips
//! becomes a failure carrying the same reason, prefixed
//! `required real data missing:` — so a run that claims to need the real data
//! cannot pass quietly without it. The flag is read by one rule, and only two
//! values mean anything: `0` (or unset, or empty) means the run does not need
//! the data, `1` (or `true`) means it does. Anything else is a typo, and a
//! typo must not fail open: it panics.
//!
//! The decision itself is pure: it takes the folder, the flag and whether the
//! file is there as arguments and never reads the environment. The `*_from`
//! functions take the raw environment values and make that decision, so a
//! test can check them without touching the process environment; the wrappers
//! below read the environment and call them.

#![allow(dead_code)]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The environment variable that names the folder holding the real files.
const REAL_DATA_DIR_VAR: &str = "PHASIT_REAL_DATA_DIR";

/// The environment variable that turns every skip into a failure.
const REAL_DATA_REQUIRED_VAR: &str = "PHASIT_REAL_DATA_REQUIRED";

/// The environment variable that names the third-party Bgr24 CZI sample.
const BGR_SAMPLE_VAR: &str = "PHASIT_CZI_BGR_SAMPLE";

/// What a test must do about the real-data artifact it needs.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// The artifact is there: this is its path.
    Found(PathBuf),
    /// The artifact is not there and the run does not require it: skip, with
    /// this reason.
    Skip(String),
    /// The artifact is not there and the run requires it: fail, with this reason.
    Required(String),
}

/// Whether the run demands the real data, from the value of the flag.
///
/// One rule, everywhere: unset, empty or `0` means the run does not need the
/// data; `1` or `true` (ASCII case-insensitive, surrounding whitespace
/// trimmed) means it does. Any other value is a typo, and a typo must not
/// fail open — it panics. Pure: it reads no environment and touches no file.
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

/// The folder holding the real files, from the value of the folder variable.
///
/// An empty value counts as unset: `Path::new("")` resolves a file against the
/// working directory, which is never what the variable meant. Pure.
pub fn parse_real_data_dir(value: Option<&OsStr>) -> Option<PathBuf> {
    let value = value?;
    if value.is_empty() {
        return None;
    }
    Some(PathBuf::from(value))
}

/// The decision for a file inside the folder named by `PHASIT_REAL_DATA_DIR`.
///
/// `dir` is the value of that variable (`None` when it is unset), `required`
/// says whether the run demands the file, `relative` names the file inside the
/// folder and `exists` says whether it is there. Pure: the caller does the
/// filesystem lookup, so the decision is testable without an environment.
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

/// The decision for a file that is named in full: a reference dump, a
/// manifest, or the third-party sample `PHASIT_CZI_BGR_SAMPLE` points at.
///
/// `required` says whether the run demands it, `label` names the variable that
/// points at it, `path` is the file that variable points at (`None` when the
/// variable is unset) and `exists` says whether that file is there. Pure, like
/// `decide_real_file`. The reason names the file, never the folders that
/// lead to it: these reasons are pasted into shared logs.
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

/// The decision between a skip and a loud failure.
pub fn absent(reason: &str, required: bool) -> Decision {
    if required {
        Decision::Required(format!("required real data missing: {reason}"))
    } else {
        Decision::Skip(reason.to_string())
    }
}

/// The real file at `relative` inside the real-data folder.
///
/// `None` means the test must skip: the variable is unset, or the file is not
/// there. The reason is printed either way.
pub fn real_file(relative: &str) -> Option<PathBuf> {
    resolve(real_file_from(
        std::env::var_os(REAL_DATA_DIR_VAR).as_deref(),
        std::env::var_os(REAL_DATA_REQUIRED_VAR).as_deref(),
        relative,
    ))
}

/// The reference dump at `path` (a manifest, a `.npy`), when it is there.
///
/// `None` means the test must skip: the dump is not there.
pub fn reference_dump(path: &Path) -> Option<PathBuf> {
    resolve(reference_dump_from(
        std::env::var_os(REAL_DATA_REQUIRED_VAR).as_deref(),
        path,
    ))
}

/// The third-party Bgr24 CZI sample `PHASIT_CZI_BGR_SAMPLE` names, when it is there.
///
/// `None` means the test must skip: the variable is unset, or the file is not there.
pub fn bgr_sample() -> Option<PathBuf> {
    resolve(bgr_sample_from(
        std::env::var_os(BGR_SAMPLE_VAR).as_deref(),
        std::env::var_os(REAL_DATA_REQUIRED_VAR).as_deref(),
    ))
}

/// The decision for the real file at `relative` inside the real-data folder.
///
/// `dir` and `required` are the raw values of `PHASIT_REAL_DATA_DIR` and
/// `PHASIT_REAL_DATA_REQUIRED`; the file's presence is looked up here. No
/// environment is read, so a test can call this without touching the process
/// environment.
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

/// The decision for the reference dump at `path`.
///
/// `required` is the raw value of `PHASIT_REAL_DATA_REQUIRED`; the dump's
/// presence is looked up here. No environment is read.
pub fn reference_dump_from(required: Option<&OsStr>, path: &Path) -> Decision {
    decide_named_file(
        parse_required(required),
        "the reference dump",
        Some(path),
        path.exists(),
    )
}

/// The decision for the third-party sample `PHASIT_CZI_BGR_SAMPLE` names.
///
/// `sample` is the raw value of that variable and `required` the raw value of
/// `PHASIT_REAL_DATA_REQUIRED`; the sample's presence is looked up here. No
/// environment is read.
pub fn bgr_sample_from(sample: Option<&OsStr>, required: Option<&OsStr>) -> Decision {
    let path = sample.map(PathBuf::from);
    let exists = path.as_ref().is_some_and(|p| p.exists());
    decide_named_file(
        parse_required(required),
        BGR_SAMPLE_VAR,
        path.as_deref(),
        exists,
    )
}

/// The name of a file without the folders that lead to it.
///
/// The reasons a skip or a failure carries travel into shared logs, so they
/// must name the file and not the machine's directory layout.
fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("<unnamed file>")
        .to_string()
}

/// The outcome of a decision: the path, a printed skip, or a loud failure.
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
