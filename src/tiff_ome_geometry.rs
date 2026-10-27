// SPDX-License-Identifier: Apache-2.0
//! What one OME scene is, read from its `<Pixels>` and checked against its
//! first page: its size, pixel type, colour model, calibration, channels, and
//! the order its planes are stored in.
//!
//! The OME-XML shapes read here are ported from the reference reader `tifffile`
//! (Christoph Gohlke, BSD-3-Clause). Keep this attribution.
//!
//! A `<Pixels>` that states something this reader does not model, or that
//! disagrees with the page it describes, is refused with the reason named.
//! Nothing here reads pixels.

use crate::calibration::Calibration;
use crate::error::{Error, Result};
use crate::tiff_calibration::micrometers_per_unit;
use crate::tiff_ome::Pixels;
use crate::tiff_ome_planes::{parse_dim_order, Axis};
use crate::tiff_page::PageFacts;
use crate::tiff_tags;
use crate::types::{ChannelMeta, DType, Dims5, SceneMeta};

/// The scene a `<Pixels>` describes, and the order its planes are stored in.
pub(crate) struct SceneGeometry {
    pub meta: SceneMeta,
    pub order: [Axis; 3],
}

/// Read the geometry of one scene from its `<Pixels>`, given the facts of the
/// first page its planes come from.
pub(crate) fn scene_geometry(pixels: &Pixels, page: &PageFacts) -> Result<SceneGeometry> {
    let (sx, sy) = declared_plane_size(pixels)?;
    let (sz, sc, st) = declared_counts(pixels)?;
    refuse_interleaved(pixels)?;
    let dtype = checked_dtype(pixels, page)?;
    check_page_size(page, (sx, sy))?;

    // `XYCZT` is the OME spec default DimensionOrder, so falling back to
    // it when the attribute is absent is spec-correct.
    let order = parse_dim_order(pixels.dim_order.as_deref().unwrap_or("XYCZT"))?;

    let samples = samples_of_page(page)?;
    let (channels, sc) = scene_channels(pixels, samples, sc)?;

    let calibration = scene_calibration(pixels);
    let meta = SceneMeta {
        dims: Dims5 {
            t: st,
            c: sc,
            z: sz,
            y: sy,
            x: sx,
        },
        dtype,
        samples: samples as u32,
        px_size_um: calibration.px_size_um,
        notes: calibration.notes,
        channels,
        palette: None,
        name: pixels.name.clone().unwrap_or_default(),
    };
    Ok(SceneGeometry { meta, order })
}

/// One OME `PhysicalSize*` attribute in micrometres, or `None` when it is
/// absent, non-positive, or named in a unit the calibration table cannot
/// read. An absent unit attribute is the spec default, the micrometre.
fn physical_size_um(value: Option<f32>, unit: Option<&str>) -> Option<f32> {
    let scale = match unit {
        Some(name) => micrometers_per_unit(name)?,
        None => 1.0,
    };
    let um = value? * scale as f32;
    (um.is_finite() && um > 0.0).then_some(um)
}

/// Map an OME `Pixels/@Type` string to our [`DType`].
fn ome_dtype(t: &str) -> Result<DType> {
    match t.trim() {
        "uint8" => Ok(DType::U8),
        "uint16" => Ok(DType::U16),
        "int16" => Ok(DType::I16),
        "float" => Ok(DType::F32),
        other => Err(Error::Unsupported(format!(
            "OME PixelType {other:?} not modelled by phasit-imread"
        ))),
    }
}

/// The samples per pixel a scene carries, as its first page states them.
///
/// A grey page carries one sample. An RGB page carries its components together
/// in the page, so the scene is one channel of that many components, the model
/// Bio-Formats publishes for such a file. Any other photometric is refused.
fn samples_of_page(page: &PageFacts) -> Result<usize> {
    match page.photometric {
        tiff_tags::PHOTOMETRIC_BLACK_IS_ZERO | tiff_tags::PHOTOMETRIC_WHITE_IS_ZERO
            if page.samples == 1 =>
        {
            Ok(1)
        }
        tiff_tags::PHOTOMETRIC_RGB if page.samples >= 3 => Ok(page.samples),
        tiff_tags::PHOTOMETRIC_PALETTE => Err(Error::Unsupported(
            "OME-TIFF with indexed (RGBPalette) pages not supported yet".to_string(),
        )),
        other => Err(Error::Unsupported(format!(
            "OME-TIFF page with photometric {} and {} samples per pixel not supported yet",
            tiff_tags::photometric_name(other),
            page.samples
        ))),
    }
}

