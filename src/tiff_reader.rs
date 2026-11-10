// SPDX-License-Identifier: Apache-2.0
//! TIFF / OME-TIFF reader: geometry is parsed once at `open()`; each `plane()` builds a fresh decoder (`&mut self`).
//! OME-TIFF: each `<Image>` is a scene, `<Pixels>` is authoritative and `<TiffData>` names each plane's IFD. Plain
//! TIFF: the pages are the planes.
//! An RGB page is ONE channel of three components (`dims.c == 1`, `samples == 3`), as Bio-Formats publishes it.
//! Reduced, thumbnail and mask sub-files are dropped from Z, as `tifffile` does (Christoph Gohlke, BSD-3-Clause,
//! https://github.com/cgohlke/tifffile): keep this attribution.

use std::io::Cursor;
use std::path::Path;

use tiff::decoder::Decoder;
use tiff::tags::Tag;

use memmap2::Mmap;

use crate::error::{Error, Result};
use crate::io::map_file;
use crate::tiff_layout;
use crate::tiff_ome_planes::{self, OmePlan};
use crate::tiff_ome_scene;
use crate::tiff_page;
use crate::tiff_pages;
use crate::tiff_palette;
use crate::tiff_plain::{self, PagesPlan};
use crate::tiff_region::{self, TiffDec};
use crate::types::{
    checked_region, empty_region_plane, Dims5, Endian, ImageMeta, ImageReader, PixelRect,
    Plane, PlaneBuf, RefusedScene,
};

mod classify;
mod decode;
mod limits;
#[cfg(test)]
mod tests;

use self::classify::{description_from, looks_like_ome, scenes_serve_regions};
use self::decode::plane_result;
use self::limits::{raw_page_bytes, tiff_limits};

/// Plain data plus the `Mmap`: `Send + Sync`, shareable across threads.
pub struct TiffReader {
    mmap: Mmap,
    meta: ImageMeta,
    plans: Vec<PlanePlan>,
    /// Plain TIFF: the real focal-plane IFDs in order, which `plane()` indexes; empty on the OME path.
    real_pages: Vec<usize>,
    /// OME-TIFF only: the refused `<Image>`s, with their reasons.
    refused: Vec<RefusedScene>,
    /// Each readable scene's `<Image>` position; empty for plain TIFF (one scene at position 0).
    file_indices: Vec<usize>,
    /// Each page's facts and window service, from the one IFD-chain walk at open.
    pages: Vec<tiff_pages::PageEntry>,
    endian: Endian,
    /// Whether every published scene is served by `region()` natively; decided at open, per file.
    native_region: bool,
}

#[derive(Debug, Clone)]
enum PlanePlan {
    /// Plain TIFF: pages are the planes, stacked CHANNEL-FIRST as ImageJ stacks them (`c + z * channels + t * channels
    /// * slices`).
    /// Every page carries `samples` samples per pixel, side by side.
    Pages(PagesPlan),
    /// OME-TIFF: `(t, c, z)` maps to the IFD its `<TiffData>` names; one sample per IFD.
    Ome(OmePlan),
}

impl TiffReader {
    /// Resolves `(t, c, z)` to its IFD and the samples the decoded page carries.
    fn resolve(&self, scene: usize, t: usize, c: usize, z: usize) -> Result<(usize, usize)> {
        let plan = self
            .plans
            .get(scene)
            .ok_or(Error::SceneOutOfRange {
                scene,
                count: self.plans.len(),
            })?;
        match plan {
            PlanePlan::Pages(PagesPlan {
                channels,
                slices,
                samples,
                ..
            }) => {
                let page = c
                    .checked_add(z.checked_mul(*channels).unwrap_or(usize::MAX))
                    .and_then(|v| {
                        v.checked_add(
                            t.checked_mul(channels.checked_mul(*slices)?)?,
                        )
                    })
                    .ok_or(Error::OutOfRange { scene, t, c, z })?;
                // Through the real-page table: skipped reduced, thumbnail and mask IFDs are never decoded.
                let ifd = self
                    .real_pages
                    .get(page)
                    .copied()
                    .ok_or(Error::OutOfRange { scene, t, c, z })?;
                Ok((ifd, *samples))
            }
            PlanePlan::Ome(plan) => {
                let flat = tiff_ome_planes::ome_ifd_index(
                    &plan.order,
                    t,
                    c,
                    z,
                    plan.nt,
                    plan.nc,
                    plan.nz,
                )?;
                let ifd = plan
                    .ifds
                    .get(flat)
                    .copied()
                    .ok_or(Error::OutOfRange { scene, t, c, z })?;
                Ok((ifd, self.meta.scene(scene)?.samples as usize))
            }
        }
    }
}

