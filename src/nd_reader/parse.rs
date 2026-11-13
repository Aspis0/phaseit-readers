// SPDX-License-Identifier: Apache-2.0
//! The `.nd` text: one `"Key", value` pair per line, read into the dataset's axes.
//! A missing key means the simpler thing: a missing `Do*` key means that axis holds one plane, a missing count
//! means one, and a missing `WaveDoZ<i>` means the wavelength has a z-series.

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::guard::{MAX_ND_AXIS_LEN, MAX_ND_PLANES, MAX_ND_WAVELENGTHS};

/// One wavelength of the dataset: its name, and whether it holds a z-series.
#[derive(Debug, Clone)]
pub(super) struct Wave {
    pub name: String,
    /// The wavelength's images are `.STK` files holding a z-series; without one they are one-plane `.TIF` files.
    pub has_z: bool,
}

/// What the `.nd` file states about the acquisition.
#[derive(Debug, Clone)]
pub(super) struct Dataset {
    /// The `.nd` file's own name without its extension: the image files are named after it.
    pub base: String,
    pub do_wave: bool,
    pub do_stage: bool,
    pub do_time: bool,
    /// Whether a wavelength's name is part of its image file name.
    pub wave_in_file_name: bool,
    pub waves: Vec<Wave>,
    /// The stage positions' names, one per scene; empty when the dataset states none.
    pub stages: Vec<String>,
    pub times: usize,
    /// The z steps of the dataset; a wavelength without a z-series holds one plane whatever this says.
    pub z_steps: usize,
    /// The z step in µm as the file states it; the shared calibration rule decides whether it is a distance.
    pub z_step_um: Option<f32>,
}

/// The value of one line: a quoted string, a number, or a flag.
#[derive(Debug, Clone)]
enum Value {
    Bool(bool),
    Number(f64),
    Text(String),
}

/// The dataset the text states.
pub(super) fn parse(text: &str, base: &str) -> Result<Dataset> {
    // A byte order mark may precede the first key: the detection tolerates it, so the parser does too.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let entries = entries(text)?;
    markers(&entries)?;
    let map = values(&entries);
    let axes = Axes::of(&map)?;
    // The planes are counted before the per-axis vectors are built: a file that declares a million wavelengths must
    // not cost a million allocations before it is refused.
    guard_planes(axes.stages, axes.waves, axes.times, axes.z_steps)?;
    let waves = waves(&map, &axes)?;
    let stages = stages(&map, &axes)?;

    Ok(Dataset {
        base: base.to_string(),
        do_wave: axes.do_wave,
        do_stage: axes.do_stage,
        do_time: axes.do_time,
        wave_in_file_name: flag(&map, "WaveInFileName")?,
        waves,
        stages,
        times: axes.times,
        z_steps: axes.z_steps,
        z_step_um: number(&map, "ZStepSize").map(|value| value as f32),
    })
}

/// The file opens with its `"NDInfoFile"` key and ends with its `"EndFile"` marker.
fn markers(entries: &[(String, Option<Value>)]) -> Result<()> {
    let first = entries
        .first()
        .ok_or_else(|| Error::Parse("MetaMorph .nd file is empty".to_string()))?;
    if !first.0.eq_ignore_ascii_case("NDInfoFile") {
        return Err(Error::Parse(format!(
            "MetaMorph .nd file opens with {:?}, not with \"NDInfoFile\"",
            first.0
        )));
    }
    let last = entries.last().expect("a non-empty list of entries");
    if !last.0.eq_ignore_ascii_case("EndFile") {
        return Err(Error::Parse(
            "MetaMorph .nd file ends without its \"EndFile\" marker".to_string(),
        ));
    }
    Ok(())
}

/// The axes the dataset states: which of them it has, and how long each one is.
struct Axes {
    do_wave: bool,
    do_stage: bool,
    do_time: bool,
    do_z: bool,
    waves: usize,
    stages: usize,
    times: usize,
    z_steps: usize,
}

