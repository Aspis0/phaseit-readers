// SPDX-License-Identifier: Apache-2.0
//! Nikon ND2 reader: modern, uncompressed frames.
//! The chunkmap, the `CLxLiteVariant` format and the loop model follow `nd2` (Talley Lambert, BSD-3-Clause,
//! https://github.com/tlambert03/nd2); the experiment-loop, attribute and channel parsers re-implement its functions.
//! Keep that attribution, as THIRD-PARTY-NOTICES.md requires, if you touch the framing, CLX parser or loop math.
//!
//! The file ends with a 40-byte locator giving the chunkmap offset. A frame chunk holds an 8-byte prefix, then
//! `height * widthBytes` interleaved (Y, X, C) bytes; each component is its own plane. Only compression 2 is decoded.
//! Rows sit at computable offsets, so `region()` copies only the rows a rect covers. A plane over the decode
//! ceiling is still published: `plane()` refuses it and `region()` serves windows, as for CZI.

mod attributes;
mod chunks;
mod clx;
mod clx_parse;
mod coords;
mod cursor;
mod experiment;
mod metadata;
mod read;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::Path;

use crate::calibration::Calibration;

use memmap2::Mmap;

use crate::error::{Error, Result};
use crate::io::map_file;
use crate::types::{
    checked_region, empty_region_plane, DType, Dims5, Endian, ImageMeta, ImageReader,
    PixelRect, Plane, PlaneBuf, SceneMeta,
};

use self::attributes::parse_attributes;
use self::chunks::{chunk_data, read_chunkmap, ND2_CHUNK_MAGIC, JP2_MAGIC};
use self::clx_parse::{parse_clx_variant};
use self::coords::{axis_count, scene_count, seq_index};
use self::experiment::{load_experiment, Axis, Loop};
use self::metadata::{find_z_step, parse_calibration_um, parse_channels};

/// Plain data plus the `Mmap` (`Send + Sync`): the reader is shareable across threads.
pub struct Nd2Reader {
    mmap: Mmap,
    meta: ImageMeta,
    map: HashMap<Vec<u8>, (u64, u64)>,
    dtype: DType,
    comp: usize,
    height: usize,
    width: usize,
    width_bytes: usize,
    scenes: usize,
    dims: Dims5,
    loops: Vec<Loop>,
}

impl ImageReader for Nd2Reader {
    fn open(path: &Path) -> Result<Self> {
        let mmap = map_file(path)?;
        let buf = &mmap[..];

        // Magic: modern ND2 only; legacy JPEG2000 ND2 is out of scope.
        let head = buf
            .get(0..4)
            .ok_or_else(|| Error::Parse("ND2 file too small for a header".to_string()))?;
        let magic = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
        if magic == JP2_MAGIC {
            return Err(Error::Unsupported(
                "legacy JPEG2000 ND2 not supported".to_string(),
            ));
        }
        if magic != ND2_CHUNK_MAGIC {
            return Err(Error::Parse(format!(
                "not a modern ND2 file (magic {magic:#010x})"
            )));
        }

        let map = read_chunkmap(buf)?;

        let attrs = parse_attributes(buf, &map)?;

        let exp_off = map
            .get(b"ImageMetadataLV!".as_slice())
            .ok_or_else(|| Error::Parse("ND2 missing ImageMetadataLV! chunk".to_string()))?
            .0;
        let exp_root = parse_clx_variant(chunk_data(buf, exp_off, None)?)?;
        let exp = exp_root
            .get_level("SLxExperiment")
            .ok_or_else(|| Error::Parse("ND2 missing SLxExperiment".to_string()))?;
        let loops = load_experiment(exp)?;

        let scenes = scene_count(&loops);
        let dims = Dims5 {
            t: axis_count(&loops, Axis::T),
            c: attrs.comp,
            z: axis_count(&loops, Axis::Z),
            y: attrs.height,
            x: attrs.width,
        };
        // Sized without the decode ceiling: the ceiling guards one materialisation, so an oversize plane is published.
        // Only a size that overflows the byte arithmetic refuses the file; plane() and region() handle the rest.
        dims.plane_bytes_no_ceiling(attrs.dtype)?;

        let cal_um = parse_calibration_um(buf, &map);
        let z_step_um = find_z_step(exp, 0);
        let calibration = cal_um
            .map(|c| Calibration::sized(c as f32, c as f32, z_step_um.map(|v| v as f32)))
            .unwrap_or_default();
        let channels = parse_channels(buf, &map, attrs.comp);
        let stem = file_stem(path);

        let scene_metas: Vec<SceneMeta> = (0..scenes)
            .map(|s| SceneMeta {
                dims,
                dtype: attrs.dtype,
                samples: 1,
                px_size_um: calibration.px_size_um,
                notes: calibration.notes.clone(),
                channels: channels.clone(),
                palette: None,
                name: if scenes == 1 {
                    stem.clone()
                } else {
                    format!("{stem} (P{s})")
                },
            })
            .collect();

        let meta = ImageMeta { name: stem, scenes: scene_metas };

        Ok(Nd2Reader {
            mmap,
            meta,
            map,
            dtype: attrs.dtype,
            comp: attrs.comp,
            height: attrs.height,
            width: attrs.width,
            width_bytes: attrs.width_bytes,
            scenes,
            dims,
            loops,
        })
    }

    fn meta(&self) -> &ImageMeta {
        &self.meta
    }

    fn plane(&self, scene: usize, t: usize, c: usize, z: usize) -> Result<Plane> {
        if scene >= self.scenes {
            return Err(Error::SceneOutOfRange { scene, count: self.scenes });
        }
        let d = self.dims;
        if t >= d.t || c >= d.c || z >= d.z {
            return Err(Error::OutOfRange { scene, t, c, z });
        }
        let seq = seq_index(&self.loops, scene, t, z)?;
        self.decode_plane(seq, c)
    }

    /// Frames are uncompressed with computable row offsets, so a sub-rectangle is read without decoding the plane.
    fn supports_region(&self) -> bool {
        true
    }

    /// Reads only the rect's rows. Byte-identical to `plane(..)?.crop(rect)`, and sized by the rect, so a plane
    /// over the decode ceiling is still readable one window at a time.
    fn region(
        &self,
        scene: usize,
        t: usize,
        c: usize,
        z: usize,
        rect: PixelRect,
    ) -> Result<Plane> {
        if scene >= self.scenes {
            return Err(Error::SceneOutOfRange { scene, count: self.scenes });
        }
        // The shared helper: the same validation order and wording as every other reader.
        checked_region("ND2 region", &self.dims, scene, t, c, z, rect)?;

        // An empty rect is an empty plane, as the trait's default returns.
        if rect.is_empty() {
            return empty_region_plane(self.dtype, 1, rect);
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

        let seq = seq_index(&self.loops, scene, t, z)?;
        let pixels = self.frame_pixels(seq)?;
        // Same check as `decode_plane`: a region read must tell a short frame from a whole one.
        self.check_frame_length(pixels)?;

        let mut raw = vec![0u8; out_bytes];
        if self.comp == 1 {
            // One component: a rect row is contiguous, since a row's padding sits after its pixels.
            self.gather_contiguous_rows(pixels, rect, &mut raw)?;
        } else {
            self.gather_interleaved_pixels(pixels, rect, c, &mut raw)?;
        }

        let data = PlaneBuf::from_bytes(self.dtype, Endian::Little, &raw)?;
        Plane::new(data, [rect.h, rect.w])
    }
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image")
        .to_string()
}
