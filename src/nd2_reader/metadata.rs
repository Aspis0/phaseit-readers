use std::collections::HashMap;

use crate::types::ChannelMeta;

use super::chunks::chunk_data;
use super::clx::{ClxMap, ClxValue};
use super::clx_parse::parse_clx_variant;
use super::experiment::{as_loop_params, exp_children, MAX_EXP_DEPTH};

/// X/Y calibration in micrometres per pixel from `ImageCalibrationLV|0!`.
pub(super) fn parse_calibration_um(buf: &[u8], map: &HashMap<Vec<u8>, (u64, u64)>) -> Option<f64> {
    let (off, _) = map.get(b"ImageCalibrationLV|0!".as_slice())?;
    let data = chunk_data(buf, *off, None).ok()?;
    let root = parse_clx_variant(data).ok()?;
    let cal = root.get_level("SLxCalibration")?;
    cal.get_f64("dCalibration").filter(|&v| v > 0.0)
}

/// Z step in µm from the ZStackLoop parameters (`dZStep`, or `dZHigh`/`dZLow`/count); best-effort.
pub(super) fn find_z_step(exp: &ClxMap, depth: usize) -> Option<f64> {
    if depth > MAX_EXP_DEPTH {
        return None;
    }
    if exp.get_i64("eType") == Some(4) {
        if let Some(lp) = exp.get("uLoopPars").and_then(as_loop_params) {
            if let Some(step) = lp.get_f64("dZStep").filter(|&v| v != 0.0) {
                return Some(step.abs());
            }
            let count = lp.get_usize("uiCount").unwrap_or(0);
            if count > 1 {
                if let (Some(hi), Some(lo)) = (lp.get_f64("dZHigh"), lp.get_f64("dZLow")) {
                    return Some((hi - lo).abs() / (count as f64 - 1.0));
                }
            }
        }
    }
    if let Some(next) = exp.get("ppNextLevelEx") {
        for child in exp_children(next) {
            if let Some(s) = find_z_step(child, depth + 1) {
                return Some(s);
            }
        }
    }
    None
}

/// Per-channel names and colours from `ImageMetadataSeqLV|0!`, padded to `comp`; best-effort, as `nd2`'s
/// `load_metadata`.
pub(super) fn parse_channels(buf: &[u8], map: &HashMap<Vec<u8>, (u64, u64)>, comp: usize) -> Vec<ChannelMeta> {
    let mut out: Vec<ChannelMeta> = Vec::new();
    if let Some((off, _)) = map.get(b"ImageMetadataSeqLV|0!".as_slice()) {
        if let Ok(data) = chunk_data(buf, *off, None) {
            if let Ok(root) = parse_clx_variant(data) {
                let planes = root
                    .get_level("SLxPictureMetadata")
                    .and_then(|m| m.get_level("sPicturePlanes"))
                    .and_then(|pp| pp.get_level("sPlaneNew").or_else(|| pp.get_level("sPlane")));
                if let Some(planes) = planes {
                    for (_, v) in &planes.entries {
                        if let ClxValue::Level(plane) = v {
                            let name = plane
                                .get_str("sDescription")
                                .map(str::to_string)
                                .filter(|s| !s.is_empty());
                            let color = plane.get_u32("uiColor").map(abgr_to_rgb);
                            out.push(ChannelMeta { name, color });
                        }
                    }
                }
            }
        }
    }
    // One metadata record maps to one component. A record carrying `uiCompCount > 1` (RGB-style) leaves its
    // trailing components unnamed and uncoloured: metadata-only, no effect on pixels.
    out.resize(comp, ChannelMeta::default());
    out
}

/// Decode a LIM `uiColor` (packed ABGR, R in the low byte) to display `[R,G,B]`.
fn abgr_to_rgb(v: u32) -> [u8; 3] {
    [(v & 0xFF) as u8, ((v >> 8) & 0xFF) as u8, ((v >> 16) & 0xFF) as u8]
}
