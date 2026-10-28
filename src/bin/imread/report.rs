// SPDX-License-Identifier: Apache-2.0
//! One file -> one JSON line: what the reader saw of the file, and what it
//! decoded of it.
//!
//! The line is the unit of record of a sweep. It states the format the reader
//! detected, the outcome of the open (opened / refused / error), the metadata
//! of every scene, the scenes the reader refused with their reasons, and — for
//! the planes the sweep decodes — the statistics and the hash of the plane's
//! bytes. Only metadata, statistics and hashes are recorded: no pixel data is
//! written anywhere, and the plane bytes are hashed, never stored.

use std::path::Path;
use std::time::Instant;

use phasit_imread::types::PixelRect;
use phasit_imread::{Dims5, Error, ImageReader, Plane, SceneMeta};

use super::json;
use super::plane;
use super::sha256;

/// The file opened and its scenes are readable.
const OPENED: &str = "opened";
/// The reader refused every scene of the file.
const REFUSED: &str = "refused";
/// The file could not be read at all.
const ERROR: &str = "error";

/// The sentences a reader opens a wholly-refused file's error with.
///
/// The crate has no dedicated error variant for "every scene of this file was
/// refused", so the wording is matched here. Should a reader change its
/// wording, such a file is reported as an error instead of a refusal — never
/// as an opened file.
const REFUSAL_OPENINGS: [&str; 3] = [
    "no scene of this OME-TIFF can be read",
    "LIF has no decodable images",
    "CZI has no readable scene",
];

/// One file, as one JSON line.
///
/// `regions` says whether each window the reader serves is compared with the
/// same window cropped out of the whole plane. The comparison is ON by
/// default; a timing run turns it OFF, so that two builds do exactly the
/// same work per file.
pub fn file_report(path: &Path, regions: bool) -> String {
    let format = match phasit_imread::detect_format(path) {
        Ok(format) => format!("{format:?}"),
        Err(error) => return failed_report(path, None, &error),
    };

    let started = Instant::now();
    let reader = match phasit_imread::open(path) {
        Ok(reader) => reader,
        Err(error) => return failed_report(path, Some(&format), &error),
    };
    let open_ms = elapsed_ms(started);

    let meta = reader.meta();
    let mut scenes = Vec::with_capacity(meta.scenes.len());
    for (index, scene) in meta.scenes.iter().enumerate() {
        scenes.push(scene_json(index, reader.scene_file_index(index), scene));
    }

    let mut planes = Vec::new();
    for (index, scene) in meta.scenes.iter().enumerate() {
        planes.extend(plane_reports(index, reader.as_ref(), scene, regions));
    }
    let refused = reader
        .refused_scenes()
        .iter()
        .map(|refused| {
            format!(
                "{{\"image_index\":{},\"name\":{},\"reason\":{}}}",
                json::integer(refused.image_index),
                json::string(&refused.name),
                json::string(&refused.reason)
            )
        })
        .collect::<Vec<_>>();

    format!(
        "{{\"path\":{},\"format\":{},\"status\":\"{OPENED}\",\"error\":null,\"open_ms\":{},\
         \"supports_region\":{},\
         \"scene_count\":{},\"scenes\":[{}],\"refused_scenes\":[{}],\"planes\":[{}]}}",
        json::string(&path.to_string_lossy()),
        json::string(&format),
        json::number(open_ms),
        reader.supports_region(),
        json::integer(meta.scenes.len()),
        scenes.join(","),
        refused.join(","),
        planes.join(","),
    )
}

/// A file the reader could not open: refused, or an error.
fn failed_report(path: &Path, format: Option<&str>, error: &Error) -> String {
    let message = error.to_string();
    let refusal = refusal_entries(error);
    let (status, kind) = if refusal.is_empty() {
        (ERROR, error_kind(error))
    } else {
        (REFUSED, REFUSED)
    };
    format!(
        "{{\"path\":{},\"format\":{},\"status\":\"{status}\",\
         \"error\":{{\"kind\":\"{kind}\",\"message\":{}}},\"open_ms\":null,\
         \"supports_region\":false,\
         \"scene_count\":0,\"scenes\":[],\"refused_scenes\":[{}],\"planes\":[]}}",
        json::string(&path.to_string_lossy()),
        match format {
            Some(name) => json::string(name),
            None => "null".to_string(),
        },
        json::string(&message),
        refusal.join(","),
    )
}