impl ImageReader for TiffReader {
    fn open(path: &Path) -> Result<Self> {
        let mmap = map_file(path)?;
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("image")
            .to_string();

        // Geometry only, so `raw_bytes` is 0; the raised limits let a large OME-XML description through.
        let mut dec: TiffDec<'_> = Decoder::new(Cursor::new(&mmap[..]))
            .map_err(|e| Error::Decode(e.to_string()))?
            .with_limits(tiff_limits(0));

        // An unreadable description fails the open, never reads as absent (see `description_from`).
        let description: Option<String> = description_from(dec.find_tag(Tag::ImageDescription))?;

        let pages = tiff_pages::walk_pages(&mut dec, &mmap)?;
        let endian = tiff_layout::file_endian(&dec);

        let (scenes, plans, real_pages, refused, file_indices) = match description.as_deref() {
            Some(d) if looks_like_ome(d) => {
                let ifd_count = pages.len();
                let mut page_at = |ifd: usize| {
                    pages
                        .get(ifd)
                        .ok_or_else(|| {
                            Error::Decode(format!(
                                "OME scene names IFD {ifd}, which the file does not hold"
                            ))
                        })?
                        .facts(ifd)
                };
                let ome = tiff_ome_scene::parse_scenes(d, ifd_count, &mut page_at)?;
                let refused = ome.refused;
                let mut scenes = Vec::with_capacity(ome.scenes.len());
                let mut plans = Vec::with_capacity(ome.scenes.len());
                let mut file_indices = Vec::with_capacity(ome.scenes.len());
                for (index, scene) in ome.scenes.into_iter().enumerate() {
                    file_indices.push(scene.image_index);
                    let mut meta = scene.meta;
                    if meta.name.is_empty() {
                        // An unnamed scene takes the file's name (the first) or its position (the rest).
                        meta.name = if index == 0 {
                            name.clone()
                        } else {
                            format!("{name} (scene {index})")
                        };
                    }
                    scenes.push(meta);
                    plans.push(PlanePlan::Ome(scene.plan));
                }
                (scenes, plans, Vec::new(), refused, file_indices)
            }
            _ => {
                // The plain path reads IFD 0's tags, so the decoder seeks back to it after the walk.
                dec.seek_to_image(0)
                    .map_err(|e| Error::Decode(format!("seek to IFD 0: {e}")))?;
                let real: Vec<usize> = pages
                    .iter()
                    .enumerate()
                    .filter(|(_, page)| !page.reduced)
                    .map(|(ifd, _)| ifd)
                    .collect();
                let geometry = tiff_plain::infer_plain(&mut dec, description.as_deref(), &real)?;
                let mut scene = geometry.scene;
                scene.name = name.clone();
                (
                    vec![scene],
                    vec![PlanePlan::Pages(geometry.plan)],
                    geometry.real_pages,
                    Vec::new(),
                    Vec::new(),
                )
            }
        };

        let meta = ImageMeta { name, scenes };

        let native_region = scenes_serve_regions(&pages, &plans, &real_pages);

        Ok(TiffReader {
            mmap,
            meta,
            plans,
            real_pages,
            refused,
            file_indices,
            pages,
            endian,
            native_region,
        })
    }

    fn meta(&self) -> &ImageMeta {
        &self.meta
    }

    fn refused_scenes(&self) -> &[RefusedScene] {
        &self.refused
    }

    fn scene_file_index(&self, scene: usize) -> usize {
        self.file_indices.get(scene).copied().unwrap_or(scene)
    }

