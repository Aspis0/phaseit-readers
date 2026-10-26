// SPDX-License-Identifier: Apache-2.0
//! The scenes of an OME-TIFF: each `<Image>` with its geometry and the plan
//! that says which IFD holds each of its planes.
//!
//! The OME-XML shapes read here are ported from the reference reader `tifffile`
//! (Christoph Gohlke, BSD-3-Clause). Keep this attribution.
//!
//! A scene that cannot be read is refused with its reason named, and the scenes
//! around it are still read. Only a file with no readable scene fails.

use crate::error::{Error, Result};
use crate::tiff_ome::{parse_images, Pixels, TiffData};
use crate::tiff_ome_geometry::scene_geometry;
use crate::tiff_ome_planes::{scene_plan, OmePlan};
use crate::tiff_page::PageFacts;
use crate::types::{RefusedScene, SceneMeta};

/// One OME scene: its metadata and how its planes sit in the file.
#[derive(Debug, Clone)]
pub(crate) struct OmeScene {
    /// The scene metadata.
    pub meta: SceneMeta,
    /// The scene's plane layout.
    pub plan: OmePlan,
    /// Position of the scene's `<Image>` in the file, counting refused ones.
    pub image_index: usize,
}

/// The scenes of an OME-XML description that can be read, in file order, and
/// the refusal of each scene that cannot.
#[derive(Debug)]
pub(crate) struct OmeScenes {
    pub scenes: Vec<OmeScene>,
    /// One entry per refused scene, in file order.
    pub refused: Vec<RefusedScene>,
}

/// The most planes one file may state across all its scenes. Each scene's plane
/// table is capped on its own (16 MiB, so 1 Mi planes); this caps their sum, so
/// one file states at most 4 Mi planes — four scenes' worth. A plane costs 16
/// bytes of table and 8 bytes of collected IFD, so the planes of one file are
/// about 96 MiB of live memory at this cap.
pub(crate) const MAX_PLANES_PER_FILE: usize = 1 << 22;

/// Count a scene's planes against the file's budget, refusing the scene when
/// the file's scenes would state more than [`MAX_PLANES_PER_FILE`] together.
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

/// Parse every `<Image>` of an OME-XML description into one scene each.
///
/// `ifd_count` is the number of pages in the file, and `page_at` reads the facts
/// of the page at an IFD. Each scene's colour model comes from the page of its
/// plane 0.
///
/// The first `<Image>` of the file is the first scene; a later `<Image>` that
/// omits its `<TiffData>` is refused, since the file does not say where its
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

/// The IFD of the scene's plane 0, `(t=0, c=0, z=0)`, which is where its colour
/// model is read from. The block that starts at that plane names it, whatever
/// its position in the file. Absent or unreadable, it is IFD 0; an unreadable
/// attribute is refused by the plane plan, not here.
fn plane_zero_ifd(tiff_data: &[TiffData]) -> usize {
    tiff_data
        .iter()
        .find(|block| starts_at_origin(block))
        .and_then(|block| block.ifd.as_deref())
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0)
}

/// Whether a block's first plane is `(t=0, c=0, z=0)`. In every `DimensionOrder`
/// that is the first plane of the scene, so only such a block covers plane 0.
fn starts_at_origin(block: &TiffData) -> bool {
    [&block.first_t, &block.first_c, &block.first_z]
        .iter()
        .all(|first| match first.as_deref() {
            None => true,
            Some(text) => text.trim().parse::<usize>() == Ok(0),
        })
}

/// Build one scene from its `<Image>`, the `index`-th one in the file: its
/// geometry, then where its planes are.
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