/// The metadata of one scene.
fn scene_json(index: usize, file_index: usize, scene: &SceneMeta) -> String {
    let dims = scene.dims;
    let channels = scene
        .channels
        .iter()
        .map(|channel| {
            let color = match channel.color {
                Some([r, g, b]) => format!("[{r},{g},{b}]"),
                None => "null".to_string(),
            };
            format!(
                "{{\"name\":{},\"color\":{}}}",
                json::string_or_null(channel.name.as_deref()),
                color
            )
        })
        .collect::<Vec<_>>();

    let px_size = match scene.px_size_um {
        Some(px) => format!(
            "{{\"z\":{},\"y\":{},\"x\":{}}}",
            json::number_or_null(px.z.map(|z| z as f64)),
            json::number(px.y as f64),
            json::number(px.x as f64)
        ),
        None => "null".to_string(),
    };

    format!(
        "{{\"index\":{},\"name\":{},\"file_index\":{},\
         \"dims\":{{\"t\":{},\"c\":{},\"z\":{},\"y\":{},\"x\":{}}},\
         \"dtype\":\"{:?}\",\"samples\":{},\"px_size_um\":{},\"notes\":{},\"channels\":[{}]}}",
        json::integer(index),
        json::string(&scene.name),
        json::integer(file_index),
        json::integer(dims.t),
        json::integer(dims.c),
        json::integer(dims.z),
        json::integer(dims.y),
        json::integer(dims.x),
        scene.dtype,
        json::integer(scene.samples as usize),
        px_size,
        json::string_array(&scene.notes),
        channels.join(","),
    )
}

/// The planes the sweep decodes of one scene.
///
/// The first and the middle Z of `t = 0`, for every channel of the scene: two
/// planes per channel, whatever the depth of the scene is. A plane the reader
/// refuses is a result, not a failure — it is reported with its reason and the
/// rest of the file is still reported.
fn plane_reports(
    scene_index: usize,
    reader: &dyn ImageReader,
    scene: &SceneMeta,
    regions: bool,
) -> Vec<String> {
    let mut reports = Vec::new();
    for z in sweep_z_positions(scene.dims.z) {
        for c in 0..scene.dims.c {
            reports.push(plane_report(scene_index, reader, scene, c, z, regions));
        }
    }
    reports
}

/// The Z positions the sweep decodes: the first and the middle one.
fn sweep_z_positions(z_count: usize) -> Vec<usize> {
    if z_count <= 1 {
        vec![0]
    } else {
        vec![0, z_count / 2]
    }
}

/// One decoded plane: its statistics, its hash, and how long it took.
fn plane_report(
    scene_index: usize,
    reader: &dyn ImageReader,
    scene: &SceneMeta,
    c: usize,
    z: usize,
    regions: bool,
) -> String {
    let started = Instant::now();
    let outcome = reader.plane(scene_index, 0, c, z);
    let decode_ms = elapsed_ms(started);

    match outcome {
        Ok(plane) => {
            let bytes = plane::to_ne_bytes(&plane.data);
            let stats = plane::statistics(&plane.data);
            let windows = if regions {
                region_reports(reader, scene_index, scene, c, z, &plane)
            } else {
                Vec::new()
            };
            format!(
                "{{\"scene\":{},\"t\":0,\"c\":{},\"z\":{},\"dtype\":\"{:?}\",\"samples\":{},\
                 \"bytes\":{},\"min\":{},\"max\":{},\"mean\":{},\"sha256\":\"{}\",\
                 \"decode_ms\":{},\"regions\":[{}],\"error\":null}}",
                json::integer(scene_index),
                json::integer(c),
                json::integer(z),
                plane.dtype(),
                json::integer(plane.samples() as usize),
                json::integer(bytes.len()),
                json::number(stats.min),
                json::number(stats.max),
                json::number(stats.mean),
                sha256::sha256_hex(&bytes),
                json::number(decode_ms),
                windows.join(","),
            )
        }
        Err(error) => format!(
            "{{\"scene\":{},\"t\":0,\"c\":{},\"z\":{},\"dtype\":null,\"samples\":null,\
             \"bytes\":null,\"min\":null,\"max\":null,\"mean\":null,\"sha256\":null,\
             \"decode_ms\":{},\"regions\":[],\"error\":{{\"kind\":\"{}\",\"message\":{}}}}}",
            json::integer(scene_index),
            json::integer(c),
            json::integer(z),
            json::number(decode_ms),
            error_kind(&error),
            json::string(&error.to_string()),
        ),
    }
}

/// The windows the sweep reads of one plane.
///
/// Three windows per plane: a quarter at the origin, a quarter in the middle,
/// and the last pixel. They are the windows a pyramid asks for, and the
/// middle one falls on no chunk boundary in general.
fn sweep_rects(dims: &Dims5) -> Vec<PixelRect> {
    let (w, h) = (dims.x, dims.y);
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let half_w = (w / 2).max(1);
    let half_h = (h / 2).max(1);
    vec![
        PixelRect::new(0, 0, half_w, half_h),
        PixelRect::new(w / 4, h / 4, half_w, half_h),
        PixelRect::new(w - 1, h - 1, 1, 1),
    ]
}

/// The windows the sweep reads of one plane, each compared with the same
/// window cropped out of the plane.
///
/// A window the reader refuses is a result, not a failure: it is reported with
/// its reason. Only hashes are recorded — never pixels.
fn region_reports(
    reader: &dyn ImageReader,
    scene_index: usize,
    scene: &SceneMeta,
    c: usize,
    z: usize,
    plane: &Plane,
) -> Vec<String> {
    sweep_rects(&scene.dims)
        .into_iter()
        .map(|rect| region_report(reader, scene_index, c, z, rect, plane))
        .collect()
}

