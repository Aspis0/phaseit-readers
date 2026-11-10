// SPDX-License-Identifier: Apache-2.0
//! Zeiss LSM reader. LSM is a TIFF variant the `tiff` crate cannot open: its full-resolution planes have
//! `RowsPerStrip = 0` and `PlanarConfiguration = 2` (one strip per channel), so `Decoder::new` fails with
//! `InconsistentSizesEncountered`. So this reader walks the IFD chain and reads the uncompressed strips itself.
//! Facts come from `tifffile` (C. Gohlke, BSD-3-Clause, github.com/cgohlke/tifffile); keep this attribution.
//! Thumbnails (`NewSubfileType` bit 0) are skipped: each real page is one Z per time point; planar strips per channel.
//! `(t, c, z)` maps to page `scene * Zdim + z` and strip block `c`; geometry is in the private CZ-LSM-Info tag (34412).

mod ifd;
mod parse;
mod strips;
mod values;
#[cfg(test)]
mod tests;

use std::path::Path;

use memmap2::Mmap;

use crate::error::{Error, Result};
use crate::io::map_file;
use crate::lsm_channel_colors::read_channel_meta;
use crate::lsm_positions::{build_scenes, LsmLayout};
use crate::lsm_region;
use crate::types::{
    checked_region, empty_region_plane, ChannelMeta, DType, Dims5, Endian, ImageMeta, ImageReader,
    PixelRect, Plane, PlaneBuf,
};

use self::parse::parse_lsm;
use self::strips::{strips_hold_whole_rows, PageStrips};

/// Plain data plus the `Mmap` (`Send + Sync`): the reader is shareable across threads.
pub struct LsmReader {
    mmap: Mmap,
    meta: ImageMeta,
    /// Byte order from the TIFF header (LSM is little-endian in practice).
    endian: Endian,
    dtype: DType,
    /// Canonical dims, shared by every scene of the file.
    dims: Dims5,
    layout: LsmLayout,
    /// Strips per channel within a page: 1 for LSM's single-strip planar layout.
    strips_per_channel: usize,
    real_pages: Vec<PageStrips>,
    /// Decided at open: a window can only be read out of strips that hold whole rows.
    native_region: bool,
}

impl ImageReader for LsmReader {
    fn open(path: &Path) -> Result<Self> {
        let mmap = map_file(path)?;
        let name = file_stem(path);

        let parsed = parse_lsm(&mmap[..])?;

        let dims = Dims5 {
            t: parsed.t,
            c: parsed.channels,
            z: parsed.z,
            y: parsed.y,
            x: parsed.x,
        };

        // Sized without the decode ceiling: an oversize plane is still published, and `region()` serves its windows.
        // Only a size that overflows the byte arithmetic refuses the file.
        dims.plane_bytes_no_ceiling(parsed.dtype)?;

        // Without a channel-colours block the file still opens, with unnamed channels.
        let channels = match parsed.channel_colors_offset {
            Some(offset) => read_channel_meta(&mmap[..], offset, parsed.endian, dims.c),
            None => vec![ChannelMeta::default(); dims.c],
        };

        // One scene per tile or position, named as Bio-Formats names its series.
        let scenes = build_scenes(
            &name,
            &parsed.layout,
            dims,
            parsed.dtype,
            parsed.calibration,
            channels,
        );

        let meta = ImageMeta {
            name,
            scenes,
        };

        // Decided per file, before the reader is published (see `strips_hold_whole_rows`).
        let native_region = match dims.x.checked_mul(parsed.dtype.size_bytes()) {
            Some(row_bytes) => parsed.real_pages.iter().all(|page| {
                strips_hold_whole_rows(
                    page,
                    parsed.strips_per_channel,
                    dims.c,
                    row_bytes,
                    dims.y,
                )
            }),
            None => false,
        };

        Ok(LsmReader {
            mmap,
            meta,
            endian: parsed.endian,
            dtype: parsed.dtype,
            dims,
            layout: parsed.layout,
            strips_per_channel: parsed.strips_per_channel,
            real_pages: parsed.real_pages,
            native_region,
        })
    }

    fn meta(&self) -> &ImageMeta {
        &self.meta
    }

