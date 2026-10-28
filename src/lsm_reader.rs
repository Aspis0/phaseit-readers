// SPDX-License-Identifier: Apache-2.0
//! Zeiss LSM reader.
//!
//! LSM is a TIFF variant, but the `tiff` crate **cannot open it**: LSM stores
//! its full-resolution planes with `RowsPerStrip = 0` and
//! `PlanarConfiguration = 2` (one strip per channel), which the crate rejects at
//! `Decoder::new` with `InconsistentSizesEncountered`. So this reader parses the
//! TIFF IFD chain itself and reads the (always uncompressed) strips straight out
//! of the memory-mapped file. This is exactly why LSM has its own backend and
//! does NOT share `tiff_reader.rs`.
//!
//! Layout observed on the reference sample (verified byte-exact against the
//! tifffile `_ref` dumps):
//!
//! * The top-level IFD chain **interleaves** real data pages with RGB thumbnail
//!   pages. Real pages have `NewSubfileType` bit 0 clear and are full size;
//!   thumbnails have bit 0 set (reduced) and are 128×128. We keep only the real
//!   pages — the opposite of what a plain-TIFF walk would want, and the reason a
//!   naive `tiff`-crate read would either fail or mix thumbnails into Z.
//! * Each real page is one Z (per time point) and stores every channel as a
//!   separate **planar** strip. Channel `c` is strip `c` (single strip per
//!   channel because `RowsPerStrip = 0` ⇒ the whole plane is one strip).
//! * Geometry (T/C/Z/Y/X) and calibration come from the private **CZ-LSM-Info**
//!   tag (34412), a little-endian binary struct on the first IFD.
//!
//! `(t, c, z)` → data:
//! * page index in the real-page list = `scene * Zdim + z` — a scene owns a
//!   contiguous block of `Zdim` pages, and a file with a single scene has
//!   `scene = 0`;
//! * channel `c` selects planar strip block `c` within that page.
//!
//! Color convention: this reader maps color onto `dims.c` and always sets
//! `SceneMeta::samples == 1`. Interleaved multi-sample planes (`samples > 1`)
//! are a CZI-only convention; see
//! [`SceneMeta::samples`](crate::types::SceneMeta::samples).
//!
//! The IFD/strip-layout facts above are ported from the reference reader
//! `tifffile` (Christoph Gohlke, BSD-3-Clause,
//! <https://github.com/cgohlke/tifffile>). Only structural facts and Gohlke's
//! algorithms are reproduced; no code was copied (the IFD/strip parsing here
//! is a from-scratch hand-rolled binary reader, not a port of tifffile's
//! Python source). Keep this attribution if you touch the framing or the
//! IFD/strip parsing.
//!
//! The CZ-LSM-Info field layout, the channel-colours block and the tile and
//! position counts are facts of the Zeiss LSM format itself; they were
//! verified byte-for-byte against the owner's LSM files (every file of that
//! corpus carries a 464-byte struct with the magic `0x0400494C`, the voxel
//! sizes at +40/+48/+56, the channel-colours pointer at +108 and the tile and
//! position counts at +264/+268).

use std::path::Path;

use memmap2::Mmap;

use crate::calibration::Calibration;
use crate::error::{Error, Result};
use crate::io::map_file;
use crate::lsm_bytes::{read_u16, read_u32, read_u8};
use crate::lsm_channel_colors::read_channel_meta;
use crate::lsm_cz_info::{self, CzTag};
use crate::lsm_positions::{build_scenes, LsmLayout};
use crate::lsm_region;
use crate::types::{
    checked_region, empty_region_plane, ChannelMeta, DType, Dims5, Endian, ImageMeta, ImageReader,
    PixelRect, Plane, PlaneBuf,
};

/// Private LSM tag carrying the `CZ-LSM-Info` geometry/calibration struct.
const TAG_CZ_LSM_INFO: u16 = 34412;

// Standard TIFF tags we consume from each IFD.
const TAG_NEW_SUBFILE_TYPE: u16 = 254;
const TAG_IMAGE_WIDTH: u16 = 256;
const TAG_IMAGE_LENGTH: u16 = 257;
const TAG_BITS_PER_SAMPLE: u16 = 258;
const TAG_COMPRESSION: u16 = 259;
const TAG_STRIP_OFFSETS: u16 = 273;
const TAG_SAMPLES_PER_PIXEL: u16 = 277;
const TAG_STRIP_BYTE_COUNTS: u16 = 279;
const TAG_PLANAR_CONFIGURATION: u16 = 284;
const TAG_SAMPLE_FORMAT: u16 = 339;

/// Strip table for one real (non-reduced) LSM page.
#[derive(Debug, Clone)]
struct PageStrips {
    /// Absolute byte offsets of this page's strips, in TIFF strip order
    /// (planar: all strips of channel 0, then channel 1, …).
    offsets: Vec<u64>,
    /// Byte length of each strip, index-aligned with [`offsets`](Self::offsets).
    bytes: Vec<u64>,
}

/// Zeiss LSM reader. `Mmap` is `Send + Sync` and every other field is plain
/// data, so the reader is `Send + Sync` and shareable across threads.
pub struct LsmReader {
    mmap: Mmap,
    meta: ImageMeta,
    /// Byte order of the underlying TIFF container (LSM is little-endian in
    /// practice, but we honour the header either way).
    endian: Endian,
    dtype: DType,
    /// Canonical dims (every scene of a file shares them, so one copy is
    /// enough; cached for `plane`).
    dims: Dims5,
    /// The tile/position layout: how many scenes the file holds and which
    /// real page each scene's planes live on.
    layout: LsmLayout,
    /// Strips per channel within a page (`offsets.len() / channels`); 1 for the
    /// single-strip planar layout LSM uses.
    strips_per_channel: usize,
    /// Real (non-reduced) pages, in file order. `real_pages[page_index]`.
    real_pages: Vec<PageStrips>,
    /// Whether every scene of THIS file is served by the native `region()`
    /// path. Decided at open: a window can only be read out of strips that
    /// hold whole rows.
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

