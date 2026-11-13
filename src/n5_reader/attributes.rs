// SPDX-License-Identifier: Apache-2.0
//! The attributes of one N5 dataset: the axis sizes (dimension 0 fastest), the block shape, the sample type, the
//! compression and an optional pixel calibration. Unknown keys are ignored; every value is checked before an
//! allocation is sized from it.

use std::path::Path;

use serde_json::{Map, Value};

use crate::calibration::Calibration;
use crate::error::{Error, Result};
use crate::guard::MAX_SCENE_DIM;
use crate::types::DType;

/// An `attributes.json` above this holds no dataset: a corrupt or hostile file is not read whole.
const MAX_ATTRIBUTES_BYTES: u64 = 1024 * 1024;

#[derive(Debug)]
pub(super) struct Attributes {
    /// Axis sizes, dimension 0 fastest.
    pub dimensions: Vec<usize>,
    /// Block shape, one side per axis, dimension 0 fastest.
    pub block_size: Vec<usize>,
    pub dtype: DType,
    pub compression: Compression,
    pub calibration: Calibration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Compression {
    None,
    Gzip,
    Zstd,
}

impl Compression {
    pub(super) fn name(self) -> &'static str {
        match self {
            Compression::None => "raw",
            Compression::Gzip => "gzip",
            Compression::Zstd => "zstd",
        }
    }
}

pub(super) fn read(path: &Path) -> Result<Attributes> {
    let len = path.metadata()?.len();
    if len > MAX_ATTRIBUTES_BYTES {
        return Err(Error::OversizedAllocation {
            requested: usize::try_from(len).unwrap_or(usize::MAX),
            cap: MAX_ATTRIBUTES_BYTES as usize,
        });
    }
    parse(&std::fs::read(path)?)
}

fn parse(bytes: &[u8]) -> Result<Attributes> {
    let json: Value = serde_json::from_slice(bytes)
        .map_err(|e| Error::Parse(format!("N5 attributes.json is not valid JSON: {e}")))?;
    let root = json.as_object().ok_or_else(|| {
        Error::Parse("N5 attributes.json does not hold a JSON object".to_string())
    })?;
    let dimensions = counts("dimensions", root)?;
    let block_size = counts("blockSize", root)?;
    if block_size.len() != dimensions.len() {
        return Err(Error::Parse(format!(
            "N5 blockSize states {} side(s) for a dataset of {} dimension(s)",
            block_size.len(),
            dimensions.len()
        )));
    }
    Ok(Attributes {
        dimensions,
        block_size,
        dtype: dtype_of(root)?,
        compression: compression_of(root)?,
        calibration: calibration_of(root),
    })
}

/// A JSON array of axis sizes, dimension 0 first: the fastest axis.
fn counts(key: &str, root: &Map<String, Value>) -> Result<Vec<usize>> {
    let array = root
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Parse(format!("N5 attributes state no {key}")))?;
    array
        .iter()
        .enumerate()
        .map(|(axis, value)| count(&format!("{key}[{axis}]"), value))
        .collect()
}

/// A JSON number is an f64: a size is exact only when whole, positive and within the scene bound.
fn count(what: &str, value: &Value) -> Result<usize> {
    let number = value
        .as_f64()
        .ok_or_else(|| Error::Parse(format!("N5 {what} is not a number")))?;
    if number < 1.0 || number.fract() != 0.0 || number > MAX_SCENE_DIM as f64 {
        return Err(Error::Unsupported(format!(
            "N5 {what} is {number}: an extent or block side must be a whole number from 1 to {MAX_SCENE_DIM}"
        )));
    }
    Ok(number as usize)
}

fn dtype_of(root: &Map<String, Value>) -> Result<DType> {
    match root.get("dataType").and_then(Value::as_str) {
        Some("uint8") => Ok(DType::U8),
        Some("uint16") => Ok(DType::U16),
        Some("int16") => Ok(DType::I16),
        Some("float32") => Ok(DType::F32),
        Some(other) => Err(Error::Unsupported(format!(
            "N5 dataType {other:?} is not a sample type this crate represents (uint8, uint16, int16 and float32)"
        ))),
        None => Err(Error::Parse(match root.get("dataType") {
            None => "N5 attributes state no dataType".to_string(),
            Some(other) => format!("N5 dataType {other} is not a sample type name (uint8, uint16, int16 and float32)"),
        })),
    }
}