impl Axes {
    /// The axes of the file: an axis the file does not state holds one plane, and its count is ignored.
    fn of(map: &HashMap<String, Value>) -> Result<Axes> {
        let do_wave = flag(map, "DoWave")?;
        let do_stage = flag(map, "DoStage")?;
        let do_time = flag(map, "DoTimelapse")?;
        let do_z = flag(map, "DoZSeries")?;
        Ok(Axes {
            do_wave,
            do_stage,
            do_time,
            do_z,
            waves: count(map, "NWavelengths", do_wave, MAX_ND_WAVELENGTHS)?.unwrap_or(1),
            stages: count(map, "NStagePositions", do_stage, MAX_ND_AXIS_LEN)?.unwrap_or(1),
            times: count(map, "NTimePoints", do_time, MAX_ND_AXIS_LEN)?.unwrap_or(1),
            z_steps: count(map, "NZSteps", do_z, MAX_ND_AXIS_LEN)?.unwrap_or(1),
        })
    }
}

/// The wavelengths of the dataset: their names, and whether each one holds a z-series.
fn waves(map: &HashMap<String, Value>, axes: &Axes) -> Result<Vec<Wave>> {
    let mut list = Vec::with_capacity(axes.waves);
    for index in 1..=axes.waves {
        list.push(Wave {
            name: name_of(map, &format!("WaveName{index}")).unwrap_or_default(),
            has_z: axes.do_z && wave_do_z(map, index)?,
        });
    }
    Ok(list)
}

/// The stage positions of the dataset: their names.
fn stages(map: &HashMap<String, Value>, axes: &Axes) -> Result<Vec<String>> {
    let mut list = Vec::with_capacity(axes.stages);
    for index in 1..=axes.stages {
        list.push(name_of(map, &format!("Stage{index}")).unwrap_or_default());
    }
    Ok(list)
}

/// Every line of the file as its key and its value.
fn entries(text: &str) -> Result<Vec<(String, Option<Value>)>> {
    let mut out = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        out.push(entry(line, index + 1)?);
    }
    Ok(out)
}

/// One `"Key", value` line: the key, and the value when the line carries one.
fn entry(line: &str, number: usize) -> Result<(String, Option<Value>)> {
    let trimmed = line.trim();
    let Some(rest) = trimmed.strip_prefix('"') else {
        return Err(Error::Parse(format!(
            "MetaMorph .nd line {number} does not start with a quoted key: {trimmed:?}"
        )));
    };
    let Some(end) = rest.find('"') else {
        return Err(Error::Parse(format!(
            "MetaMorph .nd line {number} has an unterminated key: {trimmed:?}"
        )));
    };
    let key = rest[..end].to_string();
    let after = rest[end + 1..].trim();
    if after.is_empty() {
        return Ok((key, None));
    }
    let Some(raw) = after.strip_prefix(',') else {
        return Err(Error::Parse(format!(
            "MetaMorph .nd line {number} does not separate its key and its value with a comma: {trimmed:?}"
        )));
    };
    let (value, rest) = value(raw.trim(), number)?;
    if !rest.trim().is_empty() {
        return Err(Error::Parse(format!(
            "MetaMorph .nd line {number} holds text after its value: {trimmed:?}"
        )));
    }
    Ok((key, Some(value)))
}

/// The value of a line and the text left after it.
fn value(raw: &str, number: usize) -> Result<(Value, &str)> {
    if let Some(inner) = raw.strip_prefix('"') {
        let Some(end) = inner.find('"') else {
            return Err(Error::Parse(format!(
                "MetaMorph .nd line {number} holds an unterminated value: {raw:?}"
            )));
        };
        return Ok((Value::Text(inner[..end].to_string()), &inner[end + 1..]));
    }
    if raw.eq_ignore_ascii_case("TRUE") {
        return Ok((Value::Bool(true), ""));
    }
    if raw.eq_ignore_ascii_case("FALSE") {
        return Ok((Value::Bool(false), ""));
    }
    match raw.parse::<f64>() {
        Ok(number) => Ok((Value::Number(number), "")),
        Err(_) => Ok((Value::Text(raw.to_string()), "")),
    }
}

