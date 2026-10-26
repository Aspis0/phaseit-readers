// SPDX-License-Identifier: Apache-2.0
//! TIFF / OME-TIFF reader.
//!
//! A reader over a memory-mapped file. Geometry is parsed once at
//! [`TiffReader::open`] and cached in an [`ImageMeta`]; pixels are decoded
//! lazily per [`plane`](ImageReader::plane) by building a throw-away
//! [`tiff::decoder::Decoder`] over the mapped bytes (the decoder is `&mut self`-only,
//! our `plane` is `&self`, so a fresh decoder per call is the
//! contract-mandated pattern).
//!
//! Two geometry flavours are supported:
//!
//! * **OME-TIFF** — when `ImageDescription` (tag 270) carries OME-XML, every
//!   `<Image>` element is one scene: its `<Pixels>` element is authoritative
//!   for dims/dtype/calibration, and its `<TiffData>` blocks say which IFD
//!   holds each `(t, c, z)` plane. Nothing is inferred from the position of a
//!   scene in the file.
//! * **plain TIFF** — inferred from the file itself: the pages (IFDs) are the
//!   planes, and the `PhotometricInterpretation` says what a pixel in them
//!   means. The pixel size comes from the file's own calibration tags (the
//!   ImageJ description and the resolution tags); a file that states none
//!   stays uncalibrated.
//!
//! Color convention: a pixel's samples stay TOGETHER. A page that carries
//! three samples (an RGB page) is ONE channel of THREE components
//! (`dims.c == 1`, `samples == 3`), not three gray channels — the same model
//! Bio-Formats publishes for such a file (`effectiveSizeC = 1`,
//! `rgbChannelCount = 3`). An indexed page keeps its index values and
//! publishes the palette beside them, the way Bio-Formats keeps them.
//!
//! Series-grouping behaviour (dropping reduced-resolution/thumbnail/mask
//! sub-files from Z, matching plain-TIFF pages into one series) is ported from
//! the reference reader `tifffile` (Christoph Gohlke, BSD-3-Clause,
//! <https://github.com/cgohlke/tifffile>). Only structural facts and Gohlke's
//! algorithms are reproduced; no code was copied (actual pixel decoding here
//! goes through the independent `tiff` crate, not tifffile). Keep this
//! attribution.

use std::io::Cursor;
use std::path::Path;

use tiff::decoder::{Decoder, DecodingResult};
use tiff::tags::Tag;

use memmap2::Mmap;

use crate::error::{Error, Result};
use crate::io::map_file;
use crate::tiff_ome_planes::{self, OmePlan};
use crate::tiff_ome_scene;
use crate::tiff_page;
use crate::tiff_plain::{self, PagesPlan};
use crate::tiff_palette;
use crate::types::{DType, ImageMeta, ImageReader, Plane, PlaneBuf, RefusedScene};

/// Concrete decoder type used everywhere in this module (cursor over the mmap).
type TiffDec<'a> = Decoder<Cursor<&'a [u8]>>;

/// Generous cap for a single IFD value (e.g. a large OME-XML `ImageDescription`
/// ASCII tag): real OME-XML runs from a few hundred KB to a few MB, so 64 MiB
/// leaves large headroom while still bounding a corrupt/huge declared length.
///
/// NOTE: verified against the vendored `tiff` 0.11.3 source: ASCII/array IFD
/// value reads are actually gated by `Limits::decoding_buffer_size`, not
/// `Limits::ifd_value_size` (which exists on the struct but is unread by this
/// version of the crate). We raise BOTH fields so behaviour is correct against
/// the documented contract and any future crate version that DOES enforce
/// `ifd_value_size`.
const MAX_IFD_VALUE_BYTES: usize = 64 * 1024 * 1024;

