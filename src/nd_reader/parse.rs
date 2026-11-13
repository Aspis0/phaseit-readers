// SPDX-License-Identifier: Apache-2.0
//! The `.nd` text: one `"Key", value` pair per line, read into the dataset's axes.
//! A missing key means the simpler thing: a missing `Do*` key means that axis holds one plane, a missing count
//! means one, and a missing `WaveDoZ<i>` means the wavelength has a z-series.

use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::guard::{MAX_ND_PLANES, MAX_SCENE_DIM};

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
    let entries = entries(text)?;
    markers(&entries)?;
    let map = values(&entries);
    let axes = Axes::of(&map)?;
    let waves = waves(&map, &axes)?;
    let stages = stages(&map, &axes)?;
    guard_planes(axes.stages, axes.waves, axes.times, axes.z_steps)?;

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
            waves: count(map, "NWavelengths", do_wave)?.unwrap_or(1),
            stages: count(map, "NStagePositions", do_stage)?.unwrap_or(1),
            times: count(map, "NTimePoints", do_time)?.unwrap_or(1),
            z_steps: count(map, "NZSteps", do_z)?.unwrap_or(1),
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

/// The entries as a map of the values; a key stated twice keeps its last value.
fn values(entries: &[(String, Option<Value>)]) -> HashMap<String, Value> {
    let mut map = HashMap::new();
    for (key, value) in entries {
        if let Some(value) = value {
            map.insert(key.clone(), value.clone());
        }
    }
    map
}