/// The `compression` object of current writers, the `compressionType` string of the legacy ones, or a `compression`
/// stated as a plain string.
fn compression_of(root: &Map<String, Value>) -> Result<Compression> {
    let stated = match root.get("compression").and_then(Value::as_object) {
        Some(object) => object.get("type").and_then(Value::as_str),
        None => ["compressionType", "compression"]
            .iter()
            .find_map(|key| root.get(*key).and_then(Value::as_str)),
    };
    // A dataset that states no compression holds raw blocks: that is N5's own default.
    match stated {
        None => Ok(Compression::None),
        Some("raw") => Ok(Compression::None),
        Some("gzip") => Ok(Compression::Gzip),
        Some("zstd") => Ok(Compression::Zstd),
        Some(other) => Err(Error::Unsupported(format!(
            "N5 compression {other:?} is not one this reader decodes (raw, gzip and zstd)"
        ))),
    }
}

/// `pixelResolution` states one spacing and one unit for every axis. Each axis counts only when finite and positive;
/// an unknown unit or shape leaves the scene uncalibrated rather than guessed.
fn calibration_of(root: &Map<String, Value>) -> Calibration {
    let resolution = root.get("pixelResolution").and_then(Value::as_object);
    let spacing = |axis: usize| {
        let factor = unit_to_um(resolution?.get("unit")?.as_str()?)?;
        let stated = resolution?
            .get("dimensions")?
            .as_array()?
            .get(axis)?
            .as_f64()?;
        (stated.is_finite() && stated > 0.0).then_some(stated as f32 * factor)
    };
    match (spacing(0), spacing(1)) {
        (Some(x), Some(y)) => Calibration::sized(x, y, spacing(2)),
        _ => Calibration::uncalibrated(vec![
            "the file states no pixel size: pixelResolution is missing or unusable along x or y"
                .to_string(),
        ]),
    }
}