        // Size the plane WITHOUT the decode ceiling: the ceiling guards ONE
        // materialisation, not the file's existence. A file whose plane is too
        // big to hold in RAM is still PUBLISHED — `plane()` refuses the
        // materialisation and `region()` serves windows of it — exactly as the
        // CZI, LIF and ND2 readers do. Only a size that OVERFLOWS the reader's
        // byte arithmetic is refused here.
        dims.plane_bytes_no_ceiling(parsed.dtype)?;

        // Channel names and display colours come from the file's
        // channel-colours block. A file without one still opens; its channels
        // stay unnamed and colourless.
        let channels = match parsed.channel_colors_offset {
            Some(offset) => read_channel_meta(&mmap[..], offset, parsed.endian, dims.c),
            None => vec![ChannelMeta::default(); dims.c],
        };

        // One scene per tile/position: a mosaic shot is published as one
        // scene per tile, in file order, named the way Bio-Formats names its
        // series ("<name> #1", "<name> #2", …).
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

        // Whether a window can be read out of this file's strips (see
        // `strips_hold_whole_rows`). Decided per FILE, before the reader is
        // published.
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
        // Every coordinate must be inside the scene it addresses. Reject any
        // other coordinate as OutOfRange (never panic, never wrap).
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

        // (scene, t, c, z) → real page → planar strip block c. A scene owns a
        // contiguous block of Z pages, and inside a scene the pages run
        // Z-ascending (the reader refuses T > 1, whose ordering is not
        // established).
        // TODO(perf): O(1) here, but `open()` walks the whole IFD chain once;
        // that is fine for LSM's tens of pages.
        let page_idx = self
            .layout
            .page_index(scene, z)
            .ok_or(Error::OutOfRange { scene, t, c, z })?;
        let page = self
            .real_pages
            .get(page_idx)
            .ok_or(Error::OutOfRange { scene, t, c, z })?;

        // Gather channel `c`'s strips (contiguous planar block) into one buffer.
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
            // Bound the RUNNING total before each copy: a crafted file can
            // declare millions of small in-bounds strips that individually pass
            // the checks above yet grow `raw` to terabytes → uncatchable
            // `handle_alloc_error` abort. Cap at `plane_bytes` (≤ MAX_DECODED_PLANE_BYTES).
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

        // The strips are native pixel bytes in the container's byte order;
        // `from_bytes` byteswaps to native as needed (LSM is little-endian).
        let buf = PlaneBuf::from_bytes(self.dtype, self.endian, &raw)?;
        Plane::new(buf, [d.y, d.x])
    }

    fn supports_region(&self) -> bool {
        self.native_region
    }

    /// Read only the rows of the plane the rectangle covers.
    ///
    /// The result is byte-identical to `self.plane(scene, t, c, z)?.crop(rect)`:
    /// the same strip bytes are read and byteswapped, over the rows the
    /// rectangle covers alone. The output buffer is sized by the RECT, so the
    /// decode ceiling applies to what is materialised, never to the full
    /// plane — a file whose plane is over the ceiling is still readable here,
    /// one window at a time.
    fn region(
        &self,
        scene: usize,
        t: usize,
        c: usize,
        z: usize,
        rect: PixelRect,
    ) -> Result<Plane> {
        // Validate the coordinate FIRST, then the rect — the same order and
        // the same wording as every other reader (the shared helper).
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

        // An empty rectangle covers no pixels: the answer is an empty plane,
        // the same one the trait's default returns.
        if rect.is_empty() {
            return empty_region_plane(self.dtype, 1, rect);
        }

        if !self.native_region {
            // This file's strips do not hold whole rows, so a window cannot be
            // read out of them. The default path (decode the plane, then crop)
            // is byte-identical and always available.
            return self.plane(scene, t, c, z)?.crop(rect);
        }

        // Size the OUTPUT by the RECT, never by the plane: the ceiling
        // (`MAX_DECODED_PLANE_BYTES`) guards what is materialised, so a small
        // window of a huge plane is not refused.
        let rect_dims = Dims5 {
            t: 1,
            c: 1,
            z: 1,
            y: rect.h,
            x: rect.w,
        };
        let out_bytes = rect_dims.checked_plane_bytes(self.dtype)?;

        // (scene, t, c, z) → real page → planar strip block c (see `plane()`).
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

        // The strips are native pixel bytes in the container's byte order;
        // `from_bytes` byteswaps to native as needed (LSM is little-endian).
        let buf = PlaneBuf::from_bytes(self.dtype, self.endian, &raw)?;
        Plane::new(buf, [rect.h, rect.w])
    }
}

/// Whether a page's strips hold whole rows, so a window can be read out of
/// them.
///
/// The default `region()` path (decode the whole plane, then crop) needs
/// nothing of the strips; the native path reads the rows of a rectangle out of
/// the strips that hold them, which needs each strip to hold a whole number of
/// rows and the channel's strips to cover the plane's rows.
fn strips_hold_whole_rows(
    page: &PageStrips,
    strips_per_channel: usize,
    channels: usize,
    row_bytes: usize,
    plane_rows: usize,
) -> bool {
    if row_bytes == 0 {
        return false;
    }
    for channel in 0..channels {
        let start = match channel.checked_mul(strips_per_channel) {
            Some(start) => start,
            None => return false,
        };
        let end = match start.checked_add(strips_per_channel) {
            Some(end) => end,
            None => return false,
        };
        if end > page.bytes.len() {
            return false;
        }
        let mut rows = 0usize;
        for s in start..end {
            let count = match usize::try_from(page.bytes[s]) {
                Ok(count) => count,
                Err(_) => return false,
            };
            if count % row_bytes != 0 {
                return false;
            }
            rows += count / row_bytes;
        }
        if rows != plane_rows {
            return false;
        }
    }
    true
}

