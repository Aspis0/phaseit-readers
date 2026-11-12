// SPDX-License-Identifier: Apache-2.0
//! The ICS (Image Cytometry Standard) header: the text before the pixels. Its first two bytes are the field and the
//! line separator; every following line is fields split by the field separator — a category, a subcategory, then
//! values. Only the fields this reader serves are kept: `filename`, `history` and unknown categories are dropped.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// A header without `end` inside this cap is refused; the cap keeps a corrupt file from being read whole.
pub(super) const MAX_HEADER_BYTES: u64 = 1024 * 1024;

/// Bytes asked for per read; a header far below the cap costs one.
const READ_CHUNK: usize = 32 * 1024;

/// Version 1.0 keeps the pixels in a separate `.ids` file, 2.0 keeps them after the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Storage {
    Ids,
    Inline,
}

#[derive(Debug, PartialEq)]
pub(super) struct IcsHeader {
    pub version: String,
    pub storage: Storage,
    /// Byte offset of the pixel data: right after `end` and its line separator, inline storage only.
    pub data_offset: usize,
    pub compression: String,
    /// Bits per sample, the first entry of `layout sizes`.
    pub bits: usize,
    pub format: String,
    pub sign: String,
    /// `representation byte_order`, the sample's byte numbers in file order; `None` when the line is absent.
    pub byte_order: Option<String>,
    /// `layout order` after its `bits` word: axis names, fastest first.
    pub axes: Vec<String>,
    /// `layout sizes` after its bits word, aligned with `axes`.
    pub sizes: Vec<usize>,
    /// `parameter scale` after its bits word, aligned with `axes`.
    pub scale: Vec<f32>,
    /// `parameter units` after its bits word, aligned with `axes`.
    pub units: Vec<String>,
}

/// Reads the header and resolves the file the pixels live in: the path itself for 2.0, the `.ids` sibling for 1.0.
pub(super) fn load(path: &Path) -> Result<(IcsHeader, PathBuf)> {
    let header = parse(path)?;
    match header.storage {
        Storage::Inline => Ok((header, path.to_path_buf())),
        Storage::Ids => {
            let data = sibling(path, &["ids", "IDS"])?;
            Ok((header, data))
        }
    }
}

/// The header a path names: an `.ids` names the `.ics` beside it, and a missing sibling is a refusal naming it.
pub(super) fn header_path(path: &Path) -> Result<PathBuf> {
    if !has_extension(path, "ids") {
        return Ok(path.to_path_buf());
    }
    sibling(path, &["ics", "ICS"])
}

fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(extension))
}

/// The first existing sibling of `path` with the same stem and one of `extensions`.
fn sibling(path: &Path, extensions: &[&str]) -> Result<PathBuf> {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| Error::Unsupported("the ICS path has no file stem".to_string()))?;
    for extension in extensions {
        let candidate = path.with_file_name(format!("{stem}.{extension}"));
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    let wanted = path.with_file_name(format!("{stem}.{}", extensions[0]));
    Err(Error::Unsupported(format!(
        "ICS file {} is missing: the header and its pixels are two files",
        wanted.display()
    )))
}

/// The header text, read in chunks up to [`MAX_HEADER_BYTES`]: the pixels after it are not read here.
fn parse(path: &Path) -> Result<IcsHeader> {
    let mut file = File::open(path)?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; READ_CHUNK];
    loop {
        let read = file.read(&mut chunk)?;
        if read == 0 {
            return Err(Error::Unsupported(
                "not an ICS file: the header has no `end`".to_string(),
            ));
        }
        buf.extend_from_slice(&chunk[..read]);
        if let Some(end) = end_of(&buf) {
            return parse_fields(&buf, end);
        }
        if buf.len() as u64 >= MAX_HEADER_BYTES {
            return Err(Error::Unsupported(format!(
                "the ICS header has no `end` in its first {MAX_HEADER_BYTES} bytes"
            )));
        }
    }
}