/// Bytes the `tiff` crate must actually materialize to decode one page:
/// `y * x * samples * dtype.size_bytes()`, checked and capped at
/// [`crate::guard::MAX_DECODED_PLANE_BYTES`].
///
/// This is distinct from [`Dims5::checked_plane_bytes`], which only certifies
/// ONE channel's worth of OUTPUT bytes. On a chunky page `read_image()`
/// decodes the whole page (all `samples` per pixel) in one shot, so a crafted
/// inflated `SamplesPerPixel` can force a real allocation our per-channel
/// guard thought was tiny. Call this BEFORE sizing the decoder's `Limits` /
/// building the decoder.
fn raw_page_bytes(y: usize, x: usize, samples: usize, dtype: DType) -> Result<usize> {
    let bytes = y
        .checked_mul(x)
        .and_then(|v| v.checked_mul(samples))
        .and_then(|v| v.checked_mul(dtype.size_bytes()));
    match bytes {
        None => Err(Error::OversizedAllocation {
            requested: usize::MAX,
            cap: crate::guard::MAX_DECODED_PLANE_BYTES,
        }),
        Some(b) if b > crate::guard::MAX_DECODED_PLANE_BYTES => {
            Err(Error::OversizedAllocation {
                requested: b,
                cap: crate::guard::MAX_DECODED_PLANE_BYTES,
            })
        }
        Some(b) => Ok(b),
    }
}

/// Build `tiff::decoder::Limits` wide enough for `raw_bytes` of pixel data and
/// for a generously large IFD value (see [`MAX_IFD_VALUE_BYTES`]), capped at
/// [`crate::guard::MAX_DECODED_PLANE_BYTES`] so the tiff crate's limit is never
/// wider than our own guard. `checked_plane_bytes` / `raw_page_bytes` remain
/// the authoritative size checks; this only stops the tiff crate's defaults
/// (256 MiB / 1 MiB) from rejecting something our guard already approved.
fn tiff_limits(raw_bytes: usize) -> tiff::decoder::Limits {
    let buf = raw_bytes.clamp(MAX_IFD_VALUE_BYTES, crate::guard::MAX_DECODED_PLANE_BYTES);
    let mut limits = tiff::decoder::Limits::default();
    limits.decoding_buffer_size = buf;
    limits.intermediate_buffer_size = buf;
    limits.ifd_value_size = MAX_IFD_VALUE_BYTES.min(crate::guard::MAX_DECODED_PLANE_BYTES);
    limits
}

/// TIFF / OME-TIFF reader. `Mmap` is `Send + Sync`; `ImageMeta`/`PlanePlan` are
/// plain data, so the whole reader is `Send + Sync` and shareable across threads.
pub struct TiffReader {
    mmap: Mmap,
    meta: ImageMeta,
    /// One plan per scene, in scene order.
    plans: Vec<PlanePlan>,
    /// Plain-TIFF only: the IFD indices that are REAL focal planes, in
    /// order, after dropping reduced-resolution / thumbnail / mask sub-files.
    /// `plane()` maps a requested plane through this (`real_pages[page]`) to
    /// the on-disk IFD. Empty on the OME path (that path computes the IFD
    /// index from `DimensionOrder`).
    real_pages: Vec<usize>,
    /// OME-TIFF only: each `<Image>` that was refused, with its reason. The
    /// readable scenes are in `meta`, in file order.
    refused: Vec<RefusedScene>,
    /// OME-TIFF only: the `<Image>` position of each readable scene, in the same
    /// order as `meta.scenes`. Empty for a plain TIFF, whose one scene is file
    /// position 0.
    file_indices: Vec<usize>,
}

/// How a `(t, c, z)` coordinate of one scene resolves to an IFD.
#[derive(Debug, Clone)]
enum PlanePlan {
    /// Plain TIFF: the pages of the file are the planes, ordered the way
    /// ImageJ stacks them — CHANNEL-FIRST, so the page of channel `c`, slice
    /// `z` and frame `t` is page `c + z * channels + t * channels * slices`.
    /// Every page carries `samples` samples per pixel, side by side.
    Pages(PagesPlan),
    /// OME-TIFF: `(t, c, z)` maps to the IFD its `<TiffData>` names; the whole
    /// decoded page is the plane (one sample per IFD).
    Ome(OmePlan),
}

impl TiffReader {
    /// Resolve `(t, c, z)` of one scene to the IFD that holds it, and the
    /// number of samples the decoded page carries.
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
                // The page of (t, c, z) in ImageJ's channel-first order.
                let page = c
                    .checked_add(z.checked_mul(*channels).unwrap_or(usize::MAX))
                    .and_then(|v| {
                        v.checked_add(
                            t.checked_mul(channels.checked_mul(*slices)?)?,
                        )
                    })
                    .ok_or(Error::OutOfRange { scene, t, c, z })?;
                // Map the requested plane through the real-page table so that
                // reduced/thumbnail/mask IFDs skipped at `open()` are never
                // decoded.
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

