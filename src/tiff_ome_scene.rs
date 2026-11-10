// SPDX-License-Identifier: Apache-2.0
//! The scenes of an OME-TIFF: each `<Image>` with its geometry and the plan that says which IFD holds each plane.
//! Ported from `tifffile` (Christoph Gohlke, BSD-3-Clause): keep this attribution. A refused scene does not stop the
//! others.

use crate::error::{Error, Result};
use crate::tiff_ome::{parse_images, Pixels, TiffData};
use crate::tiff_ome_geometry::scene_geometry;
use crate::tiff_ome_planes::{scene_plan, OmePlan};
use crate::tiff_page::PageFacts;
use crate::types::{RefusedScene, SceneMeta};

#[derive(Debug, Clone)]
pub(crate) struct OmeScene {
    pub meta: SceneMeta,
    pub plan: OmePlan,
    /// Position of the scene's `<Image>` in the file, counting refused ones.
    pub image_index: usize,
}

#[derive(Debug)]
pub(crate) struct OmeScenes {
    pub scenes: Vec<OmeScene>,
    pub refused: Vec<RefusedScene>,
}

/// The most planes one file may state across its scenes: four scenes' worth of the 1 Mi per-scene cap.
/// At this cap the planes cost about 96 MiB of live memory (16 bytes of table and 8 of IFD each).
pub(crate) const MAX_PLANES_PER_FILE: usize = 1 << 22;

fn admit_planes(scene: OmeScene, planes: &mut usize) -> Result<OmeScene> {
    let total = planes.saturating_add(scene.plan.ifds.len());
    if total > MAX_PLANES_PER_FILE {
        return Err(Error::Unsupported(format!(
            "the scenes of this file together state more than {MAX_PLANES_PER_FILE} planes"
        )));
    }
    *planes = total;
    Ok(scene)
}

/// Parses every `<Image>` into one scene; each scene's colour model comes from its plane 0.
/// The first `<Image>` is the first scene; a later one without `<TiffData>` is refused: the file does not say where its
/// planes are.
pub(crate) fn parse_scenes(
    xml: &str,
    ifd_count: usize,
    mut page_at: impl FnMut(usize) -> Result<PageFacts>,
) -> Result<OmeScenes> {
    let document = parse_images(xml)?;
    let mut scenes = Vec::with_capacity(document.images.len());
    let mut refused = Vec::new();
    let mut planes = 0;
    for (index, image) in document.images.iter().enumerate() {
        let built = page_at(plane_zero_ifd(&image.tiff_data))
            .and_then(|page| build_scene(image, &page, document.uuid.as_deref(), index, ifd_count))
            .and_then(|scene| admit_planes(scene, &mut planes));
        match built {
            Ok(scene) => scenes.push(scene),
            Err(reason) => refused.push(RefusedScene {
                image_index: index,
                name: image.name.clone().unwrap_or_else(|| "unnamed".to_string()),
                reason: reason.to_string(),
            }),
        }
    }
    if scenes.is_empty() {
        let reasons: Vec<String> = refused.iter().map(ToString::to_string).collect();
        return Err(Error::Unsupported(format!(
            "no scene of this OME-TIFF can be read: {}",
            reasons.join("; ")
        )));
    }
    Ok(OmeScenes { scenes, refused })
}

/// The IFD of plane 0 (`t=0, c=0, z=0`), where the colour model is read from; IFD 0 when absent.
fn plane_zero_ifd(tiff_data: &[TiffData]) -> usize {
    tiff_data
        .iter()
        .find(|block| starts_at_origin(block))
        .and_then(|block| block.ifd.as_deref())
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0)
}

/// Whether a block's first plane is `(t=0, c=0, z=0)`, in every DimensionOrder.
fn starts_at_origin(block: &TiffData) -> bool {
    [&block.first_t, &block.first_c, &block.first_z]
        .iter()
        .all(|first| match first.as_deref() {
            None => true,
            Some(text) => text.trim().parse::<usize>() == Ok(0),
        })
}

fn build_scene(
    image: &Pixels,
    page: &PageFacts,
    root_uuid: Option<&str>,
    index: usize,
    ifd_count: usize,
) -> Result<OmeScene> {
    let geometry = scene_geometry(image, page)?;
    let dims = geometry.meta.dims;
    let plan = scene_plan(
        &image.tiff_data,
        root_uuid,
        index == 0,
        geometry.order,
        (dims.t, dims.c, dims.z),
        ifd_count,
    )?;
    Ok(OmeScene {
        meta: geometry.meta,
        plan,
        image_index: index,
    })
}

#[cfg(test)]
#[path = "tiff_ome_scene_tests.rs"]
mod tests;
