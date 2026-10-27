// SPDX-License-Identifier: Apache-2.0
//! The decision a real-file test makes when the data is not there.
//!
//! The rules live in one pure function so they can be checked without an
//! environment: without the real data a test skips, and with
//! `PHASIT_REAL_DATA_REQUIRED=1` it fails loudly instead. These tests call
//! the decision directly, so they never read or set an environment variable
//! and can run in parallel with everything else.
//!
//! The one exception is the THIRD-PARTY sample `PHASIT_CZI_BGR_SAMPLE`
//! names: it is not the owner's data and is not committed, so it skips
//! whatever the required flag says.

mod real_data;

use std::any::Any;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use real_data::{
    absent, bgr_sample_from, decide_named_file, decide_real_file, parse_real_data_dir,
    parse_required, real_file_from, reference_dump_from, Decision,
};

/// The folder the real files would live in.
const DIR: &str = "real-data";

/// A file inside that folder.
const SAMPLE: &str = "czi/mosaic.czi";

/// A reference dump outside that folder.
const DUMP: &str = "manifest.json";

/// The variable that points at the third-party sample.
const NAMED_VAR: &str = "PHASIT_CZI_BGR_SAMPLE";

/// A folder that is certainly there: the crate's own directory.
const CRATE_DIR: &str = env!("CARGO_MANIFEST_DIR");

/// A file in that folder that is certainly there.
const PRESENT: &str = "Cargo.toml";

/// A name in that folder that is certainly not there.
const MISSING: &str = "no-such-real-file.czi";

/// The message of a panic, as its payload carries it.
fn panic_message(payload: Box<dyn Any + Send>) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|text| text.to_string()))
        .unwrap_or_default()
}

/// A dump that is not there: a name in a folder that does not exist.
fn missing_dump() -> PathBuf {
    Path::new("no-such-folder").join(DUMP)
}

/// A file that is there: the crate's own manifest.
fn present_file() -> PathBuf {
    PathBuf::from(CRATE_DIR).join(PRESENT)
}

/// A third-party sample that is not there.
fn missing_sample() -> PathBuf {
    Path::new("samples").join("third-party").join("c_winnt.czi")
}

#[test]
fn an_unset_flag_does_not_require_the_real_data() {
    assert!(!parse_required(None));
}

#[test]
fn an_empty_flag_does_not_require_the_real_data() {
    assert!(!parse_required(Some(OsStr::new(""))));
}

#[test]
fn a_zero_flag_does_not_require_the_real_data() {
    assert!(!parse_required(Some(OsStr::new("0"))));
}

#[test]
fn a_one_flag_requires_the_real_data() {
    assert!(parse_required(Some(OsStr::new("1"))));
}

#[test]
fn a_true_flag_requires_the_real_data() {
    assert!(parse_required(Some(OsStr::new("true"))));
}

#[test]
fn a_true_flag_ignores_case_and_surrounding_space() {
    assert!(parse_required(Some(OsStr::new(" TRUE "))));
}

#[test]
fn a_typo_in_the_flag_panics_instead_of_failing_open() {
    for value in ["yes", "01", "on", "2", "truthy"] {
        let panic = std::panic::catch_unwind(|| parse_required(Some(OsStr::new(value))));
        let message = match panic {
            Ok(flag) => panic!("{value:?} was accepted as {flag}"),
            Err(payload) => panic_message(payload),
        };
        assert!(
            message.contains("PHASIT_REAL_DATA_REQUIRED must be 0 or 1"),
            "{value:?} gave the message {message:?}"
        );
        assert!(
            message.contains(value),
            "{value:?} is not named in the message {message:?}"
        );
    }
}

#[test]
fn an_empty_folder_variable_is_treated_as_unset() {
    assert_eq!(parse_real_data_dir(None), None);
    assert_eq!(parse_real_data_dir(Some(OsStr::new(""))), None);
    assert_eq!(
        parse_real_data_dir(Some(OsStr::new(DIR))),
        Some(PathBuf::from(DIR))
    );
}

#[test]
fn the_parsed_flag_turns_a_missing_file_into_a_failure_or_a_skip() {
    let reason = "czi/mosaic.czi is not present under PHASIT_REAL_DATA_DIR";

    let required = parse_required(Some(OsStr::new("1")));
    assert_eq!(
        absent(reason, required),
        Decision::Required(format!("required real data missing: {reason}"))
    );

    let required = parse_required(Some(OsStr::new("0")));
    assert_eq!(absent(reason, required), Decision::Skip(reason.to_string()));
}

#[test]
fn the_reason_for_a_named_file_names_the_file_and_not_its_folders() {
    // The path is built from its parts instead of written as a Windows path:
    // on Linux and macOS a backslash is not a separator, so a hard-coded
    // `C:\...` has no file name to strip.
    let dump = Path::new("build")
        .join("machine")
        .join("tests")
        .join("parity")
        .join("_ref")
        .join("manifest.json");
    let decision = decide_named_file(false, "the reference dump", Some(&dump), false);
    assert_eq!(
        decision,
        Decision::Skip(
            "the reference dump points at manifest.json, which is not present".to_string()
        )
    );
}

#[test]
fn the_reason_for_the_third_party_sample_names_the_file_and_not_its_folders() {
    let sample = Path::new("samples").join("third-party").join("c_winnt.czi");
    let decision = decide_named_file(false, NAMED_VAR, Some(&sample), false);
    assert_eq!(
        decision,
        Decision::Skip(format!(
            "{NAMED_VAR} points at c_winnt.czi, which is not present"
        ))
    );
}

#[test]
fn an_unset_folder_and_no_requirement_skips() {
    let decision = decide_real_file(None, false, SAMPLE, false);
    assert_eq!(
        decision,
        Decision::Skip("PHASIT_REAL_DATA_DIR is not set".to_string())
    );
}