/// Everything `parse_lsm` extracts from the container.
struct ParsedLsm {
    endian: Endian,
    dtype: DType,
    t: usize,
    channels: usize,
    z: usize,
    y: usize,
    x: usize,
    strips_per_channel: usize,
    calibration: Calibration,
    /// The tile/position layout: how many scenes the file holds and which
    /// real page each scene's planes live on.
    layout: LsmLayout,
    /// The file offset of the channel-colours block, when the file states one.
    channel_colors_offset: Option<usize>,
    real_pages: Vec<PageStrips>,
}

/// Parse the LSM container: walk the IFD chain, keep the real (non-reduced)
/// pages and their strip tables, and read geometry/calibration from CZ-LSM-Info.
// Plain modulo (not `usize::is_multiple_of`, Rust 1.87+) keeps the crate
// buildable on older stable toolchains, matching `types.rs`.
#[allow(clippy::manual_is_multiple_of)]
fn parse_lsm(buf: &[u8]) -> Result<ParsedLsm> {
    let endian = read_tiff_header(buf)?;

    // Walk the whole top-level IFD chain, keeping only full-resolution pages.
    let mut next = read_u32(buf, 4, endian)? as u64;
    let mut real_pages: Vec<PageStrips> = Vec::new();
    let mut first_ifd_props: Option<FirstPage> = None;
    let mut cz_tag: Option<CzTag> = None;

    let mut visited = 0usize;
    while next != 0 {
        // Bound the walk so a corrupt cyclic chain cannot loop forever.
        visited += 1;
        if visited > MAX_IFDS {
            return Err(Error::Parse(format!(
                "LSM IFD chain exceeds {MAX_IFDS} entries (corrupt/cyclic?)"
            )));
        }

        let ifd = usize::try_from(next)
            .map_err(|_| Error::Parse("LSM IFD offset overflows usize".to_string()))?;
        let (page, props, cz, next_ifd) = parse_ifd(buf, ifd, endian)?;

        if cz_tag.is_none() {
            cz_tag = cz;
        }
        // Keep only real focal planes (drop reduced / thumbnail sub-files).
        if !props.reduced {
            match &first_ifd_props {
                None => {
                    first_ifd_props = Some(FirstPage {
                        width: props.width,
                        height: props.height,
                        samples: props.samples,
                        bits: props.bits,
                        sample_format: props.sample_format,
                        planar: props.planar,
                        compression: props.compression,
                        strips: page.offsets.len(),
                    });
                }
                Some(first) => {
                    // Every later real page must share page 0's geometry/dtype.
                    // Trusting only page 0 would let a page with different W×H but
                    // coincidentally equal total bytes decode with wrong geometry.
                    if props.width != first.width
                        || props.height != first.height
                        || props.samples != first.samples
                        || props.bits != first.bits
                        || props.sample_format != first.sample_format
                        || props.compression != first.compression
                        || props.planar != first.planar
                    {
                        return Err(Error::Unsupported(format!(
                            "LSM real page {} geometry differs from page 0 \
                             ({}x{} s{} b{} f{} c{} planar={} vs {}x{} s{} b{} f{} c{} planar={})",
                            real_pages.len(),
                            props.width,
                            props.height,
                            props.samples,
                            props.bits,
                            props.sample_format,
                            props.compression,
                            props.planar,
                            first.width,
                            first.height,
                            first.samples,
                            first.bits,
                            first.sample_format,
                            first.compression,
                            first.planar,
                        )));
                    }
                    // Strip-count check: the checks above never compare STRIP COUNT. A
                    // crafted later page can share every scalar field with
                    // page 0 yet declare a different number of strips, so
                    // `plane()`'s `strips_per_channel`-sized slice (derived
                    // from page 0) would select the wrong strip subset on that
                    // page.
                    if page.offsets.len() != first.strips || page.bytes.len() != first.strips {
                        return Err(Error::Decode(format!(
                            "LSM real page {} has {} strip offsets / {} strip byte-counts, \
                             page 0 has {} strips",
                            real_pages.len(),
                            page.offsets.len(),
                            page.bytes.len(),
                            first.strips,
                        )));
                    }
                }
            }
            real_pages.push(page);
        }
        next = next_ifd;
    }

    if real_pages.is_empty() {
        return Err(Error::Decode(
            "LSM has no full-resolution pages".to_string(),
        ));
    }
    let first = first_ifd_props.expect("at least one real page => Some");

    // Only uncompressed LSM is modelled (the reference and the common case);
    // compressed variants would need a codec and are refused clearly.
    if first.compression != 1 {
        return Err(Error::Unsupported(format!(
            "LSM compression {} not supported (only uncompressed)",
            first.compression
        )));
    }

    let dtype = map_dtype(first.bits, first.sample_format)?;

    // Geometry comes from CZ-LSM-Info, cross-checked against the pixel data so a
    // malformed header can never silently mis-shape the volume.
    let cz_tag = cz_tag.ok_or_else(|| {
        Error::Decode("LSM is missing the CZ-LSM-Info tag (34412)".to_string())
    })?;
    let geo = lsm_cz_info::parse(buf, cz_tag, endian)?;

    let (x, y) = (first.width as usize, first.height as usize);
    let samples = first.samples.max(1) as usize;

    // --- axis / geometry allow-list (reject weird or inconsistent shapes) ---
    if geo.dim_x as usize != x || geo.dim_y as usize != y {
        return Err(Error::Decode(format!(
            "LSM CZ-LSM-Info XY ({},{}) disagrees with IFD ({x},{y})",
            geo.dim_x, geo.dim_y
        )));
    }
    if geo.dim_channels as usize != samples {
        return Err(Error::Decode(format!(
            "LSM CZ-LSM-Info channels {} disagrees with SamplesPerPixel {samples}",
            geo.dim_channels
        )));
    }
    let channels = samples;
    let z = geo.dim_z.max(1) as usize;
    let t = geo.dim_time.max(1) as usize;

    // The page order inside a scene is only validated at t=1 (our only
    // sample). A real multi-T LSM with a different on-disk T/Z convention
    // would silently misassign planes, so we fail closed here.
    // TODO: re-enable once a real multi-T .lsm is validated in the parity suite.
    if t > 1 {
        return Err(Error::Unsupported(
            "multi-timepoint LSM (T>1) not yet supported: page ordering unverified".to_string(),
        ));
    }

    // Channels are stored planar (one strip block per channel); interleaved
    // multi-channel LSM is not seen in practice and would need de-interleaving.
    if channels > 1 && !first.planar {
        return Err(Error::Unsupported(
            "interleaved (chunky) multi-channel LSM not supported".to_string(),
        ));
    }

    // Strip layout must divide evenly into per-channel blocks.
    if first.strips == 0 || first.strips % channels != 0 {
        return Err(Error::Decode(format!(
            "LSM page has {} strips, not a multiple of {channels} channels",
            first.strips
        )));
    }
    let strips_per_channel = first.strips / channels;

    // The tile/position layout: how many scenes the file holds, and how many
    // pages each of them owns.
    let layout = LsmLayout::new(geo.dim_p, geo.dim_m, z)?;

    // The real page count must equal T*Z per scene times the number of
    // scenes: each page = one Z at one T of one tile/position, and channels
    // live inside the page as planar strips.
    let expected_pages = layout
        .expected_pages()
        .ok_or_else(|| Error::Decode("LSM T*Z*P*M overflow".to_string()))?;
    if real_pages.len() != expected_pages {
        // The wording names the product the counts imply: T*Z for a plain
        // file, T*Z*P*M for a tiled or multi-position one.
        let implied = if layout.scene_count() == 1 { "T*Z" } else { "T*Z*P*M" };
        return Err(Error::Decode(format!(
            "LSM has {} real pages but CZ-LSM-Info implies {implied} = {expected_pages}",
            real_pages.len()
        )));
    }

    Ok(ParsedLsm {
        endian,
        dtype,
        t,
        channels,
        z,
        y,
        x,
        strips_per_channel,
        calibration: geo.calibration,
        layout,
        channel_colors_offset: geo.channel_colors_offset,
        real_pages,
    })
}

