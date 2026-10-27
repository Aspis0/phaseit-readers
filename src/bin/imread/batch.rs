// SPDX-License-Identifier: Apache-2.0
//! The batch mode: a list of files, one JSON line each.
//!
//! The list is a text file with one path per line; blank lines are skipped.
//! Every path is reported on its own line, so a reader that hangs on one file
//! is the sweep script's problem (it runs one child process per file), and a
//! reader that panics on one file is this module's problem: the panic is
//! caught and reported as an error line, so one file cannot end the run.

use std::io::{BufRead, BufWriter, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use super::json;
use super::report;

/// Report every path of the list file, one JSON line each.
pub fn run(list_path: &Path) -> std::io::Result<()> {
    let list = std::fs::File::open(list_path)?;
    let stdout = std::io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    for line in std::io::BufReader::new(list).lines() {
        let line = line?;
        let path = line.trim();
        if path.is_empty() {
            continue;
        }
        writeln!(out, "{}", guarded_report(Path::new(path)))?;
    }
    out.flush()
}

/// One file's report. A panic in the reader becomes an error line.
///
/// The guard is the binary's half of the per-file isolation: a reader that
/// panics on one file must not take the run down with it.
pub fn guarded_report(path: &Path) -> String {
    match catch_unwind(AssertUnwindSafe(|| report::file_report(path))) {
        Ok(line) => line,
        Err(payload) => panic_report(path, payload.as_ref()),
    }
}

/// The report of a file whose reader panicked.
fn panic_report(path: &Path, payload: &(dyn std::any::Any + Send)) -> String {
    let message = if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "the reader panicked".to_string()
    };
    format!(
        "{{\"path\":{},\"format\":null,\"status\":\"error\",\
         \"error\":{{\"kind\":\"panic\",\"message\":{}}},\"open_ms\":null,\
         \"scene_count\":0,\"scenes\":[],\"refused_scenes\":[],\"planes\":[]}}",
        json::string(&path.to_string_lossy()),
        json::string(&message),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panicking_reader_becomes_an_error_line() {
        let payload: Box<dyn std::any::Any + Send> = Box::new("boom");
        let line = panic_report(Path::new("a.tif"), payload.as_ref());
        assert!(line.contains("\"status\":\"error\""));
        assert!(line.contains("\"kind\":\"panic\""));
        assert!(line.contains("boom"));
    }
}
