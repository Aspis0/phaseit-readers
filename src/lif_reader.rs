// SPDX-License-Identifier: Apache-2.0
//! Leica LIF reader.
//!
//! Ported from the behaviour of `liffile` (Christoph Gohlke, BSD-3-Clause,
//! pinned 2026.4.11) — the trusted reference decoder. Only the LIF container
//! *facts* and Gohlke's plane-offset algorithm are reproduced here; no code was
//! copied from the GPL-3.0 `readlif`. Keep this attribution if you touch the
//! framing or the stride math.
//!
//! A `.lif` is a flat sequence of **memory blocks**. The first block carries a
//! UTF-16LE **XML tree** describing every image (Leica calls each an "Element");
//! every later block carries a UTF-16 **MemoryBlockID** followed by that image's
//! raw pixel bytes. The XML gives, per image, the `DimensionDescription`s
//! (`DimID`, `NumberOfElements`, `BytesInc`, `Length`) and `ChannelDescription`s
//! (`Resolution`, `DataType`, `BytesInc`, `LUTName`); the `<Memory MemoryBlockID=…>`
//! child ties the image to its data block.
//!
//! Native axis order is **ZCYX** (Y,X innermost and contiguous — one frame is a
//! single contiguous `Y*X*itemsize` read). We map LIF `DimID` → canonical TCZYX:
//! `1=X, 2=Y, 3=Z, 4=T`; channels are the `ChannelDescription` list (NOT a
//! dimension), sorted by `BytesInc` ascending so channel `c` is the c-th block in
//! memory (exactly liffile's ordering). The byte offset of plane `(t,c,z)` inside
//! its memory block is
//!
//! ```text
//! offset = t*T.BytesInc + z*Z.BytesInc + channel[c].BytesInc
//! ```
//!
//! then `Y*X*itemsize` contiguous bytes are read there. This was validated
//! bit-exact against liffile on the reference samples (see `tests/lif_real.rs`).
//!
//! Because the frame is contiguous, a SUB-REGION of a plane can be read
//! without decoding the whole plane: `region()` reads only the rows the
//! rectangle covers (one read per row, or one read for a full-width
//! rectangle, whose rows are contiguous). The result is byte-identical to
//! cropping the whole plane.
//!
//! Color convention: channels are the `ChannelDescription` list, i.e. color maps
//! onto `dims.c` and `SceneMeta::samples` is always `1`. Interleaved
//! multi-sample planes (`samples > 1`) are a CZI-only convention; see
//! [`SceneMeta::samples`](crate::types::SceneMeta::samples).
//!
//! Fail-closed, per image: an image whose data block is absent from the `.lif`
//! (its pixels live in the `.lifext` companion, e.g. `*_pmd_0`), whose
//! `DimID` is outside `{1,2,3,4}`, whose channels are interleaved/stride-padded
//! (so a frame is not contiguous), whose dtype we do not model, whose numeric
//! attributes are missing or malformed, whose dimension list is ambiguous (a
//! duplicate `DimID`, a modelled axis with no elements), whose declared size
//! overflows the reader's byte arithmetic, or whose data block does not hold
//! its whole frame, is **refused** rather than decoded wrongly: the reason is
//! named and the image keeps its place in the file's own list of images, so
//! the images after it are not renumbered. The open fails only when no image
//! of the file can be read at all — and then the error names every refusal
//! with its own reason.
//!
//! An image whose plane is over the decode ceiling
//! ([`MAX_DECODED_PLANE_BYTES`](crate::guard::MAX_DECODED_PLANE_BYTES)) is
//! NOT refused: the ceiling guards ONE materialisation, not the image's
//! existence. The image is published, `plane()` refuses the materialisation
//! with [`Error::OversizedAllocation`], and `region()` serves windows of it —
//! exactly as the CZI reader does for a whole-slide scene.

use std::collections::HashMap;
use std::path::Path;

use memmap2::Mmap;
use quick_xml::events::Event;
use quick_xml::reader::Reader;

use crate::calibration::Calibration;
use crate::error::{Error, Result};
use crate::io::map_file;
use crate::types::{
    checked_region, empty_region_plane, ChannelMeta, DType, Dims5, Endian, ImageMeta, ImageReader,
    PixelRect, Plane, PlaneBuf, RefusedScene, SceneMeta,
};

/// LIF `DimID` values we map to canonical axes. Anything else (emission λ,
/// mosaic, rotation, loop, …) is not modelled and makes an image be skipped.
const DIM_X: u32 = 1;
const DIM_Y: u32 = 2;
const DIM_Z: u32 = 3;
const DIM_T: u32 = 4;

/// Upper bound on the UTF-16 XML header (real LIF headers are well under 1 MiB;
/// the reference samples are ~0.45 MiB). Refuses an absurd declared length before
/// decoding so a corrupt header cannot force a huge allocation.
const MAX_XML_BYTES: usize = 256 * 1024 * 1024;

/// Upper bound on XML nesting depth (guards a crafted deeply-nested document).
const MAX_XML_DEPTH: usize = 256;

/// Upper bound on memory blocks we will walk (guards a corrupt/cyclic file; the
/// walk also terminates naturally at EOF or the first malformed block header).
/// Real LIF files have tens to low-hundreds of blocks; this leaves ~4 orders of
/// magnitude of headroom while still tripping long before a HashMap with
/// millions of bogus entries could be built from a corrupt/cyclic file.
const MAX_MEMORY_BLOCKS: usize = 1_000_000;

/// Per-scene byte layout, cached at `open()` so `plane()` is O(1).
struct SceneLayout {
    dims: Dims5,
    dtype: DType,
    /// Absolute file offset of the owning memory block's data.
    block_offset: usize,
    /// Absolute file offset one past the end of the owning memory block's
    /// data. Computed once at `open()`, so a row read does not recompute it.
    block_end: usize,
    /// Stride in bytes for one step along T (0 when there is no T dimension).
    t_bytes_inc: usize,
    /// Stride in bytes for one step along Z (0 when there is no Z dimension).
    z_bytes_inc: usize,
    /// Byte offset of each channel within a `(t,z)` slab, indexed by canonical
    /// channel `c`. Sorted ascending (liffile's channel order); `len == dims.c`.
    channel_bytes_inc: Vec<usize>,
    /// Byte stride of one image row (`dims.x * itemsize`). The reader refuses
    /// an image whose X or Y dimension is not contiguous, so a frame's rows
    /// are packed: this is the offset of row `y` inside the frame.
    row_stride: usize,
}

impl SceneLayout {
    /// Byte offset of the frame `(t, c, z)` inside the owning memory block:
    /// `t*T.BytesInc + z*Z.BytesInc + channel[c].BytesInc` (all checked).
    ///
    /// The caller has already validated `t`/`c`/`z` against `dims`, so the
    /// channel index is in range.
    fn plane_offset(&self, t: usize, c: usize, z: usize) -> Result<usize> {
        t.checked_mul(self.t_bytes_inc)
            .and_then(|v| v.checked_add(z.checked_mul(self.z_bytes_inc)?))
            .and_then(|v| v.checked_add(self.channel_bytes_inc[c]))
            .ok_or_else(|| Error::Decode("LIF plane offset overflow".to_string()))
    }
}