    fn plane(&self, scene: usize, t: usize, c: usize, z: usize) -> Result<Plane> {
        let d = self.dims;
        // A coordinate outside its scene is OutOfRange: never a panic, never a wrap.
        if scene >= self.layout.scene_count() {
            return Err(Error::SceneOutOfRange {
                scene,
                count: self.layout.scene_count(),
            });
        }
        if t >= d.t || c >= d.c || z >= d.z {
            return Err(Error::OutOfRange { scene, t, c, z });
        }

        // Size the OUTPUT plane BEFORE reserving/reading anything.
        let plane_bytes = d.checked_plane_bytes(self.dtype)?;

        // Scene x Z -> real page; the reader refuses T > 1, whose page order is not established.
        // TODO(perf): open() walks the whole IFD chain once, fine for LSM's tens of pages.
        let page_idx = self
            .layout
            .page_index(scene, z)
            .ok_or(Error::OutOfRange { scene, t, c, z })?;
        let page = self
            .real_pages
            .get(page_idx)
            .ok_or(Error::OutOfRange { scene, t, c, z })?;

        let spc = self.strips_per_channel;
        let start = c * spc;
        let end = start + spc;
        if end > page.offsets.len() {
            return Err(Error::Decode(format!(
                "LSM page {page_idx} has {} strips, need channel {c} block [{start},{end})",
                page.offsets.len()
            )));
        }

        let mut raw = Vec::with_capacity(plane_bytes);
        for s in start..end {
            let off = usize::try_from(page.offsets[s])
                .map_err(|_| Error::Decode("LSM strip offset overflows usize".to_string()))?;
            let len = usize::try_from(page.bytes[s])
                .map_err(|_| Error::Decode("LSM strip length overflows usize".to_string()))?;
            let stop = off
                .checked_add(len)
                .filter(|&s| s <= self.mmap.len())
                .ok_or_else(|| {
                    Error::Decode(format!("LSM strip [{off}+{len}) out of file bounds"))
                })?;
            // Bound the running total before each copy: many small in-bounds strips could otherwise grow `raw`
            // past the plane and abort on allocation (uncatchable). Capped at `plane_bytes`.
            if raw.len().saturating_add(len) > plane_bytes {
                return Err(Error::Decode(format!(
                    "LSM channel {c} page {page_idx}: strip bytes exceed plane size {plane_bytes}"
                )));
            }
            raw.extend_from_slice(&self.mmap[off..stop]);
        }

        if raw.len() != plane_bytes {
            return Err(Error::Decode(format!(
                "LSM channel {c} of page {page_idx}: {} strip bytes != {plane_bytes} plane bytes",
                raw.len()
            )));
        }

        let buf = PlaneBuf::from_bytes(self.dtype, self.endian, &raw)?;
        Plane::new(buf, [d.y, d.x])
    }

    fn supports_region(&self) -> bool {
        self.native_region
    }

    /// Reads only the rows the rect covers; byte-identical to `plane(..)?.crop(rect)`, and sized by the rect,
    /// so a plane over the decode ceiling is still readable one window at a time.
    fn region(
        &self,
        scene: usize,
        t: usize,
        c: usize,
        z: usize,
        rect: PixelRect,
    ) -> Result<Plane> {
        // The shared helper: the same validation order and wording as every other reader.
        let d = self.dims;
        if scene >= self.layout.scene_count() {
            return Err(Error::SceneOutOfRange {
                scene,
                count: self.layout.scene_count(),
            });
        }
        if t >= d.t || c >= d.c || z >= d.z {
            return Err(Error::OutOfRange { scene, t, c, z });
        }
        checked_region("LSM region", &d, scene, t, c, z, rect)?;

        if rect.is_empty() {
            return empty_region_plane(self.dtype, 1, rect);
        }

        if !self.native_region {
            // Strips without whole rows cannot be windowed: the default path decodes and crops instead.
            return self.plane(scene, t, c, z)?.crop(rect);
        }

        // Output sized by the rect: a small window of a huge plane is not refused by the ceiling.
        let rect_dims = Dims5 {
            t: 1,
            c: 1,
            z: 1,
            y: rect.h,
            x: rect.w,
        };
        let out_bytes = rect_dims.checked_plane_bytes(self.dtype)?;

        let page_idx = self
            .layout
            .page_index(scene, z)
            .ok_or(Error::OutOfRange { scene, t, c, z })?;
        let page = self
            .real_pages
            .get(page_idx)
            .ok_or(Error::OutOfRange { scene, t, c, z })?;

        let strips = lsm_region::Strips {
            offsets: &page.offsets,
            bytes: &page.bytes,
            per_channel: self.strips_per_channel,
        };
        let mut raw = vec![0u8; out_bytes];
        lsm_region::gather_rows(
            &self.mmap,
            strips,
            c,
            &d,
            self.dtype,
            rect,
            &mut raw,
        )?;

        let buf = PlaneBuf::from_bytes(self.dtype, self.endian, &raw)?;
        Plane::new(buf, [rect.h, rect.w])
    }
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image")
        .to_string()
}