/// The entries as a map of the values; a key stated twice keeps its last value. The keys are folded to lower case,
/// because a Windows acquisition spells a key as "DoTimelapse" or "DoTimeLapse" alike.
fn values(entries: &[(String, Option<Value>)]) -> HashMap<String, Value> {
    let mut map = HashMap::new();
    for (key, value) in entries {
        if let Some(value) = value {
            map.insert(key.trim().to_ascii_lowercase(), value.clone());
        }
    }
    map
}

/// The value of one key, whatever the case it is written in.
fn get<'a>(map: &'a HashMap<String, Value>, key: &str) -> Option<&'a Value> {
    map.get(&key.to_ascii_lowercase())
}

/// A flag: absent means FALSE, and anything but TRUE or FALSE is a malformed line.
fn flag(map: &HashMap<String, Value>, key: &str) -> Result<bool> {
    match get(map, key) {
        None => Ok(false),
        Some(Value::Bool(flag)) => Ok(*flag),
        Some(Value::Text(text)) if text.eq_ignore_ascii_case("TRUE") => Ok(true),
        Some(Value::Text(text)) if text.eq_ignore_ascii_case("FALSE") => Ok(false),
        Some(_) => Err(Error::Parse(format!(
            "MetaMorph .nd {key} is neither TRUE nor FALSE"
        ))),
    }
}

/// A count: absent means one, and a count below one is refused. The count of an axis the file does not state is
/// ignored, so that a stale count cannot make a file hold planes it has none of.
fn count(map: &HashMap<String, Value>, key: &str, stated: bool, limit: usize) -> Result<Option<usize>> {
    if !stated {
        return Ok(None);
    }
    match get(map, key) {
        None => Ok(None),
        Some(Value::Number(number)) => {
            if *number < 1.0 || number.fract() != 0.0 || *number > limit as f64 {
                return Err(Error::Unsupported(format!(
                    "MetaMorph .nd {key} is {number}: a count must be a whole number from 1 to {limit}"
                )));
            }
            Ok(Some(*number as usize))
        }
        Some(_) => Err(Error::Parse(format!("MetaMorph .nd {key} is not a number"))),
    }
}

/// A number as the file states it; absent means none.
fn number(map: &HashMap<String, Value>, key: &str) -> Option<f64> {
    match get(map, key) {
        Some(Value::Number(number)) => Some(*number),
        _ => None,
    }
}

/// A text value; absent means none.
fn name_of(map: &HashMap<String, Value>, key: &str) -> Option<String> {
    match get(map, key) {
        Some(Value::Text(text)) => Some(text.clone()),
        _ => None,
    }
}

/// Whether one wavelength holds a z-series: the file states it per wavelength, and an absent key means it does.
fn wave_do_z(map: &HashMap<String, Value>, index: usize) -> Result<bool> {
    let key = format!("WaveDoZ{index}");
    match get(map, &key) {
        None => Ok(true),
        Some(Value::Bool(flag)) => Ok(*flag),
        Some(Value::Text(text)) if text.eq_ignore_ascii_case("TRUE") => Ok(true),
        Some(Value::Text(text)) if text.eq_ignore_ascii_case("FALSE") => Ok(false),
        Some(_) => Err(Error::Parse(format!(
            "MetaMorph .nd {key} is neither TRUE nor FALSE"
        ))),
    }
}

/// The dataset's planes: stage positions x wavelengths x time points x z steps, refused above the cap.
fn guard_planes(stages: usize, waves: usize, times: usize, z_steps: usize) -> Result<()> {
    let planes = stages
        .checked_mul(waves)
        .and_then(|value| value.checked_mul(times))
        .and_then(|value| value.checked_mul(z_steps));
    match planes {
        Some(planes) if planes <= MAX_ND_PLANES => Ok(()),
        _ => Err(Error::Unsupported(format!(
            "MetaMorph .nd dataset declares {stages} stage position(s) x {waves} wavelength(s) x {times} time \
             point(s) x {z_steps} z step(s), above the cap of {MAX_ND_PLANES} planes"
        ))),
    }
}