        // Build a decoder over the mapped bytes to parse geometry once. Raised
        // limits (see `tiff_limits`) so a legitimately large OME-XML
        // `ImageDescription` is not rejected by the crate's 256 MiB default;
        // no pixel data is decoded here, so `raw_bytes = 0` is enough.
        let mut dec: TiffDec<'_> = Decoder::new(Cursor::new(&mmap[..]))
            .map_err(|e| Error::Decode(e.to_string()))?
            .with_limits(tiff_limits(0));

        // ImageDescription (tag 270), if present, may carry OME-XML. Fail
        // closed (not silently `None`) if the tag is present but unreadable
        // (e.g. `LimitsExceeded` on a big OME-XML block) -- see `description_from`.
        let description: Option<String> = description_from(dec.find_tag(Tag::ImageDescription))?;

        let (scenes, plans, real_pages, refused, file_indices) = match description.as_deref() {
            // OME path resolves IFDs from its `<TiffData>` blocks, so it needs
            // no real-page table (left empty).
            Some(d) if looks_like_ome(d) => {
                let ifd_count = tiff_page::image_count(&mut dec)?;
                let mut page_at = |ifd: usize| {
                    dec.seek_to_image(ifd)
                        .map_err(|e| Error::Decode(format!("seek to IFD {ifd}: {e}")))?;
                    tiff_page::read_page_facts(&mut dec)
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
                        // A scene the XML does not name is named after the file
                        // (the first scene) or after its position (the rest).
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
                let geometry = tiff_plain::infer_plain(&mut dec, description.as_deref())?;
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

        Ok(TiffReader {
            mmap,
            meta,
            plans,
            real_pages,
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

    fn scene_file_index(&self, scene: usize) -> usize {
        self.file_indices.get(scene).copied().unwrap_or(scene)
    }

    fn plane(&self, scene: usize, t: usize, c: usize, z: usize) -> Result<Plane> {
        let sm = self.meta.scene(scene).map_err(|_| Error::OutOfRange {
            scene,
            t,
            c,
            z,
        })?;
        let d = sm.dims;
        if t >= d.t || c >= d.c || z >= d.z {
            return Err(Error::OutOfRange { scene, t, c, z });
        }

        // Size the OUTPUT plane BEFORE allocating anything.
        d.checked_plane_bytes(sm.dtype)?;

        let (ifd, samples) = self.resolve(scene, t, c, z)?;

        // Size the RAW decode buffer BEFORE building the decoder (the raw-buffer sizing): on a
        // chunky page `read_image()` materializes `y*x*samples` elements, not
        // just the one channel `checked_plane_bytes` certified above. This
        // also sizes the decoder's raised `Limits` (the decoder-limit sizing).
        let raw_bytes = raw_page_bytes(d.y, d.x, samples, sm.dtype)?;

        // Fresh decoder per call (decoder is &mut-only; plane is &self).
        let mut dec: TiffDec<'_> = Decoder::new(Cursor::new(&self.mmap[..]))
            .map_err(|e| Error::Decode(e.to_string()))?
            .with_limits(tiff_limits(raw_bytes));
        dec.seek_to_image(ifd)
            .map_err(|e| Error::Decode(format!("seek to IFD {ifd}: {e}")))?;
        // Every page is checked against the scene that owns it, not only the
        // first one the scene was read from.
        tiff_page::read_page_facts(&mut dec)?.require_shape(ifd, (d.x, d.y), sm.dtype, samples)?;

        let buf = match (&self.plans[scene], sm.palette.is_some()) {
            // An indexed page: the `tiff` crate refuses the palette
            // photometric before it reads a pixel, so the strips are read here
            // and the page's index values are what the plane carries.
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
}

/// Turn a raw `find_tag(Tag::ImageDescription)` result into `Option<String>`,
/// failing CLOSED on `Err` (tag present but unreadable) instead of collapsing
/// it to `None` (which would silently fall back to the plain-TIFF geometry
/// model and mis-parse a real OME image). A legitimately large OME-XML block
/// is handled separately by raising the decoder's `Limits` (see `tiff_limits`),
/// not by swallowing the error here.
fn description_from(
    result: tiff::TiffResult<Option<tiff::decoder::ifd::Value>>,
) -> Result<Option<String>> {
    match result {
        Ok(None) => Ok(None),
        Ok(Some(v)) => v.into_string().map(Some).map_err(|e| Error::Decode(e.to_string())),
        Err(e) => Err(Error::Decode(e.to_string())),
    }
}

/// Cheap heuristic: does this `ImageDescription` string carry OME-XML?
fn looks_like_ome(d: &str) -> bool {
    d.contains("<OME") || d.contains("openmicroscopy.org")
}

/// Wrap a decoded page in the matching [`PlaneBuf`]. Refuses decoded dtypes we
/// do not model.
fn plane_result(decoded: DecodingResult) -> Result<PlaneBuf> {
    Ok(match decoded {
        DecodingResult::U8(v) => PlaneBuf::U8(v),
        DecodingResult::U16(v) => PlaneBuf::U16(v),
        DecodingResult::I16(v) => PlaneBuf::I16(v),
        DecodingResult::F32(v) => PlaneBuf::F32(v),
        other => {
            return Err(Error::Unsupported(format!(
                "decoded TIFF sample type {} not modelled by phasit-imread",
                decoding_result_name(&other)
            )))
        }
    })
}

/// Human name for an unsupported [`DecodingResult`] variant (for error messages).
fn decoding_result_name(d: &DecodingResult) -> &'static str {
    match d {
        DecodingResult::U8(_) => "u8",
        DecodingResult::U16(_) => "u16",
        DecodingResult::U32(_) => "u32",
        DecodingResult::U64(_) => "u64",
        DecodingResult::F16(_) => "f16",
        DecodingResult::F32(_) => "f32",
        DecodingResult::F64(_) => "f64",
        DecodingResult::I8(_) => "i8",
        DecodingResult::I16(_) => "i16",
        DecodingResult::I32(_) => "i32",
        DecodingResult::I64(_) => "i64",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_page_bytes_single_channel_matches_plane_bytes() {
        assert_eq!(
            raw_page_bytes(2048, 2048, 1, DType::U16).unwrap(),
            2048 * 2048 * 2
        );
    }

    #[test]
    fn raw_page_bytes_inflated_samples_is_refused() {
        // Raw-buffer sizing: a small per-channel output (checked_plane_bytes on ONE channel)
        // does not certify the RAW decode, which must materialize
        // y*x*samples*dtype_size for an interleaved/chunky page. A crafted
        // inflated SamplesPerPixel must be refused here.
        let r = raw_page_bytes(2048, 2048, 1_000_000, DType::U8);
        assert!(matches!(r, Err(Error::OversizedAllocation { .. })));
    }

    #[test]
    fn raw_page_bytes_overflow_is_refused_not_panicking() {
        let r = raw_page_bytes(usize::MAX, 2, 1, DType::U8);
        assert!(matches!(r, Err(Error::OversizedAllocation { .. })));
    }

    #[test]
    fn description_from_ok_none_is_none() {
        let r: tiff::TiffResult<Option<tiff::decoder::ifd::Value>> = Ok(None);
        assert_eq!(description_from(r).unwrap(), None);
    }

    #[test]
    fn description_from_ok_some_extracts_string() {
        let r: tiff::TiffResult<Option<tiff::decoder::ifd::Value>> =
            Ok(Some(tiff::decoder::ifd::Value::Ascii("<OME>hi</OME>".to_string())));
        assert_eq!(description_from(r).unwrap(), Some("<OME>hi</OME>".to_string()));
    }

    #[test]
    fn description_from_err_fails_closed_not_none() {
        // Fail-closed ImageDescription: a tag-present-but-unreadable Err (e.g. LimitsExceeded on a big
        // OME-XML ImageDescription) must NOT collapse to None (which would
        // silently fall back to the plain-TIFF geometry model and mis-parse a
        // real OME image); it must propagate as an Error::Decode.
        let r: tiff::TiffResult<Option<tiff::decoder::ifd::Value>> = Err(tiff::TiffError::LimitsExceeded);
        assert!(matches!(description_from(r), Err(Error::Decode(_))));
    }

}