/// One window: the region the reader gave, and the same window cropped out of
/// the whole plane, each as a hash.
fn region_report(
    reader: &dyn ImageReader,
    scene_index: usize,
    c: usize,
    z: usize,
    rect: PixelRect,
    plane: &Plane,
) -> String {
    let rect_json = format!(
        "[{},{},{},{}]",
        json::integer(rect.x),
        json::integer(rect.y),
        json::integer(rect.w),
        json::integer(rect.h)
    );
    match (reader.region(scene_index, 0, c, z, rect), plane.crop(rect)) {
        (Ok(region), Ok(cropped)) => {
            let region_bytes = plane::to_ne_bytes(&region.data);
            let cropped_bytes = plane::to_ne_bytes(&cropped.data);
            let same = region_bytes == cropped_bytes;
            format!(
                "{{\"rect\":{rect_json},\"region_sha256\":{},\"crop_sha256\":{},\
                 \"same\":{},\"error\":null}}",
                json::string(&sha256::sha256_hex(&region_bytes)),
                json::string(&sha256::sha256_hex(&cropped_bytes)),
                same,
            )
        }
        (Err(error), _) => format!(
            "{{\"rect\":{rect_json},\"region_sha256\":null,\"crop_sha256\":null,\
             \"same\":null,\"error\":{{\"kind\":\"{}\",\"message\":{}}}}}",
            error_kind(&error),
            json::string(&error.to_string()),
        ),
        (_, Err(error)) => format!(
            "{{\"rect\":{rect_json},\"region_sha256\":null,\"crop_sha256\":null,\
             \"same\":null,\"error\":{{\"kind\":\"{}\",\"message\":{}}}}}",
            error_kind(&error),
            json::string(&error.to_string()),
        ),
    }
}

/// The kind of a reader error, as a stable short name.
fn error_kind(error: &Error) -> &'static str {
    match error {
        Error::Io(_) => "io",
        Error::Unsupported(_) => "unsupported",
        Error::NotImplemented(_) => "not_implemented",
        Error::Decode(_) => "decode",
        Error::Parse(_) => "parse",
        Error::OversizedAllocation { .. } => "oversized_allocation",
        Error::SceneOutOfRange { .. } => "scene_out_of_range",
        Error::OutOfRange { .. } => "out_of_range",
        Error::Cancelled => "cancelled",
        _ => "other",
    }
}

/// The refusals a wholly-refused file's error names, one JSON entry each.
///
/// The crate formats every refusal as `<Image> {index} "{name}": {reason}` and
/// joins them with `"; "`. Only an `Unsupported` error can carry them, and its
/// text — without the `unsupported: ` prefix the error's `Display` adds — is
/// what the readers wrote.
fn refusal_entries(error: &Error) -> Vec<String> {
    let text = match error {
        Error::Unsupported(text) => text.as_str(),
        _ => return Vec::new(),
    };
    if !REFUSAL_OPENINGS
        .iter()
        .any(|opening| text.starts_with(opening))
    {
        return Vec::new();
    }
    text.split("<Image> ")
        .skip(1)
        .filter_map(|entry| {
            let (index, rest) = entry.split_once(' ')?;
            let index = index.parse::<usize>().ok()?;
            let rest = rest.strip_prefix('"')?;
            let (name, reason) = rest.split_once("\": ")?;
            Some(format!(
                "{{\"image_index\":{},\"name\":{},\"reason\":{}}}",
                json::integer(index),
                json::string(name),
                json::string(reason)
            ))
        })
        .collect()
}

/// The milliseconds since `started`.
fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unsupported_error_is_not_a_refusal() {
        let error = Error::Unsupported("unrecognized image format".to_string());
        assert!(refusal_entries(&error).is_empty());
    }

    #[test]
    fn refusals_are_taken_from_the_readers_wording() {
        let error = Error::Unsupported(
            "CZI has no readable scene: <Image> 0 \"first\": the tiles of this scene do not \
             share one pixel type; <Image> 2 \"third\": a tile is missing"
                .to_string(),
        );
        let entries = refusal_entries(&error);
        assert_eq!(entries.len(), 2);
        assert!(entries[0].contains("\"image_index\":0"));
        assert!(entries[0].contains("\"name\":\"first\""));
        assert!(entries[0].contains("do not share one pixel type"));
        assert!(entries[1].contains("\"image_index\":2"));
        assert!(entries[1].contains("\"name\":\"third\""));
    }

    #[test]
    fn the_z_positions_are_the_first_and_the_middle() {
        assert_eq!(sweep_z_positions(1), vec![0]);
        assert_eq!(sweep_z_positions(4), vec![0, 2]);
        assert_eq!(sweep_z_positions(7), vec![0, 3]);
    }
}