    fn plane(&self, scene: usize, t: usize, c: usize, z: usize) -> Result<Plane> {
        let sm = self.meta.scene(scene)?;
        let d = sm.dims;
        if t >= d.t || c >= d.c || z >= d.z {
            return Err(Error::OutOfRange { scene, t, c, z });
        }

        d.checked_plane_bytes(sm.dtype)?;

        let (ifd, samples) = self.resolve(scene, t, c, z)?;

        // Raw bytes (all samples of a chunky page) are sized before the decoder is built.
        let raw_bytes = raw_page_bytes(d.y, d.x, samples, sm.dtype)?;

        let mut dec: TiffDec<'_> = Decoder::new(Cursor::new(&self.mmap[..]))
            .map_err(|e| Error::Decode(e.to_string()))?
            .with_limits(tiff_limits(raw_bytes));
        dec.seek_to_image(ifd)
            .map_err(|e| Error::Decode(format!("seek to IFD {ifd}: {e}")))?;
        // Every page is checked against its own scene, not only the first one read.
        tiff_page::read_page_facts(&mut dec)?.require_shape(ifd, (d.x, d.y), sm.dtype, samples)?;

        let buf = match (&self.plans[scene], sm.palette.is_some()) {
            // Indexed pages are read from their strips: the `tiff` crate refuses the palette photometric.
            (PlanePlan::Pages(PagesPlan { bits, .. }), true) => {
                let indices = tiff_palette::read_indices(&mut dec, &self.mmap, d.x, d.y, *bits)?;
                PlaneBuf::U8(indices)
            }
            _ => {
                let decoded = dec
                    .read_image()
                    .map_err(|e| Error::Decode(format!("decode IFD {ifd}: {e}")))?;
                plane_result(decoded)?
            }
        };
        Plane::new_samples(buf, [d.y, d.x], samples as u32)
    }

    fn supports_region(&self) -> bool {
        self.native_region
    }

    /// Reads only the window, byte-identical to `plane(..)?.crop(rect)`, and sized by the rect.
    /// The path is decided at open per page ([`tiff_region::page_service`]); the default cannot read above the ceiling.
    fn region(
        &self,
        scene: usize,
        t: usize,
        c: usize,
        z: usize,
        rect: PixelRect,
    ) -> Result<Plane> {
        // The shared helper: the same validation order and wording as every other reader.
        let sm = self.meta.scene(scene)?;
        let d = sm.dims;
        checked_region("TIFF region", &d, scene, t, c, z, rect)?;

        let (ifd, samples) = self.resolve(scene, t, c, z)?;

        // An empty rect is an empty plane, as the trait's default returns.
        if rect.is_empty() {
            return empty_region_plane(sm.dtype, samples as u32, rect);
        }

        // Output sized by the rect: a small window of a huge plane is not refused by the ceiling.
        let rect_dims = Dims5 {
            t: 1,
            c: 1,
            z: 1,
            y: rect.h,
            x: rect.w,
        };
        let out_bytes = rect_dims.checked_plane_bytes_samples(sm.dtype, samples as u32)?;

        let page = self.pages.get(ifd).ok_or_else(|| {
            Error::Decode(format!("the file holds no IFD {ifd} for this plane"))
        })?;
        let Some(service) = page.service.as_ref() else {
            return self.plane(scene, t, c, z)?.crop(rect);
        };
        let facts = page.facts(ifd)?;
        facts.require_shape(ifd, (d.x, d.y), sm.dtype, samples)?;

        let mut out = vec![0u8; out_bytes];
        match service {
            tiff_region::Service::Raw { layout, table } => {
                tiff_region::read_raw_window(&self.mmap, layout, table, &facts, rect, &mut out)?;
                // The file's bytes are in the FILE's byte order.
                let buf = PlaneBuf::from_bytes(sm.dtype, self.endian, &out)?;
                Plane::new_samples(buf, [rect.h, rect.w], samples as u32)
            }
            tiff_region::Service::Chunks { layout } => {
                // The decoder allocates the chunk it decodes: its limits must cover that chunk and the output.
                let limits = tiff_limits(out_bytes.max(tiff_layout::MAX_CHUNK_BYTES));
                let mut dec = self.decoder(ifd, limits)?;
                tiff_region::decode_chunks(&mut dec, layout, &facts, rect, &mut out)?;
                // The crate decodes into the host's byte order.
                let buf = PlaneBuf::from_native_bytes(sm.dtype, &out)?;
                Plane::new_samples(buf, [rect.h, rect.w], samples as u32)
            }
        }
    }
}

impl TiffReader {
    fn decoder(&self, ifd: usize, limits: tiff::decoder::Limits) -> Result<TiffDec<'_>> {
        let mut dec: TiffDec<'_> = Decoder::new(Cursor::new(&self.mmap[..]))
            .map_err(|e| Error::Decode(e.to_string()))?
            .with_limits(limits);
        dec.seek_to_image(ifd)
            .map_err(|e| Error::Decode(format!("seek to IFD {ifd}: {e}")))?;
        Ok(dec)
    }
}