/// Leica LIF reader. `Mmap` is `Send + Sync` and every other field is plain
/// data, so the reader is `Send + Sync` and shareable across threads.
pub struct LifReader {
    mmap: Mmap,
    meta: ImageMeta,
    /// Parallel to `meta.scenes`: `layouts[s]` describes `meta.scenes[s]`.
    layouts: Vec<SceneLayout>,
    /// The images of the file this reader refused, in file order, each with its
    /// reason. The readable images are in `meta`, in file order.
    refused: Vec<RefusedScene>,
    /// Parallel to `meta.scenes`: the position of each readable scene in the
    /// file's own list of images.
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

        // --- parse the metadata XML into a small DOM ---
        let root = parse_dom(&xml)?;
        let version = xml_version(&root)?;

        // --- walk the memory-block chain, mapping BlockID -> (offset, size) ---
        let blocks = walk_memory_blocks(buf, xml_end, version)?;

        // The pixels of an ext-resident image live in the `.lifext` companion next
        // to the `.lif`. The refusal says "ext-resident" only when that
        // companion is there: a `.lif` whose block is simply missing is not
        // ext-resident, it is short.
        let has_companion = path.with_extension("lifext").exists();

        // WHY the companion is not read, and why its images are not listed:
        // a `.lifext` holds the CHILDREN of the `.lif`'s own images - their
        // histograms (`*_histo`) and their reduced pyramid levels (`*_pmd_N`).
        // Those are auxiliary data OF the images the `.lif` publishes, not
        // images a user would open, so they are deliberately neither published
        // as scenes nor listed as refused. Bio-Formats does the same: it
        // opens the `.lif` and publishes the `.lif`'s images only, and does
        // not publish the companion's. Listing them as refused would flood
        // the desktop's scene chooser with rows that are not scenes - twelve
        // for four-scene.lif, two hundred and nine over the corpus.
        //
        // Later on, the `*_pmd_N` levels could speed up the desktop pyramid:
        // they are reduced copies of the very images the `.lif` publishes, so
        // the small level could be read first instead of the full one. No
        // code for that is in this reader yet.

        // --- build one scene per readable image ---
        let mut scenes: Vec<SceneMeta> = Vec::new();
        let mut layouts: Vec<SceneLayout> = Vec::new();
        let mut file_indices: Vec<usize> = Vec::new();
        let mut refused: Vec<RefusedScene> = Vec::new();
        let mut images: Vec<&XmlNode> = Vec::new();
        collect_images(&root, &mut images);

        // The FILE's own list of images: every image element of the XML, in
        // document order. An image we cannot decode still holds its place in
        // that list, so it is refused by name instead of being dropped (a drop
        // would shift every image after it down by one).
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

        // Fail closed only when no image of the file can be read at all; the
        // error then names EVERY refusal with its own reason, so the cause of
        // the file's failure is not guessed.
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

    /// The images of the file this reader refused, each with its reason.
    fn refused_scenes(&self) -> &[RefusedScene] {
        &self.refused
    }

    /// The position of `scene` in the file's own list of images. It differs
    /// from `scene` when an earlier image of the file was refused.
    fn scene_file_index(&self, scene: usize) -> usize {
        self.file_indices.get(scene).copied().unwrap_or(scene)
    }

    /// Decode the whole plane of `(scene, t, c, z)`.
    ///
    /// A plane whose byte size is over the decode ceiling is REFUSED here
    /// with [`Error::OversizedAllocation`]: the ceiling guards what is
    /// materialised. The image itself stays published — [`region`](Self::region)
    /// serves windows of it.
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

        // Size the OUTPUT plane BEFORE reserving/reading anything. This is
        // where the decode ceiling applies: a plane over it is refused, while
        // the image stays readable through `region()`.
        let frame_bytes = d.checked_plane_bytes(layout.dtype)?;

        // offset = t*T.BytesInc + z*Z.BytesInc + channel[c].BytesInc (all checked).
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

        // Raw pixel bytes are little-endian; `from_bytes` byteswaps to native.
        let raw = &self.mmap[start..end];
        let data = PlaneBuf::from_bytes(layout.dtype, Endian::Little, raw)?;
        Plane::new(data, [d.y, d.x])
    }

    /// LIF stores its frames CONTIGUOUSLY (the reader refuses an image whose X
    /// or Y dimension is not contiguous), so the rows of a sub-rectangle sit
    /// at computable offsets inside the image's memory block and only those
    /// rows are read — the whole plane is never decoded.
    fn supports_region(&self) -> bool {
        true
    }

    /// Read only the rows of `rect` out of the image's memory block.
    ///
    /// The result is byte-identical to `self.plane(scene, t, c, z)?.crop(rect)`:
    /// the same little-endian bytes are converted to native endianness once,
    /// over the rect alone. The output buffer is sized by the RECT, so the
    /// decode ceiling applies to what is materialised, never to the full
    /// plane — an image whose plane is over the ceiling is still readable
    /// here, one window at a time.
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
        let layout = self.layouts.get(scene).ok_or(Error::SceneOutOfRange {
            scene,
            count: self.layouts.len(),
        })?;
        let d = layout.dims;
        checked_region("LIF region", &d, scene, t, c, z, rect)?;

        // An empty rectangle covers no pixels: the answer is an empty plane,
        // the same one the trait's default returns.
        if rect.is_empty() {
            return empty_region_plane(layout.dtype, 1, rect);
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
        let out_bytes = rect_dims.checked_plane_bytes(layout.dtype)?;

        let itemsize = layout.dtype.size_bytes();
        let row_bytes = rect
            .w
            .checked_mul(itemsize)
            .ok_or_else(|| Error::Decode("LIF region row size overflow".to_string()))?;
        let base = layout.plane_offset(t, c, z)?;

        // Gather the rect's rows. A FULL-WIDTH rect is a contiguous span of
        // rows (the frame is packed), so it is one read; a narrower rect is
        // one read per row. `rect.x == 0` for a full-width rect (the rect
        // check above leaves no room for a column offset), which is why the
        // `rect.y` term below is the whole row offset.
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

        // Raw pixel bytes are little-endian; `from_bytes` byteswaps to native.
        let data = PlaneBuf::from_bytes(layout.dtype, Endian::Little, &raw)?;
        Plane::new(data, [rect.h, rect.w])
    }
}

