use std::collections::HashMap;

use crate::calibration::Calibration;
use crate::error::{Error, Result};
use crate::types::{ChannelMeta, DType, Dims5, SceneMeta};

use super::color::lut_color;
use super::layout::SceneLayout;
use super::xml::{attr_f64, attr_u64, XmlNode};

/// Other `DimID`s (emission, mosaic, rotation, loop) are not modelled: their image is skipped.
const DIM_X: u32 = 1;

const DIM_Y: u32 = 2;

const DIM_Z: u32 = 3;

const DIM_T: u32 = 4;

pub(super) enum ImageBuild {
    Scene(SceneMeta, SceneLayout),
    Refused { name: String, reason: String },
}

type Refusal<T> = std::result::Result<T, String>;

/// One image Element becomes a scene, or is REFUSED by name: every failure refuses this image and no other.
/// The open fails only when no image of the file can be read at all.
pub(super) fn build_scene(
    el: &XmlNode,
    blocks: &HashMap<String, (usize, usize)>,
    file_len: usize,
    has_companion: bool,
) -> ImageBuild {
    let name = el.attr("Name").unwrap_or("image").to_string();
    match scene_of(el, &name, blocks, file_len, has_companion) {
        Ok((scene, layout)) => ImageBuild::Scene(scene, layout),
        Err(reason) => ImageBuild::Refused { name, reason },
    }
}

/// The checks of one image, in order; the first failure is the refusal's reason.
fn scene_of(
    el: &XmlNode,
    name: &str,
    blocks: &HashMap<String, (usize, usize)>,
    file_len: usize,
    has_companion: bool,
) -> Refusal<(SceneMeta, SceneLayout)> {
    let image_desc = el
        .child("Data")
        .and_then(|d| d.child("Image"))
        .and_then(|i| i.child("ImageDescription"))
        .ok_or_else(|| "the element carries no image description".to_string())?;
    let axes = parse_axes(image_desc)?;
    let chans = parse_channels(image_desc)?;
    let dtype = channel_dtype(&chans)?;
    let row_stride = check_contiguous(&axes.x, &axes.y, dtype.size_bytes())?;
    let (dims, frame_bytes) = frame_dims(&axes, chans.len(), dtype)?;

    let t_bytes_inc = axes.t.as_ref().map(|t| t.bytes_inc).unwrap_or(0);
    let z_bytes_inc = axes.z.as_ref().map(|z| z.bytes_inc).unwrap_or(0);
    check_strides(&axes, &chans, dims, frame_bytes, z_bytes_inc, t_bytes_inc)?;

    let (mbid, block_offset, block_size) = memory_block(el, blocks, has_companion)?;
    let channel_bytes_inc: Vec<usize> = chans.iter().map(|c| c.bytes_inc).collect();
    let max_offset = max_plane_offset(
        dims,
        frame_bytes,
        t_bytes_inc,
        z_bytes_inc,
        &channel_bytes_inc,
    )?;
    let block_end = check_span(mbid, block_offset, block_size, file_len, max_offset)?;

    let cal = calibration(&axes.x, &axes.y, axes.z.as_ref());
    let scene = SceneMeta {
        dims,
        dtype,
        samples: 1, // LIF planes are grayscale (one sample per pixel).
        px_size_um: cal.px_size_um,
        notes: cal.notes,
        channels: channel_metas(&chans),
        palette: None,
        name: name.to_string(),
    };
    let layout = SceneLayout {
        dims,
        dtype,
        block_offset,
        block_end,
        t_bytes_inc,
        z_bytes_inc,
        channel_bytes_inc,
        row_stride,
    };
    Ok((scene, layout))
}

struct Axes {
    x: ParsedDim,
    y: ParsedDim,
    z: Option<ParsedDim>,
    t: Option<ParsedDim>,
}

fn parse_axes(image_desc: &XmlNode) -> Refusal<Axes> {
    let dims_node = image_desc
        .child("Dimensions")
        .ok_or_else(|| "the image states no Dimensions".to_string())?;
    let mut x: Option<ParsedDim> = None;
    let mut y: Option<ParsedDim> = None;
    let mut z: Option<ParsedDim> = None;
    let mut t: Option<ParsedDim> = None;
    for dd in dims_node.children_named("DimensionDescription") {
        let dim = parse_dim(dd)?;
        // Classify the DimID before the zero-count guard: an unmodelled axis with 0 elements refuses only this image.
        let slot = match dim.id {
            DIM_X => &mut x,
            DIM_Y => &mut y,
            DIM_Z => &mut z,
            DIM_T => &mut t,
            other => {
                return Err(format!(
                    "the image has a dimension (DimID {other}) we do not model"
                ))
            }
        };
        // Only a modelled axis (X/Y/Z/T) is required to carry elements.
        if dim.n == 0 {
            return Err(format!(
                "the image has a dimension (DimID {}) with 0 elements",
                dim.id
            ));
        }
        if slot.is_some() {
            return Err(format!(
                "the image has a duplicate dimension (DimID {})",
                dim.id
            ));
        }
        *slot = Some(dim);
    }
    match (x, y) {
        (Some(x), Some(y)) => Ok(Axes { x, y, z, t }),
        _ => Err("the image states no X and Y dimension".to_string()),
    }
}