/// Upper bound on IFDs we will follow (guards against a cyclic/corrupt chain).
const MAX_IFDS: usize = 1_000_000;

/// Upper bound on elements in a single strip-table array (`StripOffsets` /
/// `StripByteCounts`). Real LSM files have a handful of strips (one per
/// channel, since `RowsPerStrip = 0`); an attacker-controlled `count` from the
/// IFD entry must never be trusted to size a `Vec<u64>` unbounded (a crafted
/// count can grow the allocation far past the mmapped file size).
const MAX_STRIP_COUNT: usize = 100_000;

/// Scalar properties read from a single IFD (before the reduced-page filter).
struct IfdProps {
    reduced: bool,
    width: u32,
    height: u32,
    samples: u16,
    bits: u16,
    sample_format: u16,
    planar: bool,
    compression: u16,
}

/// Cached properties of the FIRST real page (used to validate geometry/dtype).
struct FirstPage {
    width: u32,
    height: u32,
    samples: u16,
    bits: u16,
    sample_format: u16,
    planar: bool,
    compression: u16,
    strips: usize,
}

/// Parse the TIFF header, returning the container byte order.
///
/// LSM is a classic (non-Big) TIFF; BigTIFF LSM does not exist, so we accept
/// only the `42` magic here.
fn read_tiff_header(buf: &[u8]) -> Result<Endian> {
    if buf.len() < 8 {
        return Err(Error::Parse("LSM file too small for a TIFF header".to_string()));
    }
    let endian = match &buf[0..2] {
        b"II" => Endian::Little,
        b"MM" => Endian::Big,
        _ => return Err(Error::Parse("LSM: not a TIFF (bad byte-order mark)".to_string())),
    };
    let magic = read_u16(buf, 2, endian)?;
    if magic != 42 {
        return Err(Error::Parse(format!(
            "LSM: unexpected TIFF magic {magic} (BigTIFF LSM is not supported)"
        )));
    }
    Ok(endian)
}