fn unit_to_um(unit: &str) -> Option<f32> {
    match unit {
        "um" | "\u{b5}m" | "micrometer" => Some(1.0),
        "nm" => Some(0.001),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_json(json: &str) -> Result<Attributes> {
        parse(json.as_bytes())
    }

    /// The dataset every case below varies: dimensions [4, 3], blockSize [2, 2], uint8.
    fn dataset(extra: &str) -> String {
        format!(r#"{{"dimensions":[4,3],"blockSize":[2,2],"dataType":"uint8"{extra}}}"#)
    }

    #[test]
    fn a_minimal_dataset_parses() {
        let attributes = parse_json(r#"{"dimensions":[7,5],"blockSize":[3,2],"dataType":"uint8"}"#)
            .expect("the attributes parse");
        assert_eq!(attributes.dimensions, vec![7, 5]);
        assert_eq!(attributes.block_size, vec![3, 2]);
        assert_eq!(attributes.dtype, DType::U8);
        assert_eq!(attributes.compression, Compression::None);
        assert!(attributes.calibration.px_size_um.is_none());
        let unknown = parse_json(&dataset(",\"fillValue\":0,\"version\":\"0.5.1\""));
        assert!(unknown.is_ok(), "unknown keys are ignored: {unknown:?}");
    }

    #[test]
    fn both_compression_forms_are_read_and_others_refused_by_name() {
        for (key, expected) in [
            (r#","compression":{"type":"gzip"}"#, Compression::Gzip),
            (r#","compression":{"type":"zstd"}"#, Compression::Zstd),
            (r#","compressionType":"gzip""#, Compression::Gzip),
            (r#","compressionType":"raw""#, Compression::None),
            (
                r#","compression":{"type":"raw","blockSize":4}"#,
                Compression::None,
            ),
            (r#","compression":"gzip""#, Compression::Gzip),
        ] {
            assert_eq!(
                parse_json(&dataset(key)).unwrap().compression,
                expected,
                "{key}"
            );
        }
        let error = parse_json(&dataset(r#","compression":{"type":"blosc"}"#)).expect_err("blosc");
        assert!(matches!(error, Error::Unsupported(message) if message.contains("blosc")));
    }

    #[test]
    fn calibration_converts_um_and_nm_and_omits_the_rest() {
        for (unit, dimensions, expected, name) in [
            ("um", "[0.5,0.25,2]", (0.5, 0.25, Some(2.0)), "um"),
            ("\u{b5}m", "[0.5,0.25]", (0.5, 0.25, None), "micro"),
            ("micrometer", "[1,1]", (1.0, 1.0, None), "word"),
            ("nm", "[250,100]", (0.25, 0.1, None), "nm"),
        ] {
            let attributes = parse_json(&dataset(&format!(
                r#","pixelResolution":{{"dimensions":{dimensions},"unit":"{unit}"}}"#
            )))
            .unwrap();
            let px = attributes
                .calibration
                .px_size_um
                .unwrap_or_else(|| panic!("{name}"));
            assert_eq!((px.x, px.y, px.z), expected, "{name}");
        }
        for (resolution, name) in [
            (
                r#""pixelResolution":{"dimensions":[1,1],"unit":"mm"}"#,
                "mm",
            ),
            (r#""pixelResolution":{"dimensions":[1,1]}"#, "no unit"),
            (
                r#""pixelResolution":{"dimensions":[1],"unit":"um"}"#,
                "one axis",
            ),
            (
                r#""pixelResolution":{"dimensions":[1,0],"unit":"um"}"#,
                "zero",
            ),
            (
                r#""pixelResolution":{"dimensions":[1,-1],"unit":"um"}"#,
                "negative",
            ),
            (
                r#""pixelResolution":{"dimensions":[1,"1"],"unit":"um"}"#,
                "text",
            ),
        ] {
            let attributes = parse_json(&dataset(&format!(",{resolution}"))).unwrap();
            assert!(attributes.calibration.px_size_um.is_none(), "{name}");
            assert_eq!(attributes.calibration.notes.len(), 1, "{name}");
        }
    }

    #[test]
    fn malformed_or_impossible_values_are_refused() {
        let cases = [
            ("{", "not valid JSON"),
            ("[]", "JSON object"),
            (r#"{"blockSize":[2],"dataType":"uint8"}"#, "no dimensions"),
            (r#"{"dimensions":[4],"dataType":"uint8"}"#, "no blockSize"),
            (
                r#"{"dimensions":[4],"blockSize":[2],"dataType":"uint8x"}"#,
                "uint8x",
            ),
            (r#"{"dimensions":[4],"blockSize":[2]}"#, "no dataType"),
            (
                r#"{"dimensions":[4,0],"blockSize":[2,2],"dataType":"uint8"}"#,
                "dimensions[1]",
            ),
            (
                r#"{"dimensions":[4,3],"blockSize":[2,-2],"dataType":"uint8"}"#,
                "blockSize[1]",
            ),
            (
                r#"{"dimensions":[4,1.5],"blockSize":[2,2],"dataType":"uint8"}"#,
                "dimensions[1]",
            ),
            (
                r#"{"dimensions":[4,"3"],"blockSize":[2,2],"dataType":"uint8"}"#,
                "dimensions[1]",
            ),
            (
                r#"{"dimensions":[4,3],"blockSize":[2],"dataType":"uint8"}"#,
                "side(s)",
            ),
        ];
        for (json, reason) in cases {
            let error = parse_json(json).expect_err(json);
            assert!(error.to_string().contains(reason), "{json}: {error}");
        }
        let unstated = parse_json(r#"{"dimensions":[4],"blockSize":[2],"dataType":"uint8"}"#)
            .expect("a dataset that states none is raw");
        assert_eq!(unstated.compression, Compression::None);
        let huge =
            parse_json(&dataset(r#","dimensions":[4,1000001]"#)).expect_err("over the scene bound");
        assert!(huge.to_string().contains("1000001"), "{huge}");
    }

    #[test]
    fn an_attributes_file_past_the_cap_is_refused_by_the_cap() {
        let dir = std::env::temp_dir().join(format!("phasit_n5_attr_cap_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("attributes.json");
        std::fs::write(&path, vec![b' '; MAX_ATTRIBUTES_BYTES as usize + 1]).unwrap();
        let error = read(&path).expect_err("past the cap");
        assert!(
            matches!(error, Error::OversizedAllocation { .. }),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