fn parse_dim(dd: &XmlNode) -> Refusal<ParsedDim> {
    Ok(ParsedDim {
        id: attr_as(dd, "DimID")?,
        n: attr_as(dd, "NumberOfElements")?,
        bytes_inc: attr_as(dd, "BytesInc")?,
        length: attr_f64(dd, "Length").unwrap_or(0.0),
    })
}

/// A missing or malformed numeric attribute refuses this image by name, not the whole open.
fn attr_as<T: TryFrom<u64>>(node: &XmlNode, key: &str) -> Refusal<T> {
    let v = attr_u64(node, key)?;
    T::try_from(v).map_err(|_| format!("the image states a {key} that does not fit the reader"))
}

/// Channels sorted by BytesInc: that is the canonical channel order.
fn parse_channels(image_desc: &XmlNode) -> Refusal<Vec<ParsedChannel>> {
    let channels_node = image_desc
        .child("Channels")
        .ok_or_else(|| "the image states no Channels".to_string())?;
    let mut chans: Vec<ParsedChannel> = Vec::new();
    for cd in channels_node.children_named("ChannelDescription") {
        chans.push(ParsedChannel {
            data_type: attr_as(cd, "DataType")?,
            resolution: attr_as(cd, "Resolution")?,
            bytes_inc: attr_as(cd, "BytesInc")?,
            lut_name: cd.attr("LUTName").unwrap_or("").to_string(),
        });
    }
    if chans.is_empty() {
        return Err("the image states no channel".to_string());
    }
    chans.sort_by_key(|c| c.bytes_inc);
    Ok(chans)
}

/// Channels share one dtype; an unmodelled pair refuses this image, not the file.
fn channel_dtype(chans: &[ParsedChannel]) -> Refusal<DType> {
    let dtype = dtype_from_channel(chans[0].data_type, chans[0].resolution)
        .map_err(|_| "the image has a channel of an unsupported pixel type".to_string())?;
    for c in &chans[1..] {
        match dtype_from_channel(c.data_type, c.resolution) {
            Ok(d) if d == dtype => {}
            _ => {
                return Err("the channels of the image do not share one pixel type".to_string());
            }
        }
    }
    Ok(dtype)
}

/// The frame must be contiguous: X inner (BytesInc == itemsize), Y next (BytesInc == itemsize * X.n);
/// interleaved or stride-padded rows would make a single Y*X read wrong, so they are refused.
fn check_contiguous(x: &ParsedDim, y: &ParsedDim, itemsize: usize) -> Refusal<usize> {
    if x.bytes_inc != itemsize {
        return Err("the X dimension of the image is not contiguous".to_string());
    }
    let row_stride = itemsize.checked_mul(x.n).ok_or_else(|| {
        "the image states a row stride that overflows the reader's byte arithmetic".to_string()
    })?;
    if y.bytes_inc != row_stride {
        return Err("the Y dimension of the image is not contiguous".to_string());
    }
    Ok(row_stride)
}

/// Sized without the decode ceiling: an oversize plane is still published; only overflow refuses the image.
fn frame_dims(axes: &Axes, channels: usize, dtype: DType) -> Refusal<(Dims5, usize)> {
    let dims = Dims5 {
        t: axes.t.as_ref().map(|d| d.n).unwrap_or(1),
        c: channels,
        z: axes.z.as_ref().map(|d| d.n).unwrap_or(1),
        y: axes.y.n,
        x: axes.x.n,
    };
    let frame_bytes = match dims.plane_bytes_no_ceiling(dtype) {
        Ok(v) => v,
        Err(Error::OversizedAllocation { requested, cap }) => {
            return Err(format!(
                "the image states a plane of {requested} bytes, which overflows the reader's \
                 byte arithmetic (its cap is {cap} bytes)"
            ))
        }
        Err(e) => return Err(format!("the image states a plane the reader cannot size: {e}")),
    };
    Ok((dims, frame_bytes))
}