/// The `SizeX` and `SizeY` a `<Pixels>` states. Both are required.
fn declared_plane_size(pixels: &Pixels) -> Result<(usize, usize)> {
    match (pixels.size_x, pixels.size_y) {
        (Some(x), Some(y)) => Ok((x, y)),
        _ => Err(Error::Parse(
            "OME-XML <Pixels> missing SizeX/SizeY".to_string(),
        )),
    }
}

/// The `SizeZ`, `SizeC` and `SizeT` a `<Pixels>` states. A count that does not
/// parse refuses the scene by name; an absent one is 1.
fn declared_counts(pixels: &Pixels) -> Result<(usize, usize, usize)> {
    let sz = declared_count("SizeZ", pixels.size_z.as_deref())?;
    let sc = declared_count("SizeC", pixels.size_c.as_deref())?;
    let st = declared_count("SizeT", pixels.size_t.as_deref())?;
    Ok((sz, sc, st))
}

/// One declared count: 1 when absent, and at least 1 when present.
fn declared_count(name: &str, raw: Option<&str>) -> Result<usize> {
    match raw {
        None => Ok(1),
        Some(text) => text
            .trim()
            .parse::<usize>()
            .map(|count| count.max(1))
            .map_err(|_| {
                Error::Parse(format!(
                    "OME-XML <Pixels> {name} {text:?} is not a whole number"
                ))
            }),
    }
}

/// Interleaved (chunky-C) OME stores all channels in one IFD; the plan
/// assumes one channel per IFD (Interleaved="false"), so fail closed rather
/// than emit confusing seek/length decode errors.
fn refuse_interleaved(pixels: &Pixels) -> Result<()> {
    if pixels.interleaved {
        return Err(Error::Unsupported(
            "interleaved (chunky-C) OME-TIFF not yet supported".to_string(),
        ));
    }
    Ok(())
}

/// The pixel type a `<Pixels>` states, which must be the type of the first page.
fn checked_dtype(pixels: &Pixels, page: &PageFacts) -> Result<DType> {
    let dtype = match pixels.pixel_type.as_deref() {
        Some(t) => ome_dtype(t)?,
        None => {
            return Err(Error::Parse(
                "OME-XML <Pixels> missing Type/PixelType".to_string(),
            ))
        }
    };
    if dtype != page.dtype {
        return Err(Error::Unsupported(format!(
            "OME PixelType says {dtype:?} but the first page holds {:?}",
            page.dtype
        )));
    }
    Ok(dtype)
}

/// The first page must be the plane size the `<Pixels>` states.
fn check_page_size(page: &PageFacts, (sx, sy): (usize, usize)) -> Result<()> {
    if (page.width, page.height) != (sx, sy) {
        return Err(Error::Unsupported(format!(
            "the first page is {}x{} but SizeX/SizeY state {sx}x{sy}",
            page.width, page.height
        )));
    }
    Ok(())
}

/// The channels of a scene and its SizeC, as the page's samples allow them.
///
/// A grey page is one plane per channel, so the scene has the channels the XML
/// states, padded to SizeC. An RGB page holds its channels in one page, so the
/// scene is one channel and SizeC is 1 or the samples per pixel. That channel
/// keeps the name of the XML's `<Channel>` when the XML states exactly one.
fn scene_channels(
    pixels: &Pixels,
    samples: usize,
    sc: usize,
) -> Result<(Vec<ChannelMeta>, usize)> {
    if samples == 1 {
        let mut channels: Vec<ChannelMeta> = pixels.channels.clone();
        channels.resize(sc, ChannelMeta::default());
        return Ok((channels, sc));
    }
    if sc == 1 || sc == samples {
        let channel = match pixels.channels.as_slice() {
            [only] => only.clone(),
            _ => ChannelMeta::default(),
        };
        return Ok((vec![channel], 1));
    }
    Err(Error::Unsupported(format!(
        "SizeC {sc} with {samples} samples per pixel: a plane holds all its channels in one page, so SizeC must be 1 or {samples}"
    )))
}

/// The pixel size of a scene, from its `PhysicalSize*` attributes.
///
/// A PhysicalSize converts from its declared unit (the spec default is the
/// micrometre when the Unit attribute is absent). A unit the table cannot read
/// names no scale, so the size is not published rather than silently read as
/// micrometres. A Z step the file does not record stays "not recorded"
/// (`None`), the convention every reader in this crate and the plain-TIFF
/// path use; a placeholder in its place is not a step either.
fn scene_calibration(pixels: &Pixels) -> Calibration {
    match (
        physical_size_um(pixels.px_x, pixels.px_x_unit.as_deref()),
        physical_size_um(pixels.px_y, pixels.px_y_unit.as_deref()),
    ) {
        (Some(x), Some(y)) => Calibration::sized(
            x,
            y,
            physical_size_um(pixels.px_z, pixels.px_z_unit.as_deref()),
        ),
        _ => Calibration::default(),
    }
}