/// Byte offset just past the line whose only field is `end`.
/// The scan stops at the field separator, so a header that lost its first two bytes never matches.
pub(super) fn end_of(buf: &[u8]) -> Option<usize> {
    let field = *buf.first()?;
    let line = *buf.get(1)?;
    let body = buf.get(2..)?;
    let mut at = 0;
    while at < body.len() {
        let stop = body[at..]
            .iter()
            .position(|b| *b == line)
            .map(|past| at + past)
            .unwrap_or(body.len());
        let mut fields = body[at..stop].split(|b| *b == field);
        let only = fields.next().unwrap_or(&[]);
        if (only == b"end" || only == b"end\r") && fields.next().is_none() {
            // The line separator itself belongs to the header, so the pixels start after it.
            return Some(usize::min(stop + 1, body.len()) + 2);
        }
        at = stop + 1;
    }
    None
}

pub(super) fn parse_fields(buf: &[u8], end: usize) -> Result<IcsHeader> {
    let field = *buf
        .first()
        .ok_or_else(|| Error::Unsupported("not an ICS file: it has no separators".to_string()))?;
    let line = *buf.get(1).ok_or_else(|| {
        Error::Unsupported("not an ICS file: it has no line separator".to_string())
    })?;
    let mut header = IcsHeader {
        version: String::new(),
        storage: Storage::Ids,
        data_offset: end,
        compression: "uncompressed".to_string(),
        bits: 0,
        format: "integer".to_string(),
        sign: "unsigned".to_string(),
        byte_order: None,
        axes: Vec::new(),
        sizes: Vec::new(),
        scale: Vec::new(),
        units: Vec::new(),
    };
    let mut declared = None;
    let mut saw_end = false;
    for raw in buf.get(2..end).unwrap_or_default().split(|b| *b == line) {
        let fields: Vec<String> = raw
            .split(|b| *b == field)
            .filter(|f| !f.is_empty())
            .map(|f| String::from_utf8_lossy(strip_cr(f)).into_owned())
            .collect();
        let Some(category) = fields.first().map(String::as_str) else {
            continue;
        };
        let rest = &fields[1..];
        let (subcategory, values) = match rest.split_first() {
            Some((first, tail)) => (first.as_str(), tail),
            None => ("", rest),
        };
        match category {
            "ics_version" => header.version = value_of(category, rest)?.to_string(),
            "layout" => layout(&mut header, subcategory, values, &mut declared)?,
            "representation" => representation(&mut header, subcategory, values)?,
            "parameter" => parameter(&mut header, subcategory, values)?,
            "end" => {
                saw_end = true;
                break;
            }
            // `filename` names a file this reader never opens, `history` is provenance, the rest is unknown.
            _ => {}
        }
    }
    if !saw_end {
        return Err(Error::Unsupported(
            "not an ICS file: the header has no `end`".to_string(),
        ));
    }
    finish(header, declared)
}

fn layout(
    header: &mut IcsHeader,
    subcategory: &str,
    values: &[String],
    declared: &mut Option<usize>,
) -> Result<()> {
    match subcategory {
        // The order's first word is `bits`; the axis names follow it.
        "order" => header.axes = axis_names(&values[1..])?,
        "sizes" => {
            header.bits = count("layout sizes bits", values.first())?;
            header.sizes = values[1..]
                .iter()
                .map(|v| count("layout sizes", Some(v)))
                .collect::<Result<Vec<usize>>>()?;
        }
        "parameters" => *declared = Some(count("layout parameters", values.first())?),
        // `significant_bits` is informative; an unknown subcategory is ignored.
        _ => {}
    }
    Ok(())
}

fn representation(header: &mut IcsHeader, subcategory: &str, values: &[String]) -> Result<()> {
    match subcategory {
        "format" => header.format = value_of(subcategory, values)?.to_string(),
        "sign" => header.sign = value_of(subcategory, values)?.to_string(),
        "compression" => header.compression = value_of(subcategory, values)?.to_string(),
        "byte_order" => header.byte_order = Some(values.join(" ")),
        _ => {}
    }
    Ok(())
}