/// Parse one IFD at `ifd_off`: return its strip table, scalar properties, the
/// CZ-LSM-Info tag (if it is present here), and the next IFD offset.
fn parse_ifd(
    buf: &[u8],
    ifd_off: usize,
    e: Endian,
) -> Result<(PageStrips, IfdProps, Option<CzTag>, u64)> {
    let count = read_u16(buf, ifd_off, e)? as usize;
    let entries_start = ifd_off + 2;
    // Each entry is 12 bytes; the next-IFD pointer follows the entries.
    let next_ptr_off = entries_start
        .checked_add(count.checked_mul(12).ok_or_else(overflow)?)
        .ok_or_else(overflow)?;
    let next = read_u32(buf, next_ptr_off, e)? as u64;

    let mut props = IfdProps {
        reduced: false,
        width: 0,
        height: 0,
        samples: 1,
        bits: 1,
        sample_format: 1,
        planar: false,
        compression: 1,
    };
    let mut offsets: Vec<u64> = Vec::new();
    let mut byte_counts: Vec<u64> = Vec::new();
    let mut cz_tag: Option<CzTag> = None;

    for i in 0..count {
        let eoff = entries_start + i * 12;
        let id = read_u16(buf, eoff, e)?;
        let ty = read_u16(buf, eoff + 2, e)?;
        let cnt = read_u32(buf, eoff + 4, e)? as u64;
        // The 4 value/offset bytes live at eoff+8.
        match id {
            TAG_NEW_SUBFILE_TYPE => {
                // Bit 0 = reduced-resolution (thumbnail), bit 2 = mask: neither is
                // a real focal plane. Mirrors `tiff_reader::is_reduced`. Bit 1
                // ("page of a multi-page image") is left set — those ARE planes.
                props.reduced = (read_scalar(buf, ty, cnt, eoff + 8, e)? & 0b101) != 0;
            }
            TAG_IMAGE_WIDTH => props.width = read_scalar(buf, ty, cnt, eoff + 8, e)? as u32,
            TAG_IMAGE_LENGTH => props.height = read_scalar(buf, ty, cnt, eoff + 8, e)? as u32,
            TAG_SAMPLES_PER_PIXEL => {
                props.samples = u16::try_from(read_scalar(buf, ty, cnt, eoff + 8, e)?)
                    .map_err(|_| Error::Decode("LSM SamplesPerPixel exceeds u16".to_string()))?
            }
            TAG_BITS_PER_SAMPLE => {
                props.bits = u16::try_from(read_scalar(buf, ty, cnt, eoff + 8, e)?)
                    .map_err(|_| Error::Decode("LSM BitsPerSample exceeds u16".to_string()))?
            }
            TAG_SAMPLE_FORMAT => {
                props.sample_format = u16::try_from(read_scalar(buf, ty, cnt, eoff + 8, e)?)
                    .map_err(|_| Error::Decode("LSM SampleFormat exceeds u16".to_string()))?
            }
            TAG_COMPRESSION => {
                props.compression = u16::try_from(read_scalar(buf, ty, cnt, eoff + 8, e)?)
                    .map_err(|_| Error::Decode("LSM Compression exceeds u16".to_string()))?
            }
            TAG_PLANAR_CONFIGURATION => {
                props.planar = read_scalar(buf, ty, cnt, eoff + 8, e)? == 2
            }
            TAG_STRIP_OFFSETS => offsets = read_int_array(buf, ty, cnt, eoff + 8, e)?,
            TAG_STRIP_BYTE_COUNTS => byte_counts = read_int_array(buf, ty, cnt, eoff + 8, e)?,
            TAG_CZ_LSM_INFO => {
                // CZ-LSM-Info is always far larger than 4 bytes, so the value
                // field MUST be an offset per TIFF's inline-vs-offset rule. A tag
                // whose declared type×count fits in 4 bytes would be inline, not a
                // pointer; treating it as an offset would misread a crafted tag.
                let total = tiff_type_size(ty)
                    .checked_mul(cnt)
                    .ok_or_else(overflow)?;
                if total <= 4 {
                    return Err(Error::Decode(format!(
                        "LSM CZ-LSM-Info tag has {total} inline bytes (≤4); expected an offset"
                    )));
                }
                let declared_bytes = usize::try_from(total)
                    .map_err(|_| Error::Decode("LSM CZ-LSM-Info size overflows usize".to_string()))?;
                cz_tag = Some(CzTag {
                    offset: read_u32(buf, eoff + 8, e)? as usize,
                    declared_bytes,
                });
            }
            _ => {}
        }
    }

    if offsets.len() != byte_counts.len() {
        return Err(Error::Decode(format!(
            "LSM IFD @ {ifd_off}: {} strip offsets vs {} byte counts",
            offsets.len(),
            byte_counts.len()
        )));
    }

    Ok((
        PageStrips {
            offsets,
            bytes: byte_counts,
        },
        props,
        cz_tag,
        next,
    ))
}


/// Map a `(BitsPerSample, SampleFormat)` pair to our [`DType`]. Mirrors the
/// TIFF reader's table (1=uint, 2=int, 3=float).
fn map_dtype(bits: u16, sample_format: u16) -> Result<DType> {
    match (sample_format, bits) {
        (1, 8) => Ok(DType::U8),
        (1, 16) => Ok(DType::U16),
        (2, 16) => Ok(DType::I16),
        (3, 32) => Ok(DType::F32),
        _ => Err(Error::Unsupported(format!(
            "LSM sample type (SampleFormat={sample_format}, {bits}-bit) not modelled"
        ))),
    }
}

/// TIFF field type → element size in bytes (0 for types we do not handle).
fn tiff_type_size(ty: u16) -> u64 {
    match ty {
        1 | 2 | 6 | 7 => 1, // BYTE, ASCII, SBYTE, UNDEFINED
        3 | 8 => 2,         // SHORT, SSHORT
        4 | 9 | 11 => 4,    // LONG, SLONG, FLOAT
        5 | 10 | 12 => 8,   // RATIONAL, SRATIONAL, DOUBLE
        _ => 0,
    }
}