impl LifReader {
    /// The `len` bytes of the image's memory block at `offset` (relative to
    /// the block's start). Refuses a span that runs past the block or the
    /// file — the same bound `plane()` enforces for a whole frame.
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

/// What one image Element of the LIF XML turned out to be.
enum ImageBuild {
    /// A decodable pixel image.
    Scene(SceneMeta, SceneLayout),
    /// An image we cannot decode, with the reason named. The image still holds
    /// its place in the file's list of images.
    Refused { name: String, reason: String },
}

/// Build a [`SceneMeta`] + [`SceneLayout`] for one image Element, or REFUSE the
/// image with the reason named (ext-resident data block, unsupported
/// `DimID`/dtype, a non-contiguous frame layout, a malformed numeric
/// attribute, an ambiguous dimension list, a size the reader's byte
/// arithmetic cannot hold, or a data block that does not hold the frame).
///
/// EVERY way an image can fail to decode refuses THIS image and no other:
/// the open fails only when no image of the file can be read at all.
fn build_scene(
    el: &XmlNode,
    blocks: &HashMap<String, (usize, usize)>,
    file_len: usize,
    has_companion: bool,
) -> ImageBuild {
    let name = el.attr("Name").unwrap_or("image").to_string();
    let refuse = |reason: String| ImageBuild::Refused {
        name: name.clone(),
        reason,
    };

    let image_desc = match el
        .child("Data")
        .and_then(|d| d.child("Image"))
        .and_then(|i| i.child("ImageDescription"))
    {
        Some(desc) => desc,
        None => return refuse("the element carries no image description".to_string()),
    };

    // Every numeric attribute of this image is read through `num`: a missing
    // or malformed one makes THIS image undecodable, so the image is refused
    // by name instead of failing the whole open.
    macro_rules! num {
        ($node:expr, $key:literal, $ty:ty) => {
            match attr_u64($node, $key) {
                Ok(v) => match <$ty>::try_from(v) {
                    Ok(v) => v,
                    Err(_) => {
                        return refuse(format!(
                            "the image states a {} that does not fit the reader",
                            $key
                        ))
                    }
                },
                Err(reason) => return refuse(reason),
            }
        };
    }

    // --- dimensions ---
    let dims_node = match image_desc.child("Dimensions") {
        Some(n) => n,
        None => return refuse("the image states no Dimensions".to_string()),
    };
    let mut x: Option<ParsedDim> = None;
    let mut y: Option<ParsedDim> = None;
    let mut z: Option<ParsedDim> = None;
    let mut t: Option<ParsedDim> = None;
    for dd in dims_node.children_named("DimensionDescription") {
        let dim = ParsedDim {
            id: num!(dd, "DimID", u32),
            n: num!(dd, "NumberOfElements", usize),
            bytes_inc: num!(dd, "BytesInc", usize),
            length: attr_f64(dd, "Length").unwrap_or(0.0),
        };
        // Classify the DimID BEFORE enforcing the "0 elements" guard: an
        // unmodelled axis (emission λ / mosaic / rotation / …) declared with
        // NumberOfElements="0" must REFUSE this image, not abort the whole file.
        let slot = match dim.id {
            DIM_X => &mut x,
            DIM_Y => &mut y,
            DIM_Z => &mut z,
            DIM_T => &mut t,
            other => {
                return refuse(format!(
                    "the image has a dimension (DimID {other}) we do not model"
                ));
            }
        };
        // Only a modelled axis (X/Y/Z/T) is required to carry elements.
        if dim.n == 0 {
            return refuse(format!(
                "the image has a dimension (DimID {}) with 0 elements",
                dim.id
            ));
        }
        if slot.is_some() {
            return refuse(format!(
                "the image has a duplicate dimension (DimID {})",
                dim.id
            ));
        }
        *slot = Some(dim);
    }
    // Y and X are mandatory (a 2-D plane needs both).
    let (x, y) = match (x, y) {
        (Some(x), Some(y)) => (x, y),
        _ => return refuse("the image states no X and Y dimension".to_string()),
    };

    // --- channels (sorted by BytesInc ascending == canonical channel order) ---
    let channels_node = match image_desc.child("Channels") {
        Some(n) => n,
        None => return refuse("the image states no Channels".to_string()),
    };
    let mut chans: Vec<ParsedChannel> = Vec::new();
    for cd in channels_node.children_named("ChannelDescription") {
        chans.push(ParsedChannel {
            data_type: num!(cd, "DataType", u32),
            resolution: num!(cd, "Resolution", u32),
            bytes_inc: num!(cd, "BytesInc", usize),
            lut_name: cd.attr("LUTName").unwrap_or("").to_string(),
        });
    }
    if chans.is_empty() {
        return refuse("the image states no channel".to_string());
    }
    chans.sort_by_key(|c| c.bytes_inc);

    // All channels must share one dtype (heterogeneous channels aren't modelled).
    // An unmodelled (DataType, Resolution) pair means this image is REFUSED, not
    // a whole-file abort (matches the module doc's fail-closed-per-image contract).
    let dtype = match dtype_from_channel(chans[0].data_type, chans[0].resolution) {
        Ok(d) => d,
        Err(_) => {
            return refuse(
                "the image has a channel of an unsupported pixel type".to_string(),
            )
        }
    };
    for c in &chans[1..] {
        match dtype_from_channel(c.data_type, c.resolution) {
            Ok(d) if d == dtype => {}
            _ => {
                return refuse(
                    "the channels of the image do not share one pixel type".to_string(),
                )
            }
        }
    }
    let itemsize = dtype.size_bytes();

    // --- frame must be contiguous: X inner (BytesInc == itemsize),
    //     Y next (BytesInc == itemsize * X.n). Otherwise (interleaved RGB or
    //     stride-padded rows) a single Y*X read would be wrong → refuse. ---
    if x.bytes_inc != itemsize {
        return refuse(
            "the X dimension of the image is not contiguous".to_string(),
        );
    }
    let row_stride = match itemsize.checked_mul(x.n) {
        Some(v) => v,
        None => {
            return refuse(
                "the image states a row stride that overflows the reader's byte arithmetic"
                    .to_string(),
            )
        }
    };
    if y.bytes_inc != row_stride {
        return refuse(
            "the Y dimension of the image is not contiguous".to_string(),
        );
    }

    let dims = Dims5 {
        t: t.as_ref().map(|d| d.n).unwrap_or(1),
        c: chans.len(),
        z: z.as_ref().map(|d| d.n).unwrap_or(1),
        y: y.n,
        x: x.n,
    };
    // Size the frame WITHOUT the decode ceiling: the ceiling guards ONE
    // materialisation, not the image's existence. An image whose plane is too
    // big to hold in RAM is still PUBLISHED — `plane()` refuses the
    // materialisation and `region()` serves windows of it — exactly as the
    // CZI reader does for a whole-slide scene. Only an image whose size
    // OVERFLOWS the reader's byte arithmetic is refused here, and it refuses
    // THIS image, not the whole file.
    let frame_bytes = match dims.plane_bytes_no_ceiling(dtype) {
        Ok(v) => v,
        Err(Error::OversizedAllocation { requested, cap }) => {
            return refuse(format!(
                "the image states a plane of {requested} bytes, which overflows the reader's \
                 byte arithmetic (its cap is {cap} bytes)"
            ))
        }
        Err(e) => return refuse(format!("the image states a plane the reader cannot size: {e}")),
    };

    let t_bytes_inc = t.as_ref().map(|d| d.bytes_inc).unwrap_or(0);
    let z_bytes_inc = z.as_ref().map(|d| d.bytes_inc).unwrap_or(0);
    let channel_bytes_inc: Vec<usize> = chans.iter().map(|c| c.bytes_inc).collect();

    // --- Stride-alias guard: reject any stride that lets two planes alias the same
    //     bytes. `chans` is sorted ascending by BytesInc (channel order), so
    //     consecutive channels must be strictly increasing by at least one
    //     whole frame; Z/T strides must each be at least as large as the
    //     extent they step over. The `max_offset` bound below only proves the
    //     WORST-CASE plane fits in the block -- it never proves consecutive
    //     planes don't overlap. ---
    for w in chans.windows(2) {
        if w[1].bytes_inc < w[0].bytes_inc.saturating_add(frame_bytes) {
            return refuse(
                "the channels of the image overlap in memory".to_string(),
            );
        }
    }
    if z.is_some() && dims.z > 1 && z_bytes_inc < frame_bytes {
        return refuse(
            "the Z dimension of the image overlaps in memory".to_string(),
        );
    }
    // Effective size of one T-step's worth of data: `dims.z * z_bytes_inc` when
    // a Z axis is present, or one frame when there is none (T then steps
    // directly over frames).
    let z_span = if z.is_some() {
        match dims.z.checked_mul(z_bytes_inc) {
            Some(v) => v,
            None => {
                return refuse(
                    "the image states a Z span that overflows the reader's byte arithmetic"
                        .to_string(),
                )
            }
        }
    } else {
        frame_bytes
    };
    if t.is_some() && dims.t > 1 && t_bytes_inc < z_span {
        return refuse(
            "the T dimension of the image overlaps in memory".to_string(),
        );
    }

    // --- resolve the owning memory block (absent => ext-resident => refuse) ---
    let mbid = match el.child("Memory").and_then(|m| m.attr("MemoryBlockID")) {
        Some(id) => id,
        None => return refuse("the image names no memory block".to_string()),
    };
    let (block_offset, block_size) = match blocks.get(mbid) {
        Some(&(off, size)) if size > 0 => (off, size),
        // Present with size 0, or absent: the pixels are not in this .lif.
        // They are ext-resident only when the `.lifext` companion is there.
        _ => {
            let where_ = if has_companion {
                format!("its data block {mbid:?} is ext-resident")
            } else {
                format!("its data block {mbid:?} is absent from this .lif")
            };
            return refuse(format!(
                "the pixels of the image are not in this file ({where_})"
            ))
        }
    };

    // Validate that the worst-case plane fits inside the block (and the file):
    // this catches a truncated / mismatched block before any plane() call.
    let max_offset = match (dims.t - 1)
        .checked_mul(t_bytes_inc)
        .and_then(|v| v.checked_add((dims.z - 1).checked_mul(z_bytes_inc)?))
        .and_then(|v| v.checked_add(*channel_bytes_inc.last().unwrap()))
        .and_then(|v| v.checked_add(frame_bytes))
    {
        Some(v) => v,
        None => {
            return refuse(
                "the image states a plane offset that overflows the reader's byte arithmetic"
                    .to_string(),
            )
        }
    };
    let block_end = match block_offset.checked_add(block_size) {
        Some(v) => v,
        None => {
            return refuse(
                "the image states a data block that ends past the reader's byte arithmetic"
                    .to_string(),
            )
        }
    };
    if block_offset.checked_add(max_offset).is_none()
        || block_offset + max_offset > block_end
        || block_end > file_len
    {
        return refuse(format!(
            "the computed data span exceeds its memory block {mbid:?}"
        ));
    }

    // --- best-effort metadata (never gates decoding) ---
    let channels: Vec<ChannelMeta> = chans
        .iter()
        .map(|c| ChannelMeta {
            name: if c.lut_name.is_empty() {
                None
            } else {
                Some(c.lut_name.clone())
            },
            color: lut_color(&c.lut_name),
        })
        .collect();

    let calibration = calibration(&x, &y, z.as_ref());

    let scene = SceneMeta {
        dims,
        dtype,
        samples: 1, // LIF planes are grayscale (one sample per pixel).
        px_size_um: calibration.px_size_um,
        notes: calibration.notes,
        channels,
        palette: None,
        name,
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
    ImageBuild::Scene(scene, layout)
}

/// A `DimensionDescription` we keep.
struct ParsedDim {
    id: u32,
    n: usize,
    bytes_inc: usize,
    length: f64,
}

/// A `ChannelDescription` we keep.
struct ParsedChannel {
    data_type: u32,
    resolution: u32,
    bytes_inc: usize,
    lut_name: String,
}

/// Map a LIF `(DataType, Resolution)` to our [`DType`]. LIF is unsigned-int
/// (DataType 0) or float (DataType 1); it never stores signed ints. Anything
/// else is refused so an image with it is skipped rather than mis-decoded.
fn dtype_from_channel(data_type: u32, resolution: u32) -> Result<DType> {
    match (data_type, resolution) {
        (0, r) if (1..=8).contains(&r) => Ok(DType::U8),
        (0, r) if (9..=16).contains(&r) => Ok(DType::U16),
        (1, 32) => Ok(DType::F32),
        _ => Err(Error::Unsupported(format!(
            "LIF channel (DataType={data_type}, Resolution={resolution}) not modelled"
        ))),
    }
}

/// Best-effort per-axis pixel spacing in micrometres, mirroring liffile's
/// `coords` (linspace over `Length` metres): `spacing = Length / (N-1)`.
/// A pixel size exists only when X and Y are both derivable; the Z step is
/// resolved by the shared rule, so a file that states no Z step (or states a
/// placeholder) publishes `None` for it, with a note when the file stated a
/// placeholder.
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

/// Map a Leica LUT name to a display RGB, best-effort (metadata only).
fn lut_color(name: &str) -> Option<[u8; 3]> {
    match name.to_ascii_lowercase().as_str() {
        "red" => Some([255, 0, 0]),
        "green" => Some([0, 255, 0]),
        "blue" => Some([0, 0, 255]),
        "cyan" => Some([0, 255, 255]),
        "magenta" => Some([255, 0, 255]),
        "yellow" => Some([255, 255, 0]),
        "gray" | "grey" => Some([255, 255, 255]),
        _ => None,
    }
}

/// Walk the memory-block chain starting at `start`, returning `BlockID ->
/// (data_offset, data_size)`. Terminates at EOF or the first malformed block
/// header (matching liffile, which simply stops adding blocks on a short read).
fn walk_memory_blocks(
    buf: &[u8],
    start: usize,
    version: u32,
) -> Result<HashMap<String, (usize, usize)>> {
    // v2 (and LIFEXT) uses a u64 block size; v1 uses u32.
    let size_is_u64 = match version {
        2 => true,
        1 => false,
        other => {
            return Err(Error::Unsupported(format!(
                "LIF container version {other} not supported (expected 1 or 2)"
            )))
        }
    };
    // Fixed header: id0 u32 | descSize u32 | id1 u8 | size (u64/u32) | id2 u8 | strlen u32.
    let size_field = if size_is_u64 { 8 } else { 4 };
    let fixed = 4 + 4 + 1 + size_field + 1 + 4;

    let mut blocks: HashMap<String, (usize, usize)> = HashMap::new();
    let mut pos = start;
    let mut count = 0usize;

    while pos + fixed <= buf.len() {
        count += 1;
        if count > MAX_MEMORY_BLOCKS {
            return Err(Error::Parse(
                "LIF memory-block chain exceeds sane bound (corrupt?)".to_string(),
            ));
        }
        // id0 must be 0x70, the two markers 0x2A; a mismatch means we've run past
        // the last real block (trailing padding / EOF-ish) — stop cleanly.
        if read_u32(buf, pos)? != 0x70 || buf[pos + 8] != 0x2A {
            break;
        }
        let size1 = if size_is_u64 {
            usize::try_from(read_u64(buf, pos + 9)?)
                .map_err(|_| Error::Decode("LIF block size overflows usize".to_string()))?
        } else {
            read_u32(buf, pos + 9)? as usize
        };
        let marker2_off = pos + 9 + size_field;
        if buf[marker2_off] != 0x2A {
            break;
        }
        let strlen = read_u32(buf, marker2_off + 1)? as usize;
        let id_off = marker2_off + 5;
        let id_end = match id_off
            .checked_add(strlen.saturating_mul(2))
            .filter(|&e| e <= buf.len())
        {
            Some(e) => e,
            None => break,
        };
        let id = decode_utf16le(&buf[id_off..id_end])?;

        let data_off = id_end;
        let data_end = match data_off.checked_add(size1).filter(|&e| e <= buf.len()) {
            Some(e) => e,
            None => break, // declared data runs past EOF: stop, don't fabricate
        };
        // Keep the FIRST occurrence of an id (matches liffile's dict insertion
        // for well-formed files where ids are unique).
        blocks.entry(id).or_insert((data_off, size1));
        pos = data_end;
    }

    Ok(blocks)
}

// --- tiny XML DOM (quick-xml is a pull parser; a minimal tree keeps the image
//     walk readable and testable) ----------------------------------------------

/// A minimal XML element: tag, attributes, child elements. Text is ignored (LIF
/// metadata is entirely attribute-based for what we consume).
struct XmlNode {
    tag: String,
    attrs: Vec<(String, String)>,
    children: Vec<XmlNode>,
}

impl XmlNode {
    fn new(tag: impl Into<String>) -> Self {
        XmlNode {
            tag: tag.into(),
            attrs: Vec::new(),
            children: Vec::new(),
        }
    }

    /// First attribute value for `key`, if present.
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// First direct child element named `tag`, if any.
    fn child(&self, tag: &str) -> Option<&XmlNode> {
        self.children.iter().find(|c| c.tag == tag)
    }

    /// All direct child elements named `tag`, in document order.
    fn children_named<'a>(&'a self, tag: &'a str) -> impl Iterator<Item = &'a XmlNode> {
        self.children.iter().filter(move |c| c.tag == tag)
    }
}

/// Parse the LIF metadata XML into a DOM rooted at a synthetic node whose
/// children are the document's top-level elements.
fn parse_dom(xml: &str) -> Result<XmlNode> {
    let mut reader = Reader::from_str(xml);
    // Reader was built from `&str`, so its decoder is UTF-8; use it to unescape
    // attribute values (entities like `&amp;`) without whitespace normalization.
    let decoder = reader.decoder();
    let mut stack: Vec<XmlNode> = vec![XmlNode::new("")]; // synthetic root

    let node_from = |e: &quick_xml::events::BytesStart| -> Result<XmlNode> {
        let tag = std::str::from_utf8(e.name().as_ref())
            .map_err(|_| Error::Parse("LIF XML: non-UTF8 tag name".to_string()))?
            .to_string();
        let mut node = XmlNode::new(tag);
        for a in e.attributes() {
            let a = a.map_err(|err| Error::Parse(format!("LIF XML attribute: {err}")))?;
            let key = std::str::from_utf8(a.key.as_ref())
                .map_err(|_| Error::Parse("LIF XML: non-UTF8 attribute key".to_string()))?
                .to_string();
            // Normalization only maps tab/CR/LF to space (LIF attribute values
            // are whitespace-free names/numbers/ids), while resolving entities.
            let val = a
                .decoded_and_normalized_value(quick_xml::XmlVersion::Implicit1_0, decoder)
                .map_err(|err| Error::Parse(format!("LIF XML attribute value: {err}")))?
                .into_owned();
            node.attrs.push((key, val));
        }
        Ok(node)
    };

    loop {
        match reader
            .read_event()
            .map_err(|e| Error::Parse(format!("LIF XML parse: {e}")))?
        {
            Event::Start(e) => {
                if stack.len() >= MAX_XML_DEPTH {
                    return Err(Error::Parse("LIF XML nesting too deep".to_string()));
                }
                stack.push(node_from(&e)?);
            }
            Event::Empty(e) => {
                let node = node_from(&e)?;
                // A self-closing element is a leaf of the current top node.
                stack
                    .last_mut()
                    .expect("synthetic root always present")
                    .children
                    .push(node);
            }
            Event::End(_) => {
                // Never pop the synthetic root: more ends than starts = malformed.
                if stack.len() <= 1 {
                    return Err(Error::Parse("LIF XML: unbalanced end tag".to_string()));
                }
                let node = stack.pop().expect("checked len > 1");
                stack
                    .last_mut()
                    .expect("synthetic root always present")
                    .children
                    .push(node);
            }
            Event::Eof => break,
            _ => {} // Text / CDATA / Decl / Comment / PI: irrelevant here
        }
    }

    if stack.len() != 1 {
        return Err(Error::Parse("LIF XML: unclosed element(s)".to_string()));
    }
    Ok(stack.pop().expect("synthetic root"))
}

/// Read the container `Version` from the XML root (`<LMSDataContainerHeader
/// Version="2">`). Required: the memory-block framing depends on it.
fn xml_version(root: &XmlNode) -> Result<u32> {
    // The synthetic root's single child is the real document root.
    let header = root
        .children
        .iter()
        .find(|c| c.tag.starts_with("LMSDataContainer"))
        .ok_or_else(|| Error::Parse("LIF XML: LMSDataContainerHeader not found".to_string()))?;
    let v = header
        .attr("Version")
        .ok_or_else(|| Error::Parse("LIF XML: Version attribute not found".to_string()))?;
    v.trim()
        .parse::<u32>()
        .map_err(|_| Error::Parse(format!("LIF XML: bad Version {v:?}")))
}

/// Collect every image-bearing `Element` (has `Data/Image`) in pre-order,
/// exactly matching liffile's `_image_iter` traversal (parent before children).
fn collect_images<'a>(node: &'a XmlNode, out: &mut Vec<&'a XmlNode>) {
    for child in &node.children {
        if child.tag == "Element" {
            let is_image = child
                .child("Data")
                .and_then(|d| d.child("Image"))
                .is_some();
            if is_image {
                out.push(child);
            }
        }
        // Recurse through the whole tree (container Elements hold Children/Element).
        collect_images(child, out);
    }
}

// --- primitive readers + attribute helpers ----------------------------------

fn read_u32(buf: &[u8], off: usize) -> Result<u32> {
    let s = buf
        .get(off..off + 4)
        .ok_or_else(|| bounds(off, 4, buf.len()))?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn read_u64(buf: &[u8], off: usize) -> Result<u64> {
    let s = buf
        .get(off..off + 8)
        .ok_or_else(|| bounds(off, 8, buf.len()))?;
    Ok(u64::from_le_bytes([
        s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
    ]))
}

/// Decode a little-endian UTF-16 byte slice into an owned `String`.
// Plain modulo (not `usize::is_multiple_of`, Rust 1.87+) keeps the crate
// buildable on older stable toolchains, matching `types.rs`.
#[allow(clippy::manual_is_multiple_of)]
fn decode_utf16le(bytes: &[u8]) -> Result<String> {
    if bytes.len() % 2 != 0 {
        return Err(Error::Parse(
            "LIF UTF-16 string has an odd byte length".to_string(),
        ));
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16(&units).map_err(|_| Error::Parse("LIF: invalid UTF-16 in header".to_string()))
}

/// The value of a numeric attribute of an image element, or the reason the
/// image is refused because the attribute is missing or does not hold an
/// integer. The reason is already phrased for the refusal, so the caller
/// passes it on as is.
fn attr_u64(node: &XmlNode, key: &str) -> std::result::Result<u64, String> {
    let v = node
        .attr(key)
        .ok_or_else(|| format!("the image states no {key} attribute"))?;
    v.trim().parse::<u64>().map_err(|_| {
        format!("the image states a malformed {key} attribute ({v:?} is not an integer)")
    })
}

fn attr_f64(node: &XmlNode, key: &str) -> Option<f64> {
    node.attr(key).and_then(|v| v.trim().parse::<f64>().ok())
}

fn bounds(off: usize, need: usize, len: usize) -> Error {
    Error::Parse(format!(
        "LIF read out of bounds: need {need} bytes at offset {off}, file is {len} bytes"
    ))
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
    fn utf16le_roundtrip_and_odd_length() {
        let s = "MB0";
        let bytes: Vec<u8> = s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(decode_utf16le(&bytes).unwrap(), "MB0");
        assert!(matches!(decode_utf16le(&[0x00]), Err(Error::Parse(_))));
    }

    #[test]
    fn dtype_mapping() {
        assert_eq!(dtype_from_channel(0, 8).unwrap(), DType::U8);
        assert_eq!(dtype_from_channel(0, 1).unwrap(), DType::U8);
        assert_eq!(dtype_from_channel(0, 16).unwrap(), DType::U16);
        assert_eq!(dtype_from_channel(0, 12).unwrap(), DType::U16);
        assert_eq!(dtype_from_channel(1, 32).unwrap(), DType::F32);
        assert!(dtype_from_channel(0, 32).is_err()); // u32 not modelled
        assert!(dtype_from_channel(2, 8).is_err()); // bad data type
    }

    #[test]
    fn lut_colors() {
        assert_eq!(lut_color("Green"), Some([0, 255, 0]));
        assert_eq!(lut_color("RED"), Some([255, 0, 0]));
        assert_eq!(lut_color("Gray"), Some([255, 255, 255]));
        assert_eq!(lut_color("Rhodamine"), None);
    }

    #[test]
    fn dom_parse_and_navigate() {
        let xml = r#"<Root a="1"><Element Name="X"><Data><Image/></Data>
            <Memory MemoryBlockID="MB0" Size="16"/></Element></Root>"#;
        let root = parse_dom(xml).unwrap();
        let mut images = Vec::new();
        collect_images(&root, &mut images);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].attr("Name"), Some("X"));
        assert_eq!(
            images[0].child("Memory").unwrap().attr("MemoryBlockID"),
            Some("MB0")
        );
    }

    /// Build a minimal but structurally-real LIF in memory and check the whole
    /// pipeline: header + XML + one memory block, planar Z=2 / C=2 / 2×2 u8.
    /// Data byte value at (z,c,y,x) is a unique tag so mapping errors show up.
    fn synthetic_lif() -> Vec<u8> {
        // Layout: X.BytesInc=1, Y.BytesInc=2, channel0=0, channel1=4, Z.BytesInc=8.
        // Block data = [0,1,2,3, 4,5,6,7, 8,9,10,11, 12,13,14,15] (16 bytes).
        let xml = concat!(
            r#"<LMSDataContainerHeader Version="2"><Element Name="Root"><Children>"#,
            r#"<Element Name="Img"><Data><Image><ImageDescription><Dimensions>"#,
            r#"<DimensionDescription DimID="1" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="1" BitInc="0"/>"#,
            r#"<DimensionDescription DimID="2" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="2" BitInc="0"/>"#,
            r#"<DimensionDescription DimID="3" NumberOfElements="2" Origin="0" Length="2e-6" Unit="m" BytesInc="8" BitInc="0"/>"#,
            r#"</Dimensions><Channels>"#,
            r#"<ChannelDescription DataType="0" ChannelTag="0" Resolution="8" NameOfMeasuredQuantity="" Min="0" Max="255" Unit="" LUTName="Green" IsLUTInverted="0" BytesInc="0" BitInc="0"/>"#,
            r#"<ChannelDescription DataType="0" ChannelTag="0" Resolution="8" NameOfMeasuredQuantity="" Min="0" Max="255" Unit="" LUTName="Red" IsLUTInverted="0" BytesInc="4" BitInc="0"/>"#,
            r#"</Channels></ImageDescription></Image></Data>"#,
            r#"<Memory MemoryBlockID="MB0" Size="16"/></Element></Children></Element></LMSDataContainerHeader>"#,
        );
        wrap_lif_bytes(xml)
    }

    /// Wrap a full LIF metadata XML document into header + one "MB0" memory
    /// block (16 bytes: `0..16`). Shared by every synthetic-file test below so
    /// each test only needs to write the `<LMSDataContainerHeader>` XML.
    fn wrap_lif_bytes(xml: &str) -> Vec<u8> {
        let xml_utf16: Vec<u8> = xml.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let xml_chars = (xml_utf16.len() / 2) as u32;

        let mut f: Vec<u8> = Vec::new();
        // Header: id0=0x70, descSize(any), id1=0x2A, strlen.
        f.extend_from_slice(&0x70u32.to_le_bytes());
        f.extend_from_slice(&0u32.to_le_bytes());
        f.push(0x2A);
        f.extend_from_slice(&xml_chars.to_le_bytes());
        f.extend_from_slice(&xml_utf16);

        // One v2 memory block: id0=0x70, descSize, 0x2A, size(u64)=16, 0x2A, strlen, id, data.
        let id: Vec<u8> = "MB0".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        f.extend_from_slice(&0x70u32.to_le_bytes());
        f.extend_from_slice(&0u32.to_le_bytes());
        f.push(0x2A);
        f.extend_from_slice(&16u64.to_le_bytes());
        f.push(0x2A);
        f.extend_from_slice(&(("MB0".len()) as u32).to_le_bytes());
        f.extend_from_slice(&id);
        f.extend_from_slice(&(0u8..16).collect::<Vec<u8>>());
        f
    }

    /// Append one memory block (id0=0x70, descSize, 0x2A, size(u64), 0x2A,
    /// strlen, id, data) to `f`. Shared by `wrap_lif_bytes_two_blocks` below.
    fn append_lif_block(f: &mut Vec<u8>, id: &str, data: &[u8]) {
        let id_bytes: Vec<u8> = id.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        f.extend_from_slice(&0x70u32.to_le_bytes());
        f.extend_from_slice(&0u32.to_le_bytes());
        f.push(0x2A);
        f.extend_from_slice(&(data.len() as u64).to_le_bytes());
        f.push(0x2A);
        f.extend_from_slice(&(id.len() as u32).to_le_bytes());
        f.extend_from_slice(&id_bytes);
        f.extend_from_slice(data);
    }

    /// Like `wrap_lif_bytes`, but with TWO named memory blocks instead of one
    /// hardcoded "MB0". Needed whenever a test's "Bad" image must actually
    /// reach its channel/dimension validation logic rather than being
    /// trivially skipped for referencing a nonexistent `MemoryBlockID`
    /// (`wrap_lif_bytes` alone cannot exercise a second image's real-data path).
    fn wrap_lif_bytes_two_blocks(xml: &str, id0: &str, data0: &[u8], id1: &str, data1: &[u8]) -> Vec<u8> {
        let xml_utf16: Vec<u8> = xml.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let xml_chars = (xml_utf16.len() / 2) as u32;

        let mut f: Vec<u8> = Vec::new();
        f.extend_from_slice(&0x70u32.to_le_bytes());
        f.extend_from_slice(&0u32.to_le_bytes());
        f.push(0x2A);
        f.extend_from_slice(&xml_chars.to_le_bytes());
        f.extend_from_slice(&xml_utf16);

        append_lif_block(&mut f, id0, data0);
        append_lif_block(&mut f, id1, data1);
        f
    }

    fn open_synth(bytes: &[u8]) -> LifReader {
        let dir = std::env::temp_dir();
        // Unique per call (process id + thread id + a counter): cargo test runs
        // tests in parallel threads within the same process, so a filename keyed
        // only on process id is a race (one test's write/read/delete can stomp
        // another's, as this test module discovered once it grew past 2 tests).
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(
            "phasit_synth_{:?}_{}_{}.lif",
            std::thread::current().id(),
            std::process::id(),
            n
        ));
        std::fs::write(&path, bytes).unwrap();
        let r = LifReader::open(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        r
    }

    #[test]
    fn synthetic_geometry_and_plane_mapping() {
        let bytes = synthetic_lif();
        let reader = open_synth(&bytes);

        assert_eq!(reader.meta().scene_count(), 1);
        let scene = reader.meta().scene(0).unwrap();
        assert_eq!(scene.name, "Img");
        assert_eq!(scene.dims, Dims5 { t: 1, c: 2, z: 2, y: 2, x: 2 });
        assert_eq!(scene.dtype, DType::U8);
        assert_eq!(scene.channels.len(), 2);
        assert_eq!(scene.channels[0].name.as_deref(), Some("Green"));
        assert_eq!(scene.channels[1].color, Some([255, 0, 0]));

        // offset = z*8 + channel[c].BytesInc(c*4); read 4 contiguous bytes.
        let cases = [
            (0usize, 0usize, vec![0u8, 1, 2, 3]),
            (1, 0, vec![4, 5, 6, 7]),
            (0, 1, vec![8, 9, 10, 11]),
            (1, 1, vec![12, 13, 14, 15]),
        ];
        for (c, z, expect) in cases {
            let p = reader.plane(0, 0, c, z).unwrap();
            assert_eq!(p.shape, [2, 2]);
            match p.data {
                PlaneBuf::U8(v) => assert_eq!(v, expect, "plane c{c} z{z}"),
                other => panic!("expected U8, got {:?}", other.dtype()),
            }
        }
    }

    #[test]
    fn synthetic_out_of_range() {
        let reader = open_synth(&synthetic_lif());
        assert!(matches!(
            reader.plane(0, 0, 0, 2),
            Err(Error::OutOfRange { z: 2, .. })
        ));
        assert!(matches!(
            reader.plane(0, 0, 2, 0),
            Err(Error::OutOfRange { c: 2, .. })
        ));
        assert!(matches!(
            reader.plane(0, 1, 0, 0),
            Err(Error::OutOfRange { t: 1, .. })
        ));
        assert!(matches!(
            reader.plane(1, 0, 0, 0),
            Err(Error::SceneOutOfRange { scene: 1, count: 1 })
        ));
    }

    /// Regression test: an unmodelled channel dtype must REFUSE that
    /// image (matching the module doc's fail-closed-per-image contract), not
    /// abort the whole file. A file with one good image and one bad image must
    /// still open, exposing only the good scene.
    #[test]
    fn unmodeled_channel_dtype_refuses_image_but_opens_file() {
        let xml = concat!(
            r#"<LMSDataContainerHeader Version="2"><Element Name="Root"><Children>"#,
            // Good image: 1 channel U8, 2x2, contiguous -- decodes fine.
            r#"<Element Name="Good"><Data><Image><ImageDescription><Dimensions>"#,
            r#"<DimensionDescription DimID="1" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="1" BitInc="0"/>"#,
            r#"<DimensionDescription DimID="2" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="2" BitInc="0"/>"#,
            r#"</Dimensions><Channels>"#,
            r#"<ChannelDescription DataType="0" ChannelTag="0" Resolution="8" NameOfMeasuredQuantity="" Min="0" Max="255" Unit="" LUTName="Gray" IsLUTInverted="0" BytesInc="0" BitInc="0"/>"#,
            r#"</Channels></ImageDescription></Image></Data>"#,
            r#"<Memory MemoryBlockID="MB0" Size="16"/></Element>"#,
            // Bad image: DataType=0 Resolution=32 is unsigned 32-bit -- NOT
            // modelled (dtype_from_channel only models 1..=8 / 9..=16 / float32).
            r#"<Element Name="Bad"><Data><Image><ImageDescription><Dimensions>"#,
            r#"<DimensionDescription DimID="1" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="1" BitInc="0"/>"#,
            r#"<DimensionDescription DimID="2" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="2" BitInc="0"/>"#,
            r#"</Dimensions><Channels>"#,
            r#"<ChannelDescription DataType="0" ChannelTag="0" Resolution="32" NameOfMeasuredQuantity="" Min="0" Max="255" Unit="" LUTName="Gray" IsLUTInverted="0" BytesInc="0" BitInc="0"/>"#,
            r#"</Channels></ImageDescription></Image></Data>"#,
            r#"<Memory MemoryBlockID="MB1" Size="16"/></Element>"#,
            r#"</Children></Element></LMSDataContainerHeader>"#,
        );
        // Two REAL blocks (MB0 for "Good", MB1 for "Bad"): the bad image now has
        // a present data block, so it can only be skipped by the unmodelled-dtype
        // logic -- NOT by a missing `MemoryBlockID` (which is what made this test
        // vacuous before). Revert the dtype-skip in `build_scene` and this fails.
        let bytes = wrap_lif_bytes_two_blocks(
            xml,
            "MB0",
            &(0u8..16).collect::<Vec<u8>>(),
            "MB1",
            &(0u8..16).collect::<Vec<u8>>(),
        );
        let reader = open_synth(&bytes);
        assert_eq!(
            reader.meta().scene_count(),
            1,
            "the unmodelled-dtype image must be REFUSED, not abort the whole file"
        );
        assert_eq!(reader.meta().scene(0).unwrap().name, "Good");
    }

    /// RED test for the channel-alias check: two `ChannelDescription`s with EQUAL `BytesInc`
    /// would alias onto the same bytes (silently identical/overlapping
    /// planes). The image must be REFUSED, matching the module's
    /// fail-closed-per-image contract, not decoded with aliased channels.
    #[test]
    fn equal_channel_bytes_inc_refuses_image_but_opens_file() {
        let xml = concat!(
            r#"<LMSDataContainerHeader Version="2"><Element Name="Root"><Children>"#,
            // Good image: 1 channel, 2x2 U8, contiguous -- decodes fine.
            r#"<Element Name="Good"><Data><Image><ImageDescription><Dimensions>"#,
            r#"<DimensionDescription DimID="1" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="1" BitInc="0"/>"#,
            r#"<DimensionDescription DimID="2" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="2" BitInc="0"/>"#,
            r#"</Dimensions><Channels>"#,
            r#"<ChannelDescription DataType="0" ChannelTag="0" Resolution="8" NameOfMeasuredQuantity="" Min="0" Max="255" Unit="" LUTName="Gray" IsLUTInverted="0" BytesInc="0" BitInc="0"/>"#,
            r#"</Channels></ImageDescription></Image></Data>"#,
            r#"<Memory MemoryBlockID="MB0" Size="16"/></Element>"#,
            // Bad image: two channels, BOTH BytesInc="0" -- they alias.
            r#"<Element Name="Bad"><Data><Image><ImageDescription><Dimensions>"#,
            r#"<DimensionDescription DimID="1" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="1" BitInc="0"/>"#,
            r#"<DimensionDescription DimID="2" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="2" BitInc="0"/>"#,
            r#"</Dimensions><Channels>"#,
            r#"<ChannelDescription DataType="0" ChannelTag="0" Resolution="8" NameOfMeasuredQuantity="" Min="0" Max="255" Unit="" LUTName="Green" IsLUTInverted="0" BytesInc="0" BitInc="0"/>"#,
            r#"<ChannelDescription DataType="0" ChannelTag="0" Resolution="8" NameOfMeasuredQuantity="" Min="0" Max="255" Unit="" LUTName="Red" IsLUTInverted="0" BytesInc="0" BitInc="0"/>"#,
            r#"</Channels></ImageDescription></Image></Data>"#,
            r#"<Memory MemoryBlockID="MB1" Size="16"/></Element>"#,
            r#"</Children></Element></LMSDataContainerHeader>"#,
        );
        let bytes = wrap_lif_bytes_two_blocks(
            xml,
            "MB0",
            &(0u8..16).collect::<Vec<u8>>(),
            "MB1",
            &(0u8..16).collect::<Vec<u8>>(),
        );
        let reader = open_synth(&bytes);
        assert_eq!(
            reader.meta().scene_count(),
            1,
            "an image whose channels have equal BytesInc (aliasing) must be REFUSED"
        );
        assert_eq!(reader.meta().scene(0).unwrap().name, "Good");
    }

    /// RED test for the Z-alias check: a Z dimension whose `BytesInc` is smaller than one
    /// frame (`y*x*itemsize`) makes consecutive Z planes silently overlap. The
    /// image must be REFUSED; the existing `max_offset` bound only proves the
    /// worst-case plane FITS in the block, it never proves planes don't alias.
    #[test]
    fn undersized_z_stride_refuses_image_but_opens_file() {
        let xml = concat!(
            r#"<LMSDataContainerHeader Version="2"><Element Name="Root"><Children>"#,
            // Good image: same minimal 2x2 U8 1-channel layout, no Z.
            r#"<Element Name="Good"><Data><Image><ImageDescription><Dimensions>"#,
            r#"<DimensionDescription DimID="1" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="1" BitInc="0"/>"#,
            r#"<DimensionDescription DimID="2" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="2" BitInc="0"/>"#,
            r#"</Dimensions><Channels>"#,
            r#"<ChannelDescription DataType="0" ChannelTag="0" Resolution="8" NameOfMeasuredQuantity="" Min="0" Max="255" Unit="" LUTName="Gray" IsLUTInverted="0" BytesInc="0" BitInc="0"/>"#,
            r#"</Channels></ImageDescription></Image></Data>"#,
            r#"<Memory MemoryBlockID="MB0" Size="16"/></Element>"#,
            // Bad image: 1 channel, Z=2 with BytesInc="2" (< frame_bytes=4 for
            // 2x2 U8): Z planes 0 and 1 overlap by 2 bytes.
            r#"<Element Name="Bad"><Data><Image><ImageDescription><Dimensions>"#,
            r#"<DimensionDescription DimID="1" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="1" BitInc="0"/>"#,
            r#"<DimensionDescription DimID="2" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="2" BitInc="0"/>"#,
            r#"<DimensionDescription DimID="3" NumberOfElements="2" Origin="0" Length="2e-6" Unit="m" BytesInc="2" BitInc="0"/>"#,
            r#"</Dimensions><Channels>"#,
            r#"<ChannelDescription DataType="0" ChannelTag="0" Resolution="8" NameOfMeasuredQuantity="" Min="0" Max="255" Unit="" LUTName="Gray" IsLUTInverted="0" BytesInc="0" BitInc="0"/>"#,
            r#"</Channels></ImageDescription></Image></Data>"#,
            r#"<Memory MemoryBlockID="MB1" Size="16"/></Element>"#,
            r#"</Children></Element></LMSDataContainerHeader>"#,
        );
        let bytes = wrap_lif_bytes_two_blocks(
            xml,
            "MB0",
            &(0u8..16).collect::<Vec<u8>>(),
            "MB1",
            &(0u8..16).collect::<Vec<u8>>(),
        );
        let reader = open_synth(&bytes);
        assert_eq!(
            reader.meta().scene_count(),
            1,
            "an image whose Z stride is smaller than one frame (overlapping Z planes) must be REFUSED"
        );
        assert_eq!(reader.meta().scene(0).unwrap().name, "Good");
    }

    /// Regression test: an unmodelled `DimID` declared with
    /// `NumberOfElements="0"` must REFUSE that image, not hard-error the whole
    /// file. Before the fix, the `dim.n == 0` guard ran before DimID
    /// classification and turned this into a whole-file `Err`.
    #[test]
    fn unmodeled_dimid_with_zero_elements_refuses_image_but_opens_file() {
        let xml = concat!(
            r#"<LMSDataContainerHeader Version="2"><Element Name="Root"><Children>"#,
            // Good image: same minimal 2x2 U8 layout as above.
            r#"<Element Name="Good"><Data><Image><ImageDescription><Dimensions>"#,
            r#"<DimensionDescription DimID="1" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="1" BitInc="0"/>"#,
            r#"<DimensionDescription DimID="2" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="2" BitInc="0"/>"#,
            r#"</Dimensions><Channels>"#,
            r#"<ChannelDescription DataType="0" ChannelTag="0" Resolution="8" NameOfMeasuredQuantity="" Min="0" Max="255" Unit="" LUTName="Gray" IsLUTInverted="0" BytesInc="0" BitInc="0"/>"#,
            r#"</Channels></ImageDescription></Image></Data>"#,
            r#"<Memory MemoryBlockID="MB0" Size="16"/></Element>"#,
            // Bad image: valid X/Y plus DimID=5 (rotation, unmodelled) declared
            // with NumberOfElements="0".
            r#"<Element Name="Bad"><Data><Image><ImageDescription><Dimensions>"#,
            r#"<DimensionDescription DimID="1" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="1" BitInc="0"/>"#,
            r#"<DimensionDescription DimID="2" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="2" BitInc="0"/>"#,
            r#"<DimensionDescription DimID="5" NumberOfElements="0" Origin="0" Length="0" Unit="" BytesInc="0" BitInc="0"/>"#,
            r#"</Dimensions><Channels>"#,
            r#"<ChannelDescription DataType="0" ChannelTag="0" Resolution="8" NameOfMeasuredQuantity="" Min="0" Max="255" Unit="" LUTName="Gray" IsLUTInverted="0" BytesInc="0" BitInc="0"/>"#,
            r#"</Channels></ImageDescription></Image></Data>"#,
            r#"<Memory MemoryBlockID="MB1" Size="16"/></Element>"#,
            r#"</Children></Element></LMSDataContainerHeader>"#,
        );
        // Two REAL blocks (MB0 for "Good", MB1 for "Bad"): the bad image now has
        // a present data block, so it can only be skipped by the unmodelled-DimID
        // (with 0 elements) logic -- NOT by a missing `MemoryBlockID`. Revert the
        // DimID-before-`n==0` reorder in `build_scene` and this fails.
        let bytes = wrap_lif_bytes_two_blocks(
            xml,
            "MB0",
            &(0u8..16).collect::<Vec<u8>>(),
            "MB1",
            &(0u8..16).collect::<Vec<u8>>(),
        );
        let reader = open_synth(&bytes);
        assert_eq!(
            reader.meta().scene_count(),
            1,
            "the unmodelled-DimID/0-elements image must be REFUSED, not abort the whole file"
        );
        assert_eq!(reader.meta().scene(0).unwrap().name, "Good");
    }
}
