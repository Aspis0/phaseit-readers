// SPDX-License-Identifier: Apache-2.0
//! LIF reader ported from `liffile` (Christoph Gohlke, BSD-3-Clause, pinned 2026.4.11); keep this attribution.
//! Only container facts and the plane-offset algorithm are reproduced; no code is copied from GPL-3.0 `readlif`.
//! A `.lif` is memory blocks under a UTF-16LE XML tree; frame = `t*T.BytesInc + z*Z.BytesInc + channel[c].BytesInc`.
//! Contiguous frames: `region()` reads a rect's rows. Refused images keep their place; over-ceiling ones are published.

mod blocks;
mod color;
mod layout;
mod scene;
mod xml;
#[cfg(test)]
mod tests;

use std::path::Path;

use memmap2::Mmap;

use crate::error::{Error, Result};
use crate::io::map_file;
use crate::types::{
    checked_region, empty_region_plane, Dims5, Endian, ImageMeta, ImageReader,
    PixelRect, Plane, PlaneBuf, RefusedScene, SceneMeta,
};

use self::blocks::{read_u32, walk_memory_blocks};
use self::layout::SceneLayout;
use self::scene::{build_scene, ImageBuild};
use self::xml::{collect_images, decode_utf16le, parse_dom, xml_version, XmlNode, MAX_XML_BYTES};

/// Plain data plus the `Mmap` (`Send + Sync`): the reader is shareable across threads.
pub struct LifReader {
    mmap: Mmap,
    meta: ImageMeta,
    /// Parallel to `meta.scenes`: `layouts[s]` describes `meta.scenes[s]`.
    layouts: Vec<SceneLayout>,
    refused: Vec<RefusedScene>,
    /// Parallel to `meta.scenes`: the position of each readable scene in the file's own list of images.
    file_indices: Vec<usize>,
}

impl ImageReader for LifReader {
    fn open(path: &Path) -> Result<Self> {
        let mmap = map_file(path)?;
        let file_name = file_stem(path);

        // --- header: id0(0x70) u32 | descSize u32 | id1(0x2A) u8 | strlen u32 ---
        let buf = &mmap[..];
        if buf.len() < 13 {
            return Err(Error::Parse("LIF file too small for a header".to_string()));
        }
        if read_u32(buf, 0)? != 0x70 || buf[8] != 0x2A {
            return Err(Error::Parse(
                "LIF: bad header test values (expected 0x70 … 0x2A)".to_string(),
            ));
        }
        let xml_chars = read_u32(buf, 9)? as usize;
        let xml_bytes = xml_chars
            .checked_mul(2)
            .filter(|&b| b <= MAX_XML_BYTES)
            .ok_or_else(|| {
                Error::OversizedAllocation {
                    requested: xml_chars.saturating_mul(2),
                    cap: MAX_XML_BYTES,
                }
            })?;
        let xml_start = 13usize;
        let xml_end = xml_start
            .checked_add(xml_bytes)
            .filter(|&e| e <= buf.len())
            .ok_or_else(|| Error::Parse("LIF XML header extends past end of file".to_string()))?;
        let xml = decode_utf16le(&buf[xml_start..xml_end])?;

        let root = parse_dom(&xml)?;
        let version = xml_version(&root)?;

        let blocks = walk_memory_blocks(buf, xml_end, version)?;

        // Ext-resident pixels live in the `.lifext` companion; the refusal names that only when the companion exists.
        let has_companion = path.with_extension("lifext").exists();

        // The `.lifext` holds the children of the `.lif`'s images (histograms, pyramid levels): not user images.
        // so they are neither published nor listed as refused (Bio-Formats does the same).
        // The `*_pmd_N` levels could later speed up the pyramid: no code for that exists yet.
        let mut scenes: Vec<SceneMeta> = Vec::new();
        let mut layouts: Vec<SceneLayout> = Vec::new();
        let mut file_indices: Vec<usize> = Vec::new();
        let mut refused: Vec<RefusedScene> = Vec::new();
        let mut images: Vec<&XmlNode> = Vec::new();
        collect_images(&root, &mut images);

        // Every image element holds its place in the file's list, so an undecodable one is refused, not dropped.
        for (file_index, el) in images.iter().enumerate() {
            match build_scene(el, &blocks, buf.len(), has_companion) {
                ImageBuild::Scene(scene, layout) => {
                    file_indices.push(file_index);
                    scenes.push(scene);
                    layouts.push(layout);
                }
                ImageBuild::Refused { name, reason } => {
                    refused.push(RefusedScene {
                        image_index: file_index,
                        name,
                        reason,
                    });
                }
            }
        }

        // Fails only when no image can be read; the error then names every refusal.
        if scenes.is_empty() {
            let reasons: Vec<String> = refused.iter().map(ToString::to_string).collect();
            return Err(Error::Unsupported(format!(
                "LIF has no decodable images: {}",
                reasons.join("; ")
            )));
        }

        let meta = ImageMeta {
            name: file_name,
            scenes,
        };
        Ok(LifReader {
            mmap,
            meta,
            layouts,
            refused,
            file_indices,
        })
    }

    fn meta(&self) -> &ImageMeta {
        &self.meta
    }

    fn refused_scenes(&self) -> &[RefusedScene] {
        &self.refused
    }

    /// The rank in the file's own list of images; differs from `scene` after a refused image.
    fn scene_file_index(&self, scene: usize) -> usize {
        self.file_indices.get(scene).copied().unwrap_or(scene)
    }