/// A flag: absent means FALSE, and anything but TRUE or FALSE is a malformed line.
fn flag(map: &HashMap<String, Value>, key: &str) -> Result<bool> {
    match map.get(key) {
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
fn count(map: &HashMap<String, Value>, key: &str, stated: bool) -> Result<Option<usize>> {
    if !stated {
        return Ok(None);
    }
    match map.get(key) {
        None => Ok(None),
        Some(Value::Number(number)) => {
            if *number < 1.0 || number.fract() != 0.0 || *number > MAX_SCENE_DIM as f64 {
                return Err(Error::Unsupported(format!(
                    "MetaMorph .nd {key} is {number}: a count must be a whole number from 1 to {MAX_SCENE_DIM}"
                )));
            }
            Ok(Some(*number as usize))
        }
        Some(_) => Err(Error::Parse(format!("MetaMorph .nd {key} is not a number"))),
    }
}

/// A number as the file states it; absent means none.
fn number(map: &HashMap<String, Value>, key: &str) -> Option<f64> {
    match map.get(key) {
        Some(Value::Number(number)) => Some(*number),
        _ => None,
    }
}

/// A text value; absent means none.
fn name_of(map: &HashMap<String, Value>, key: &str) -> Option<String> {
    match map.get(key) {
        Some(Value::Text(text)) => Some(text.clone()),
        _ => None,
    }
}

/// Whether one wavelength holds a z-series: the file states it per wavelength, and an absent key means it does.
fn wave_do_z(map: &HashMap<String, Value>, index: usize) -> Result<bool> {
    match map.get(&format!("WaveDoZ{index}")) {
        None => Ok(true),
        Some(Value::Bool(flag)) => Ok(*flag),
        Some(Value::Text(text)) if text.eq_ignore_ascii_case("TRUE") => Ok(true),
        Some(Value::Text(text)) if text.eq_ignore_ascii_case("FALSE") => Ok(false),
        Some(_) => Err(Error::Parse(format!(
            "MetaMorph .nd WaveDoZ{index} is neither TRUE nor FALSE"
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The dataset a text states, with the axes the text gives.
    fn dataset(text: &str) -> Dataset {
        parse(text, "base").expect("a dataset the text states")
    }

    #[test]
    fn the_axes_come_from_the_do_keys() {
        let text = "\"NDInfoFile\", Version 1.0\n\"DoWave\", TRUE\n\"NWavelengths\", 3\n\
                    \"DoStage\", TRUE\n\"NStagePositions\", 2\n\"DoTimelapse\", TRUE\n\
                    \"NTimePoints\", 4\n\"DoZSeries\", TRUE\n\"NZSteps\", 5\n\"EndFile\"\n";
        let parsed = dataset(text);
        assert_eq!(parsed.waves.len(), 3);
        assert_eq!(parsed.stages.len(), 2);
        assert_eq!(parsed.times, 4);
        assert_eq!(parsed.z_steps, 5);
        assert!(parsed.do_wave && parsed.do_stage && parsed.do_time);
    }

    #[test]
    fn a_missing_do_key_means_one_plane_on_that_axis() {
        let parsed = dataset("\"NDInfoFile\", Version 1.0\n\"NWavelengths\", 7\n\"EndFile\"\n");
        assert_eq!(
            parsed.waves.len(),
            1,
            "the count of an axis that is not stated is ignored"
        );
        assert_eq!(parsed.times, 1);
        assert_eq!(parsed.z_steps, 1);
        assert!(!parsed.do_wave);
    }

    #[test]
    fn a_quoted_value_is_text_and_a_bare_one_is_read_by_its_kind() {
        let parsed = dataset(
            "\"NDInfoFile\", Version 1.0\n\"WaveName1\", \"DAPI\"\n\"WaveDoZ1\", FALSE\n\
             \"ZStepSize\", 0.5\n\"EndFile\"\n",
        );
        assert_eq!(parsed.waves[0].name, "DAPI");
        assert!(!parsed.waves[0].has_z, "the wavelength states no z-series");
        assert_eq!(parsed.z_step_um, Some(0.5));
    }

    #[test]
    fn a_missing_wave_do_z_key_means_the_wavelength_has_a_z_series() {
        let parsed = dataset("\"NDInfoFile\", Version 1.0\n\"DoZSeries\", TRUE\n\"EndFile\"\n");
        assert!(parsed.waves[0].has_z);
    }

    #[test]
    fn a_malformed_line_is_refused() {
        for text in [
            "NDInfoFile, Version 1.0\n\"EndFile\"\n",
            "\"NDInfoFile\n\"EndFile\"\n",
            "\"NDInfoFile\" Version 1.0\n\"EndFile\"\n",
            "\"NDInfoFile\", \"unterminated\n\"EndFile\"\n",
        ] {
            assert!(parse(text, "base").is_err(), "{text:?} is refused");
        }
    }

    #[test]
    fn a_file_that_does_not_start_with_ndinfofile_or_end_with_endfile_is_refused() {
        let no_start = parse("\"EndFile\"\n", "base").expect_err("no NDInfoFile key");
        assert!(no_start.to_string().contains("NDInfoFile"), "{no_start}");
        let no_end = parse("\"NDInfoFile\", Version 1.0\n", "base").expect_err("no EndFile key");
        assert!(no_end.to_string().contains("EndFile"), "{no_end}");
    }

    #[test]
    fn a_count_of_zero_is_refused() {
        let error = parse(
            "\"NDInfoFile\", Version 1.0\n\"DoWave\", TRUE\n\"NWavelengths\", 0\n\"EndFile\"\n",
            "base",
        )
        .expect_err("no wavelength at all");
        assert!(error.to_string().contains("NWavelengths"), "{error}");
    }

    #[test]
    fn a_dataset_above_the_plane_cap_is_refused() {
        // Every count is within its own bound; the product of the three axes is not.
        let text = "\"NDInfoFile\", Version 1.0\n\"DoWave\", TRUE\n\"NWavelengths\", 1000\n\
                    \"DoStage\", TRUE\n\"NStagePositions\", 1000\n\"DoTimelapse\", TRUE\n\
                    \"NTimePoints\", 1000\n\"EndFile\"\n";
        let error = parse(text, "base").expect_err("too many planes");
        assert!(error.to_string().contains("cap"), "{error}");
    }

    #[test]
    fn a_count_above_its_own_bound_is_refused() {
        let text = format!(
            "\"NDInfoFile\", Version 1.0\n\"DoStage\", TRUE\n\"NStagePositions\", {}\n\"EndFile\"\n",
            MAX_SCENE_DIM + 1
        );
        let error = parse(&text, "base").expect_err("too many stage positions");
        assert!(error.to_string().contains("NStagePositions"), "{error}");
    }
}