#[test]
fn an_unset_folder_and_a_requirement_fails() {
    let decision = decide_real_file(None, true, SAMPLE, false);
    assert_eq!(
        decision,
        Decision::Required(
            "required real data missing: PHASIT_REAL_DATA_DIR is not set".to_string()
        )
    );
}

#[test]
fn an_absent_file_and_no_requirement_skips() {
    let decision = decide_real_file(Some(Path::new(DIR)), false, SAMPLE, false);
    assert_eq!(
        decision,
        Decision::Skip(format!("{SAMPLE} is not present under PHASIT_REAL_DATA_DIR"))
    );
}

#[test]
fn an_absent_file_and_a_requirement_fails() {
    let decision = decide_real_file(Some(Path::new(DIR)), true, SAMPLE, false);
    assert_eq!(
        decision,
        Decision::Required(format!(
            "required real data missing: {SAMPLE} is not present under PHASIT_REAL_DATA_DIR"
        ))
    );
}

#[test]
fn a_present_file_gives_its_path() {
    let decision = decide_real_file(Some(Path::new(DIR)), false, SAMPLE, true);
    assert_eq!(decision, Decision::Found(PathBuf::from(DIR).join(SAMPLE)));
}

#[test]
fn an_absent_dump_and_no_requirement_skips() {
    let decision = decide_named_file(false, "the reference dump", Some(Path::new(DUMP)), false);
    assert_eq!(
        decision,
        Decision::Skip(format!(
            "the reference dump points at {DUMP}, which is not present"
        ))
    );
}

#[test]
fn an_absent_dump_and_a_requirement_fails() {
    let decision = decide_named_file(true, "the reference dump", Some(Path::new(DUMP)), false);
    assert_eq!(
        decision,
        Decision::Required(format!(
            "required real data missing: the reference dump points at {DUMP}, which is not present"
        ))
    );
}

#[test]
fn a_present_dump_gives_its_path() {
    let decision = decide_named_file(false, "the reference dump", Some(Path::new(DUMP)), true);
    assert_eq!(decision, Decision::Found(PathBuf::from(DUMP)));
}

#[test]
fn an_unset_named_file_and_no_requirement_skips() {
    let decision = decide_named_file(false, NAMED_VAR, None, false);
    assert_eq!(decision, Decision::Skip(format!("{NAMED_VAR} is not set")));
}

#[test]
fn an_unset_named_file_and_a_requirement_fails() {
    let decision = decide_named_file(true, NAMED_VAR, None, false);
    assert_eq!(
        decision,
        Decision::Required(format!("required real data missing: {NAMED_VAR} is not set"))
    );
}

#[test]
fn real_file_from_fails_when_the_run_requires_a_missing_file() {
    let decision = real_file_from(
        Some(OsStr::new(CRATE_DIR)),
        Some(OsStr::new("1")),
        MISSING,
    );
    assert_eq!(
        decision,
        Decision::Required(format!(
            "required real data missing: {MISSING} is not present under PHASIT_REAL_DATA_DIR"
        ))
    );
}

#[test]
fn real_file_from_skips_a_missing_file_when_the_run_does_not_require_it() {
    let decision = real_file_from(
        Some(OsStr::new(CRATE_DIR)),
        Some(OsStr::new("0")),
        MISSING,
    );
    assert_eq!(
        decision,
        Decision::Skip(format!(
            "{MISSING} is not present under PHASIT_REAL_DATA_DIR"
        ))
    );
}

#[test]
fn real_file_from_gives_the_path_of_a_present_file() {
    let decision = real_file_from(
        Some(OsStr::new(CRATE_DIR)),
        Some(OsStr::new("1")),
        PRESENT,
    );
    assert_eq!(
        decision,
        Decision::Found(PathBuf::from(CRATE_DIR).join(PRESENT))
    );
}

#[test]
fn reference_dump_from_fails_when_the_run_requires_a_missing_dump() {
    let decision = reference_dump_from(Some(OsStr::new("1")), &missing_dump());
    assert_eq!(
        decision,
        Decision::Required(format!(
            "required real data missing: the reference dump points at {DUMP}, which is not present"
        ))
    );
}

#[test]
fn reference_dump_from_skips_a_missing_dump_when_the_run_does_not_require_it() {
    let decision = reference_dump_from(Some(OsStr::new("0")), &missing_dump());
    assert_eq!(
        decision,
        Decision::Skip(format!(
            "the reference dump points at {DUMP}, which is not present"
        ))
    );
}

#[test]
fn reference_dump_from_gives_the_path_of_a_present_dump() {
    let decision = reference_dump_from(Some(OsStr::new("1")), &present_file());
    assert_eq!(decision, Decision::Found(present_file()));
}

/// The third-party sample is not the owner's data, so requiring the owner's
/// data must not turn its absence into a failure: it skips.
#[test]
fn bgr_sample_from_skips_a_missing_sample_even_when_the_run_requires_the_real_data() {
    let decision = bgr_sample_from(Some(missing_sample().as_os_str()));
    assert_eq!(
        decision,
        Decision::Skip(format!(
            "{NAMED_VAR} points at c_winnt.czi, which is not present"
        ))
    );
}

#[test]
fn bgr_sample_from_skips_an_unset_sample_even_when_the_run_requires_the_real_data() {
    let decision = bgr_sample_from(None);
    assert_eq!(decision, Decision::Skip(format!("{NAMED_VAR} is not set")));
}

#[test]
fn bgr_sample_from_gives_the_path_of_a_present_sample() {
    let present = present_file();
    let decision = bgr_sample_from(Some(present.as_os_str()));
    assert_eq!(decision, Decision::Found(present));
}