    /// Decodes the whole plane. A plane over the decode ceiling is refused here (`OversizedAllocation`);
    /// the image stays published, and `region()` serves its windows.
    fn plane(&self, scene: usize, t: usize, c: usize, z: usize) -> Result<Plane> {
        // Bad scene index is SceneOutOfRange; bad (t,c,z) is OutOfRange.
        let layout = self.layouts.get(scene).ok_or(Error::SceneOutOfRange {
            scene,
            count: self.layouts.len(),
        })?;
        let d = layout.dims;
        if t >= d.t || c >= d.c || z >= d.z {
            return Err(Error::OutOfRange { scene, t, c, z });
        }

        // Sized before reading; the decode ceiling applies here, and only here.
        let frame_bytes = d.checked_plane_bytes(layout.dtype)?;

        let offset = layout.plane_offset(t, c, z)?;

        let start = layout
            .block_offset
            .checked_add(offset)
            .ok_or_else(|| Error::Decode("LIF plane start overflow".to_string()))?;
        let end = start
            .checked_add(frame_bytes)
            .ok_or_else(|| Error::Decode("LIF plane end overflow".to_string()))?;

        // The frame must lie fully inside this image's memory block AND the file.
        if end > layout.block_end || end > self.mmap.len() {
            return Err(Error::Decode(format!(
                "LIF plane (scene {scene} t{t} c{c} z{z}) spans [{start},{end}) past block/file bounds"
            )));
        }

        let raw = &self.mmap[start..end];
        let data = PlaneBuf::from_bytes(layout.dtype, Endian::Little, raw)?;
        Plane::new(data, [d.y, d.x])
    }

    /// Frames are contiguous, so a sub-rectangle is read from its rows alone.
    fn supports_region(&self) -> bool {
        true
    }

    /// Reads only the rect's rows; byte-identical to `plane(..)?.crop(rect)`, and sized by the rect so an
    /// image over the decode ceiling is still readable one window at a time.
    fn region(
        &self,
        scene: usize,
        t: usize,
        c: usize,
        z: usize,
        rect: PixelRect,
    ) -> Result<Plane> {
        // The shared helper: the same validation order and wording as every other reader.
        let layout = self.layouts.get(scene).ok_or(Error::SceneOutOfRange {
            scene,
            count: self.layouts.len(),
        })?;
        let d = layout.dims;
        checked_region("LIF region", &d, scene, t, c, z, rect)?;

        // An empty rect is an empty plane, as the trait's default returns.
        if rect.is_empty() {
            return empty_region_plane(layout.dtype, 1, rect);
        }

        // Output sized by the rect: a small window of a huge plane is not refused by the ceiling.
        let rect_dims = Dims5 {
            t: 1,
            c: 1,
            z: 1,
            y: rect.h,
            x: rect.w,
        };
        let out_bytes = rect_dims.checked_plane_bytes(layout.dtype)?;

        let itemsize = layout.dtype.size_bytes();
        let row_bytes = rect
            .w
            .checked_mul(itemsize)
            .ok_or_else(|| Error::Decode("LIF region row size overflow".to_string()))?;
        let base = layout.plane_offset(t, c, z)?;

        // A full-width rect is one contiguous read; a narrower rect is one read per row.
        let mut raw = Vec::with_capacity(out_bytes);
        if rect.w == d.x {
            let offset = base
                .checked_add(
                    rect.y
                        .checked_mul(layout.row_stride)
                        .ok_or_else(|| Error::Decode("LIF region row offset overflow".to_string()))?,
                )
                .ok_or_else(|| Error::Decode("LIF region offset overflow".to_string()))?;
            let span = rect
                .h
                .checked_mul(layout.row_stride)
                .ok_or_else(|| Error::Decode("LIF region span overflow".to_string()))?;
            raw.extend_from_slice(self.block_bytes(layout, offset, span)?);
        } else {
            let col_offset = rect
                .x
                .checked_mul(itemsize)
                .ok_or_else(|| Error::Decode("LIF region column offset overflow".to_string()))?;
            for row in 0..rect.h {
                let offset = base
                    .checked_add(
                        (rect.y + row)
                            .checked_mul(layout.row_stride)
                            .ok_or_else(|| Error::Decode("LIF region row offset overflow".to_string()))?,
                    )
                    .and_then(|v| v.checked_add(col_offset))
                    .ok_or_else(|| Error::Decode("LIF region offset overflow".to_string()))?;
                raw.extend_from_slice(self.block_bytes(layout, offset, row_bytes)?);
            }
        }

        let data = PlaneBuf::from_bytes(layout.dtype, Endian::Little, &raw)?;
        Plane::new(data, [rect.h, rect.w])
    }
}

impl LifReader {
    /// Refuses a span that runs past the block or the file: the bound `plane()` enforces for a whole frame.
    fn block_bytes(&self, layout: &SceneLayout, offset: usize, len: usize) -> Result<&[u8]> {
        let start = layout
            .block_offset
            .checked_add(offset)
            .ok_or_else(|| Error::Decode("LIF block span overflow".to_string()))?;
        let end = start
            .checked_add(len)
            .ok_or_else(|| Error::Decode("LIF block span overflow".to_string()))?;
        if end > layout.block_end || end > self.mmap.len() {
            return Err(Error::Decode(format!(
                "LIF span [{start},{end}) runs past the memory block or the file"
            )));
        }
        Ok(&self.mmap[start..end])
    }
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image")
        .to_string()
}