/// Stride-alias guard: consecutive channels must step by at least one whole frame, and Z/T strides by their extent,
/// because the max_offset bound only proves the worst-case plane fits, not that planes do not overlap.
fn check_strides(
    axes: &Axes,
    chans: &[ParsedChannel],
    dims: Dims5,
    frame_bytes: usize,
    z_bytes_inc: usize,
    t_bytes_inc: usize,
) -> Refusal<()> {
    for w in chans.windows(2) {
        if w[1].bytes_inc < w[0].bytes_inc.saturating_add(frame_bytes) {
            return Err("the channels of the image overlap in memory".to_string());
        }
    }
    if axes.z.is_some() && dims.z > 1 && z_bytes_inc < frame_bytes {
        return Err("the Z dimension of the image overlaps in memory".to_string());
    }
    // One T step spans dims.z * z_bytes_inc, or one frame when there is no Z axis.
    let z_span = if axes.z.is_some() {
        dims.z.checked_mul(z_bytes_inc).ok_or_else(|| {
            "the image states a Z span that overflows the reader's byte arithmetic".to_string()
        })?
    } else {
        frame_bytes
    };
    if axes.t.is_some() && dims.t > 1 && t_bytes_inc < z_span {
        return Err("the T dimension of the image overlaps in memory".to_string());
    }
    Ok(())
}

/// An absent memory block means the pixels are ext-resident: this image is refused.
fn memory_block<'a>(
    el: &'a XmlNode,
    blocks: &HashMap<String, (usize, usize)>,
    has_companion: bool,
) -> Refusal<(&'a str, usize, usize)> {
    let mbid = el
        .child("Memory")
        .and_then(|m| m.attr("MemoryBlockID"))
        .ok_or_else(|| "the image names no memory block".to_string())?;
    match blocks.get(mbid) {
        Some(&(off, size)) if size > 0 => Ok((mbid, off, size)),
        _ => {
            let where_ = if has_companion {
                format!("its data block {mbid:?} is ext-resident")
            } else {
                format!("its data block {mbid:?} is absent from this .lif")
            };
            Err(format!("the pixels of the image are not in this file ({where_})"))
        }
    }
}

fn max_plane_offset(
    dims: Dims5,
    frame_bytes: usize,
    t_bytes_inc: usize,
    z_bytes_inc: usize,
    channel_bytes_inc: &[usize],
) -> Refusal<usize> {
    (dims.t - 1)
        .checked_mul(t_bytes_inc)
        .and_then(|v| v.checked_add((dims.z - 1).checked_mul(z_bytes_inc)?))
        .and_then(|v| v.checked_add(*channel_bytes_inc.last().unwrap()))
        .and_then(|v| v.checked_add(frame_bytes))
        .ok_or_else(|| {
            "the image states a plane offset that overflows the reader's byte arithmetic".to_string()
        })
}

/// A truncated or mismatched block is caught here, before any `plane()` call.
fn check_span(
    mbid: &str,
    block_offset: usize,
    block_size: usize,
    file_len: usize,
    max_offset: usize,
) -> Refusal<usize> {
    let block_end = block_offset.checked_add(block_size).ok_or_else(|| {
        "the image states a data block that ends past the reader's byte arithmetic".to_string()
    })?;
    if block_offset.checked_add(max_offset).is_none()
        || block_offset + max_offset > block_end
        || block_end > file_len
    {
        return Err(format!(
            "the computed data span exceeds its memory block {mbid:?}"
        ));
    }
    Ok(block_end)
}

fn channel_metas(chans: &[ParsedChannel]) -> Vec<ChannelMeta> {
    chans
        .iter()
        .map(|c| ChannelMeta {
            name: if c.lut_name.is_empty() {
                None
            } else {
                Some(c.lut_name.clone())
            },
            color: lut_color(&c.lut_name),
        })
        .collect()
}

struct ParsedDim {
    id: u32,
    n: usize,
    bytes_inc: usize,
    length: f64,
}

struct ParsedChannel {
    data_type: u32,
    resolution: u32,
    bytes_inc: usize,
    lut_name: String,
}

/// LIF stores unsigned ints (DataType 0) or floats (DataType 1) only; anything else is refused.
pub(super) fn dtype_from_channel(data_type: u32, resolution: u32) -> Result<DType> {
    match (data_type, resolution) {
        (0, r) if (1..=8).contains(&r) => Ok(DType::U8),
        (0, r) if (9..=16).contains(&r) => Ok(DType::U16),
        (1, 32) => Ok(DType::F32),
        _ => Err(Error::Unsupported(format!(
            "LIF channel (DataType={data_type}, Resolution={resolution}) not modelled"
        ))),
    }
}

/// Best-effort pixel spacing in µm, as liffile's `coords`: `spacing = Length / (N-1)`.
/// A pixel size needs X and Y; Z goes through the shared rule (a placeholder publishes `None`).
fn calibration(x: &ParsedDim, y: &ParsedDim, z: Option<&ParsedDim>) -> Calibration {
    let spacing_um = |d: &ParsedDim| -> Option<f32> {
        if d.n > 1 && d.length > 0.0 {
            Some((d.length / (d.n as f64 - 1.0) * 1e6) as f32)
        } else {
            None
        }
    };
    match (spacing_um(x), spacing_um(y)) {
        (Some(px), Some(py)) => Calibration::sized(px, py, z.and_then(spacing_um)),
        _ => Calibration::default(),
    }
}

#[cfg(test)]
mod tests;