/// Read the FIRST element of an IFD entry as a `u64`, following the offset when
/// the value does not fit inline. `val_off` is the address of the entry's
/// 4-byte value/offset field.
fn read_scalar(buf: &[u8], ty: u16, count: u64, val_off: usize, e: Endian) -> Result<u64> {
    let tsz = tiff_type_size(ty);
    if tsz == 0 || count == 0 {
        return Err(Error::Decode(format!(
            "LSM entry has unsupported type {ty} / count {count}"
        )));
    }
    let total = tsz.checked_mul(count).ok_or_else(overflow)?;
    let data_off = if total <= 4 {
        val_off
    } else {
        read_u32(buf, val_off, e)? as usize
    };
    read_int_at(buf, ty, data_off, e)
}

/// Read a whole integer array (SHORT/LONG) from an IFD entry, following the
/// offset when it does not fit inline.
fn read_int_array(buf: &[u8], ty: u16, count: u64, val_off: usize, e: Endian) -> Result<Vec<u64>> {
    let tsz = tiff_type_size(ty);
    if tsz == 0 {
        return Err(Error::Decode(format!("LSM strip tag has unsupported type {ty}")));
    }
    let total = tsz.checked_mul(count).ok_or_else(overflow)?;
    let data_off = if total <= 4 {
        val_off
    } else {
        read_u32(buf, val_off, e)? as usize
    };
    let n = usize::try_from(count).map_err(|_| overflow())?;
    // Cap BEFORE allocating: an attacker-controlled `count` with only a
    // capacity *hint* can otherwise grow this `Vec<u64>` far past the mmapped
    // file size (up to ~8x, given `Vec<u64>` vs. e.g. 1-byte elements) into an
    // uncatchable `handle_alloc_error` abort.
    if n > MAX_STRIP_COUNT {
        return Err(Error::Decode(format!(
            "LSM strip/tag array has {n} elements, exceeds sane cap {MAX_STRIP_COUNT} (corrupt?)"
        )));
    }
    let mut out = Vec::with_capacity(n.min(1 << 20));
    for i in 0..n {
        let off = i
            .checked_mul(tsz as usize)
            .and_then(|delta| data_off.checked_add(delta))
            .ok_or_else(overflow)?;
        out.push(read_int_at(buf, ty, off, e)?);
    }
    Ok(out)
}

/// Read a single SHORT/LONG value at an absolute offset as `u64`.
fn read_int_at(buf: &[u8], ty: u16, off: usize, e: Endian) -> Result<u64> {
    Ok(match ty {
        3 | 8 => read_u16(buf, off, e)? as u64,
        4 | 9 | 11 => read_u32(buf, off, e)? as u64,
        1 | 6 | 7 => read_u8(buf, off)? as u64,
        other => {
            return Err(Error::Decode(format!(
                "LSM: cannot read type {other} as an integer"
            )))
        }
    })
}

// --- bounds-checked primitive readers ---------------------------------------