fn parameter(header: &mut IcsHeader, subcategory: &str, values: &[String]) -> Result<()> {
    // Both lines carry a bits word first, then one entry per axis of the order.
    let entries = &values[1..];
    match subcategory {
        "scale" => {
            header.scale = entries
                .iter()
                .map(|v| {
                    v.parse::<f32>()
                        .map_err(|_| Error::Parse(format!("ICS scale value {v:?} is not a number")))
                })
                .collect::<Result<Vec<f32>>>()?
        }
        "units" => header.units = entries.to_vec(),
        _ => {}
    }
    Ok(())
}

/// The version, the axis geometry and the consistency of `layout parameters` with it: all before the first pixel.
fn finish(mut header: IcsHeader, declared: Option<usize>) -> Result<IcsHeader> {
    header.version = version_of(&header.version)?.to_string();
    if let Some(n) = declared {
        if n != header.axes.len() + 1 || n != header.sizes.len() + 1 {
            return Err(Error::Parse(format!(
                "ICS layout parameters is {n}, but order holds {} axis name(s) and sizes {} size(s)",
                header.axes.len(),
                header.sizes.len()
            )));
        }
    }
    if header.axes.is_empty() || header.sizes.is_empty() {
        return Err(Error::Unsupported(
            "ICS layout states no axes: order and sizes are both needed".to_string(),
        ));
    }
    if header.axes.len() != header.sizes.len() {
        return Err(Error::Parse(format!(
            "ICS layout order holds {} axis name(s) but sizes {} size(s)",
            header.axes.len(),
            header.sizes.len()
        )));
    }
    header.storage = match header.version.as_str() {
        "2.0" => Storage::Inline,
        _ => Storage::Ids,
    };
    Ok(header)
}

fn version_of(text: &str) -> Result<&str> {
    match text {
        "1.0" | "2.0" => Ok(text),
        "" => Err(Error::Unsupported(
            "not an ICS file: the header states no `ics_version`".to_string(),
        )),
        other => Err(Error::Unsupported(format!(
            "ICS version {other} is not one this reader reads (1.0 and 2.0)"
        ))),
    }
}

/// One axis name; `c` is accepted for `ch`, an unknown or repeated name is refused by name, and so is a missing x or y.
fn axis_names(values: &[String]) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::with_capacity(values.len());
    for value in values {
        let name = if value == "c" { "ch" } else { value.as_str() };
        if !matches!(name, "x" | "y" | "z" | "t" | "ch") {
            return Err(Error::Unsupported(format!(
                "ICS layout order names axis {value:?}, which is not x, y, z, t or ch"
            )));
        }
        if out.iter().any(|seen| seen == name) {
            return Err(Error::Unsupported(format!(
                "ICS layout order names axis {value:?} more than once"
            )));
        }
        out.push(name.to_string());
    }
    for required in ["x", "y"] {
        if !out.iter().any(|name| name == required) {
            return Err(Error::Unsupported(format!(
                "ICS layout order does not name axis {required:?}, so the scene has no image"
            )));
        }
    }
    Ok(out)
}

/// A count of entries: text that is not a number is a broken header, a negative value cannot be a count.
fn count(what: &str, text: Option<&String>) -> Result<usize> {
    let text = text
        .ok_or_else(|| Error::Parse(format!("ICS {what} states no value")))?
        .as_str();
    let value: i64 = text
        .parse()
        .map_err(|_| Error::Parse(format!("ICS {what} is {text:?}, not a count")))?;
    usize::try_from(value).map_err(|_| {
        Error::Unsupported(format!(
            "ICS {what} is {value}: an extent or count cannot be negative"
        ))
    })
}

/// The single value of a line that carries no subcategory.
fn value_of<'a>(what: &str, values: &'a [String]) -> Result<&'a str> {
    values
        .first()
        .map(String::as_str)
        .ok_or_else(|| Error::Parse(format!("ICS {what} states no value")))
}

fn strip_cr(field: &[u8]) -> &[u8] {
    field.strip_suffix(b"\r").unwrap_or(field)
}
