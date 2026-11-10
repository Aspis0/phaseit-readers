// SPDX-License-Identifier: Apache-2.0
//! One JSON line per listed file. A panicking reader becomes an error line; a hang is the sweep script's
//! problem, since it runs one child process per file.

use std::io::{BufRead, BufWriter, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use super::json;
use super::report;

/// `regions` compares each sweep window with the same crop of the whole plane; a timing run turns that off.
pub fn run(list_path: &Path, regions: bool) -> std::io::Result<()> {
    let list = std::fs::File::open(list_path)?;
    let stdout = std::io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    for line in std::io::BufReader::new(list).lines() {
        let line = line?;
        let path = line.trim();
        if path.is_empty() {
            continue;
        }
        writeln!(out, "{}", guarded_report(Path::new(path), regions))?;
    }
    out.flush()
}

/// A reader panic is caught here, so it cannot take the whole run down.
pub fn guarded_report(path: &Path, regions: bool) -> String {
    match catch_unwind(AssertUnwindSafe(|| report::file_report(path, regions))) {
        Ok(line) => line,
        Err(payload) => panic_report(path, payload.as_ref()),
    }
}

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