fn overflow() -> Error {
    Error::Decode("LSM: arithmetic overflow while parsing IFD".to_string())
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dtype_mapping() {
        assert_eq!(map_dtype(8, 1).unwrap(), DType::U8);
        assert_eq!(map_dtype(16, 1).unwrap(), DType::U16);
        assert_eq!(map_dtype(16, 2).unwrap(), DType::I16);
        assert_eq!(map_dtype(32, 3).unwrap(), DType::F32);
        assert!(map_dtype(32, 1).is_err()); // u32 not modelled
        assert!(map_dtype(64, 3).is_err()); // f64 not modelled
    }

    #[test]
    fn type_sizes() {
        assert_eq!(tiff_type_size(3), 2); // SHORT
        assert_eq!(tiff_type_size(4), 4); // LONG
        assert_eq!(tiff_type_size(1), 1); // BYTE
        assert_eq!(tiff_type_size(5), 8); // RATIONAL
        assert_eq!(tiff_type_size(999), 0); // unknown
    }

    #[test]
    fn primitive_readers_respect_endianness() {
        let le = [0x34, 0x12, 0x78, 0x56, 0x00, 0x00, 0x00, 0x00];
        assert_eq!(read_u16(&le, 0, Endian::Little).unwrap(), 0x1234);
        assert_eq!(read_u32(&le, 0, Endian::Little).unwrap(), 0x5678_1234);
        let be = [0x12, 0x34, 0x56, 0x78];
        assert_eq!(read_u16(&be, 0, Endian::Big).unwrap(), 0x1234);
        assert_eq!(read_u32(&be, 0, Endian::Big).unwrap(), 0x1234_5678);
    }

    #[test]
    fn primitive_readers_bounds_checked() {
        let short = [0x00, 0x01];
        assert!(matches!(read_u32(&short, 0, Endian::Little), Err(Error::Parse(_))));
        assert!(matches!(read_u16(&short, 1, Endian::Little), Err(Error::Parse(_))));
    }

    #[test]
    fn scalar_inline_and_via_offset() {
        // Inline SHORT (count 1): value bytes at val_off.
        let mut buf = vec![0u8; 32];
        buf[8] = 0x02; // low byte of SHORT at val_off=8
        assert_eq!(read_scalar(&buf, 3, 1, 8, Endian::Little).unwrap(), 2);

        // LONG array of 2 (8 bytes) stored at offset 16; value field holds ptr.
        // val_off = 8 holds the u32 offset (16).
        let mut buf2 = vec![0u8; 32];
        buf2[8..12].copy_from_slice(&16u32.to_le_bytes());
        buf2[16..20].copy_from_slice(&100u32.to_le_bytes());
        buf2[20..24].copy_from_slice(&200u32.to_le_bytes());
        let arr = read_int_array(&buf2, 4, 2, 8, Endian::Little).unwrap();
        assert_eq!(arr, vec![100, 200]);
        // Scalar reads the first element through the same offset.
        assert_eq!(read_scalar(&buf2, 4, 2, 8, Endian::Little).unwrap(), 100);
    }

    #[test]
    fn read_int_array_caps_absurd_element_count() {
        // Element-count cap: modest in-bounds buffer (8 MiB of zeros) so every element read
        // in a naive uncapped loop would succeed in-bounds. `count = 8_000_000`
        // (attacker-controlled u32 tag) would allocate/populate an 8M-element
        // `Vec<u64>` (~64 MiB) with only a capacity *hint*; a crafted count can
        // grow this to ~8x the mmapped file size (up to ~512 GiB) -> uncatchable
        // `handle_alloc_error` abort. Real LSM files have a handful of strips.
        let buf = vec![0u8; 8 * 1024 * 1024];
        let r = read_int_array(&buf, 1 /* BYTE */, 8_000_000, 0, Endian::Little);
        assert!(r.is_err(), "absurd element count must be refused before allocating");
    }

    #[test]
    fn tiff_header_parsing() {
        let mut le = vec![0u8; 8];
        le[0..2].copy_from_slice(b"II");
        le[2..4].copy_from_slice(&42u16.to_le_bytes());
        assert_eq!(read_tiff_header(&le).unwrap(), Endian::Little);

        let mut be = vec![0u8; 8];
        be[0..2].copy_from_slice(b"MM");
        be[2..4].copy_from_slice(&42u16.to_be_bytes());
        assert_eq!(read_tiff_header(&be).unwrap(), Endian::Big);

        // BigTIFF magic (43) is refused.
        let mut big = vec![0u8; 8];
        big[0..2].copy_from_slice(b"II");
        big[2..4].copy_from_slice(&43u16.to_le_bytes());
        assert!(matches!(read_tiff_header(&big), Err(Error::Parse(_))));

        assert!(matches!(read_tiff_header(b"XY\x2a\x00\x00\x00\x00\x00"), Err(Error::Parse(_))));
    }

    // --- Strip-count check: minimal hand-rolled classic-TIFF/LSM byte builder ---
    //
    // `parse_lsm` takes a plain `&[u8]` (no file I/O), so a synthetic in-memory
    // file exercises the FULL real parse path directly, unlike the fragment-level
    // helper tests above.

    /// One little-endian classic-TIFF IFD entry (12 bytes: tag, type, count,
    /// value/offset). `value` is written as a raw LE u32 into the 4-byte
    /// value/offset field: for SHORT/LONG `count == 1` this IS the inline
    /// scalar; for larger arrays it is the pointer to external data.
    fn ifd_entry(tag: u16, ty: u16, count: u32, value: u32) -> [u8; 12] {
        let mut e = [0u8; 12];
        e[0..2].copy_from_slice(&tag.to_le_bytes());
        e[2..4].copy_from_slice(&ty.to_le_bytes());
        e[4..8].copy_from_slice(&count.to_le_bytes());
        e[8..12].copy_from_slice(&value.to_le_bytes());
        e
    }

    /// Build a minimal 2-page (Z=2, T=1, 1 channel, 2x2 U8, uncompressed)
    /// classic-TIFF/LSM file: header -> IFD0 (carries CZ-LSM-Info) -> IFD1.
    ///
    /// When `mismatched_strips` is true, IFD1's `StripOffsets`/`StripByteCounts`
    /// declare 2 strips (via an external array) instead of 1, while every OTHER
    /// scalar field (width/height/samples/bits/format/compression/planar) still
    /// matches page 0 exactly -- the precise gap the strip-count check closes (the existing
    /// per-page check compares only those scalar fields, never strip count).
    fn build_minimal_lsm(mismatched_strips: bool) -> Vec<u8> {
        const IFD0_ENTRIES: usize = 11;
        const IFD1_ENTRIES: usize = 10;
        let ifd0_size = 2 + IFD0_ENTRIES * 12 + 4;
        let ifd1_size = 2 + IFD1_ENTRIES * 12 + 4;

        let ifd0_off = 8usize;
        let cz_off = ifd0_off + ifd0_size;
        let cz_len = 28usize;
        let ifd1_off = cz_off + cz_len;
        let ext_off = ifd1_off + ifd1_size;
        let total_len = ext_off + if mismatched_strips { 16 } else { 0 };

        let mut buf = vec![0u8; total_len];
        buf[0..2].copy_from_slice(b"II");
        buf[2..4].copy_from_slice(&42u16.to_le_bytes());
        buf[4..8].copy_from_slice(&(ifd0_off as u32).to_le_bytes());

        // --- IFD0 (page 0 / z=0): also carries the CZ-LSM-Info pointer. ---
        let mut entries0: Vec<u8> = Vec::new();
        entries0.extend(ifd_entry(TAG_NEW_SUBFILE_TYPE, 3, 1, 0));
        entries0.extend(ifd_entry(TAG_IMAGE_WIDTH, 3, 1, 2));
        entries0.extend(ifd_entry(TAG_IMAGE_LENGTH, 3, 1, 2));
        entries0.extend(ifd_entry(TAG_BITS_PER_SAMPLE, 3, 1, 8));
        entries0.extend(ifd_entry(TAG_COMPRESSION, 3, 1, 1));
        entries0.extend(ifd_entry(TAG_STRIP_OFFSETS, 4, 1, 1000)); // arbitrary, unread by parse_lsm
        entries0.extend(ifd_entry(TAG_SAMPLES_PER_PIXEL, 3, 1, 1));
        entries0.extend(ifd_entry(TAG_STRIP_BYTE_COUNTS, 4, 1, 4));
        entries0.extend(ifd_entry(TAG_PLANAR_CONFIGURATION, 3, 1, 1));
        entries0.extend(ifd_entry(TAG_SAMPLE_FORMAT, 3, 1, 1));
        entries0.extend(ifd_entry(TAG_CZ_LSM_INFO, 4, 7, cz_off as u32)); // 4*7=28 bytes -> offset mode
        assert_eq!(entries0.len(), IFD0_ENTRIES * 12);
        buf[ifd0_off..ifd0_off + 2].copy_from_slice(&(IFD0_ENTRIES as u16).to_le_bytes());
        buf[ifd0_off + 2..ifd0_off + 2 + entries0.len()].copy_from_slice(&entries0);
        let next0_off = ifd0_off + 2 + entries0.len();
        buf[next0_off..next0_off + 4].copy_from_slice(&(ifd1_off as u32).to_le_bytes());

        // --- CZ-LSM-Info: the magic, the struct's size and the dimension
        // i32s at +8/+12/+16/+20/+24. ---
        // T=1, Z=2 -> 2 real pages expected (matches the 2 IFDs below).
        buf[cz_off..cz_off + 4].copy_from_slice(&0x0400_494Cu32.to_le_bytes()); // magic
        buf[cz_off + 4..cz_off + 8].copy_from_slice(&(cz_len as u32).to_le_bytes()); // size
        buf[cz_off + 8..cz_off + 12].copy_from_slice(&2i32.to_le_bytes()); // DimX
        buf[cz_off + 12..cz_off + 16].copy_from_slice(&2i32.to_le_bytes()); // DimY
        buf[cz_off + 16..cz_off + 20].copy_from_slice(&2i32.to_le_bytes()); // DimZ
        buf[cz_off + 20..cz_off + 24].copy_from_slice(&1i32.to_le_bytes()); // DimChannels
        buf[cz_off + 24..cz_off + 28].copy_from_slice(&1i32.to_le_bytes()); // DimTime

        // --- IFD1 (page 1 / z=1): identical scalar geometry to page 0. ---
        let mut entries1: Vec<u8> = Vec::new();
        entries1.extend(ifd_entry(TAG_NEW_SUBFILE_TYPE, 3, 1, 0));
        entries1.extend(ifd_entry(TAG_IMAGE_WIDTH, 3, 1, 2));
        entries1.extend(ifd_entry(TAG_IMAGE_LENGTH, 3, 1, 2));
        entries1.extend(ifd_entry(TAG_BITS_PER_SAMPLE, 3, 1, 8));
        entries1.extend(ifd_entry(TAG_COMPRESSION, 3, 1, 1));
        if mismatched_strips {
            // 2 strips instead of 1: external arrays at `ext_off`/`ext_off+8`.
            entries1.extend(ifd_entry(TAG_STRIP_OFFSETS, 4, 2, ext_off as u32));
            entries1.extend(ifd_entry(TAG_SAMPLES_PER_PIXEL, 3, 1, 1));
            entries1.extend(ifd_entry(TAG_STRIP_BYTE_COUNTS, 4, 2, (ext_off + 8) as u32));
        } else {
            entries1.extend(ifd_entry(TAG_STRIP_OFFSETS, 4, 1, 2000));
            entries1.extend(ifd_entry(TAG_SAMPLES_PER_PIXEL, 3, 1, 1));
            entries1.extend(ifd_entry(TAG_STRIP_BYTE_COUNTS, 4, 1, 4));
        }
        entries1.extend(ifd_entry(TAG_PLANAR_CONFIGURATION, 3, 1, 1));
        entries1.extend(ifd_entry(TAG_SAMPLE_FORMAT, 3, 1, 1));
        assert_eq!(entries1.len(), IFD1_ENTRIES * 12);
        buf[ifd1_off..ifd1_off + 2].copy_from_slice(&(IFD1_ENTRIES as u16).to_le_bytes());
        buf[ifd1_off + 2..ifd1_off + 2 + entries1.len()].copy_from_slice(&entries1);
        let next1_off = ifd1_off + 2 + entries1.len();
        buf[next1_off..next1_off + 4].copy_from_slice(&0u32.to_le_bytes()); // end of chain

        if mismatched_strips {
            // 2 arbitrary in-bounds strip offsets, then 2 arbitrary byte counts.
            buf[ext_off..ext_off + 4].copy_from_slice(&1000u32.to_le_bytes());
            buf[ext_off + 4..ext_off + 8].copy_from_slice(&1004u32.to_le_bytes());
            buf[ext_off + 8..ext_off + 12].copy_from_slice(&2u32.to_le_bytes());
            buf[ext_off + 12..ext_off + 16].copy_from_slice(&2u32.to_le_bytes());
        }

        buf
    }

    #[test]
    fn mismatched_strip_count_on_later_page_is_rejected() {
        // Page 1 shares every OTHER geometry field with page 0 (the existing
        // check) but has 2 strips where page 0 has 1. Before the strip-count check this parses
        // successfully with a wrong strip layout for that page.
        let mismatched = build_minimal_lsm(true);
        assert!(
            matches!(parse_lsm(&mismatched), Err(Error::Decode(_))),
            "a later page with a different strip count than page 0 must be rejected"
        );

        // Control: an otherwise-identical, internally-consistent file (both
        // pages agree on strip count) must still parse fine, proving the
        // rejection above is about the strip-count mismatch specifically, not
        // a builder bug.
        let consistent = build_minimal_lsm(false);
        assert!(
            parse_lsm(&consistent).is_ok(),
            "a consistent 2-page file must still parse: {:?}",
            parse_lsm(&consistent).err()
        );
    }
}
