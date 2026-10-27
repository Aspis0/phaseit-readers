// SPDX-License-Identifier: Apache-2.0
//! Shared data types: the FROZEN public contract for phasit-imread.
//!
//! Canonical axis order is **TCZYX** (matches OME / tifffile, so parity dumps
//! align 1:1 with the Python reference). A "plane" is a single 2-D `[Y, X]`
//! image selected by a `(t, c, z)` coordinate within a scene.
//!
//! Buffers handed to callers are always **owned** and **native-endian**: raw
//! on-disk bytes (which may be big-endian, e.g. `MM` TIFF/LSM) are byteswapped
//! at decode time by [`PlaneBuf::from_bytes`].

use crate::error::{Error, Result};
use byteorder::{BigEndian, LittleEndian, ReadBytesExt};

/// Image dimensions in canonical **TCZYX** order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dims5 {
    /// Number of time points (T).
    pub t: usize,
    /// Number of channels (C).
    pub c: usize,
    /// Number of focal planes / depth (Z).
    pub z: usize,
    /// Height in pixels (Y).
    pub y: usize,
    /// Width in pixels (X).
    pub x: usize,
}

impl Dims5 {
    /// Total number of 2-D planes = `t * c * z`.
    ///
    /// Saturates instead of panicking on the (pathological) overflow case;
    /// real dimensions parsed from files never approach `usize::MAX`.
    ///
    /// NOT for allocation sizing (saturates on overflow); use
    /// [`checked_plane_bytes`](Dims5::checked_plane_bytes) /
    /// [`guard::checked_plane_alloc`](crate::guard::checked_plane_alloc) before
    /// allocating.
    pub fn num_planes(&self) -> usize {
        self.t.saturating_mul(self.c).saturating_mul(self.z)
    }

    /// Number of elements in one 2-D plane = `y * x`.
    ///
    /// NOT for allocation sizing (saturates on overflow); use
    /// [`checked_plane_bytes`](Dims5::checked_plane_bytes) /
    /// [`guard::checked_plane_alloc`](crate::guard::checked_plane_alloc) before
    /// allocating.
    pub fn plane_elems(&self) -> usize {
        self.y.saturating_mul(self.x)
    }

    /// Checked byte size of one 2-D plane (`x * y * dtype.size_bytes()`),
    /// refusing it if the product overflows `usize` or exceeds
    /// [`MAX_DECODED_PLANE_BYTES`](crate::guard::MAX_DECODED_PLANE_BYTES).
    ///
    /// This is the single source of truth for plane sizing;
    /// [`guard::checked_plane_alloc`](crate::guard::checked_plane_alloc)
    /// delegates here. Call it BEFORE reserving any memory.
    ///
    /// Grayscale (1 sample per pixel); for color / multi-sample planes use
    /// [`checked_plane_bytes_samples`](Dims5::checked_plane_bytes_samples).
    pub fn checked_plane_bytes(&self, dtype: DType) -> Result<usize> {
        self.checked_plane_bytes_samples(dtype, 1)
    }

    /// Checked byte size of one 2-D plane carrying `samples` interleaved samples
    /// per pixel (`x * y * samples * dtype.size_bytes()`), refusing it if the
    /// product overflows `usize` or exceeds
    /// [`MAX_DECODED_PLANE_BYTES`](crate::guard::MAX_DECODED_PLANE_BYTES).
    ///
    /// `samples` is the samples-per-pixel count (1 = grayscale, 3 = RGB). Here
    /// `dtype` is the PER-SAMPLE type, so a Bgr24/RGB plane is `(U8, samples=3)`
    /// and a Bgr48/RGB48 plane is `(U16, samples=3)`. Call this BEFORE reserving
    /// any memory.
    pub fn checked_plane_bytes_samples(&self, dtype: DType, samples: u32) -> Result<usize> {
        let bytes = self
            .x
            .checked_mul(self.y)
            .and_then(|elems| elems.checked_mul(samples as usize))
            .and_then(|elems| elems.checked_mul(dtype.size_bytes()));

        match bytes {
            // Overflow: report the cap and a saturated request so callers/logs
            // see it as an oversize refusal rather than a panic.
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

    /// Flat plane index for a `(t, c, z)` coordinate in canonical order,
    /// with Z varying fastest, then C, then T: `((t*C + c)*Z + z)`.
    ///
    /// Saturates instead of panicking/wrapping on overflow (consistent with
    /// [`num_planes`](Dims5::num_planes) / [`plane_elems`](Dims5::plane_elems));
    /// real dimensions parsed from files never approach `usize::MAX`.
    pub fn plane_index(&self, t: usize, c: usize, z: usize) -> usize {
        t.saturating_mul(self.c)
            .saturating_add(c)
            .saturating_mul(self.z)
            .saturating_add(z)
    }

    /// Enumerate every plane coordinate `(t, c, z)` in canonical TCZYX order
    /// (Z fastest, then C, then T). The yielded order matches [`plane_index`].
    ///
    /// [`plane_index`]: Dims5::plane_index
    pub fn iter_planes(&self) -> impl Iterator<Item = (usize, usize, usize)> {
        let (c, z) = (self.c, self.z);
        (0..self.t).flat_map(move |ti| {
            (0..c).flat_map(move |ci| (0..z).map(move |zi| (ti, ci, zi)))
        })
    }
}

/// Pixel sample type. Microscopy is dominated by 8/16-bit unsigned; signed
/// 16-bit and 32-bit float appear in LSM/CZI. Extensible: add variants as
/// concrete readers require them (and extend [`DType::size_bytes`] +
/// [`PlaneBuf`] in lockstep).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DType {
    /// Unsigned 8-bit.
    U8,
    /// Unsigned 16-bit.
    U16,
    /// Signed 16-bit.
    I16,
    /// 32-bit IEEE-754 float.
    F32,
}

impl DType {
    /// Size of one sample in bytes.
    pub fn size_bytes(&self) -> usize {
        match self {
            DType::U8 => 1,
            DType::U16 => 2,
            DType::I16 => 2,
            DType::F32 => 4,
        }
    }
}

/// A rectangular pixel region within a scene's 2-D plane, used to request a
/// sub-region without decoding/holding the whole plane (see
/// [`ImageReader::region`]).
///
/// All fields are in pixels and share [`Plane::shape`]'s `[Y, X]` convention:
/// `x`/`w` address the horizontal (width) axis, `y`/`h` the vertical (height)
/// axis. `(x, y)` is the top-left corner within the scene's full plane
/// (`dims.x` x `dims.y`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRect {
    /// Left edge (X) in pixels within the scene plane.
    pub x: usize,
    /// Top edge (Y) in pixels within the scene plane.
    pub y: usize,
    /// Width in pixels.
    pub w: usize,
    /// Height in pixels.
    pub h: usize,
}

impl PixelRect {
    /// Construct a rect from its top-left `(x, y)` corner and `(w, h)` extent.
    pub fn new(x: usize, y: usize, w: usize, h: usize) -> PixelRect {
        PixelRect { x, y, w, h }
    }

    /// True if the rect covers zero pixels (`w == 0 || h == 0`).
    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }
}

/// Byte order of raw on-disk sample data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endian {
    /// Little-endian (`II` / Intel).
    Little,
    /// Big-endian (`MM` / Motorola).
    Big,
}

/// Owned, native-endian pixel buffer, one variant per [`DType`].
#[derive(Debug, Clone, PartialEq)]
pub enum PlaneBuf {
    /// Unsigned 8-bit samples.
    U8(Vec<u8>),
    /// Unsigned 16-bit samples.
    U16(Vec<u16>),
    /// Signed 16-bit samples.
    I16(Vec<i16>),
    /// 32-bit float samples.
    F32(Vec<f32>),
}

impl PlaneBuf {
    /// The [`DType`] of this buffer.
    pub fn dtype(&self) -> DType {
        match self {
            PlaneBuf::U8(_) => DType::U8,
            PlaneBuf::U16(_) => DType::U16,
            PlaneBuf::I16(_) => DType::I16,
            PlaneBuf::F32(_) => DType::F32,
        }
    }

    /// Number of samples (elements), NOT bytes.
    pub fn len(&self) -> usize {
        match self {
            PlaneBuf::U8(v) => v.len(),
            PlaneBuf::U16(v) => v.len(),
            PlaneBuf::I16(v) => v.len(),
            PlaneBuf::F32(v) => v.len(),
        }
    }

    /// Whether the buffer holds zero samples.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Decode raw little/big-endian bytes into a native-endian owned buffer.
    ///
    /// This is the single reusable entry point every reader uses to turn a
    /// strip/tile/chunk of bytes into pixels. `byteorder` is used (not a typed
    /// `bytemuck`/`zerocopy` cast) so that big-endian source data is correctly
    /// byteswapped on a little-endian host and the result is always
    /// native-endian, regardless of buffer alignment.
    ///
    /// Errors with [`Error::Decode`] if `raw.len()` is not an exact multiple of
    /// `dtype.size_bytes()`.
    // Plain modulo (not `usize::is_multiple_of`, which is Rust 1.87+) keeps the
    // crate buildable on older stable toolchains.
    #[allow(clippy::manual_is_multiple_of)]
    pub fn from_bytes(dtype: DType, endian: Endian, raw: &[u8]) -> Result<PlaneBuf> {
        let sz = dtype.size_bytes();
        if raw.len() % sz != 0 {
            return Err(Error::Decode(format!(
                "raw byte length {} is not a multiple of {:?} sample size {}",
                raw.len(),
                dtype,
                sz
            )));
        }
        let n = raw.len() / sz;

        Ok(match dtype {
            // U8 is endian-agnostic; a plain copy is correct and fastest.
            DType::U8 => PlaneBuf::U8(raw.to_vec()),
            DType::U16 => {
                let mut out = vec![0u16; n];
                let mut cur = raw;
                match endian {
                    Endian::Little => cur.read_u16_into::<LittleEndian>(&mut out)?,
                    Endian::Big => cur.read_u16_into::<BigEndian>(&mut out)?,
                }
                PlaneBuf::U16(out)
            }
            DType::I16 => {
                let mut out = vec![0i16; n];
                let mut cur = raw;
                match endian {
                    Endian::Little => cur.read_i16_into::<LittleEndian>(&mut out)?,
                    Endian::Big => cur.read_i16_into::<BigEndian>(&mut out)?,
                }
                PlaneBuf::I16(out)
            }
            DType::F32 => {
                let mut out = vec![0f32; n];
                let mut cur = raw;
                match endian {
                    Endian::Little => cur.read_f32_into::<LittleEndian>(&mut out)?,
                    Endian::Big => cur.read_f32_into::<BigEndian>(&mut out)?,
                }
                PlaneBuf::F32(out)
            }
        })
    }
}

/// A single decoded 2-D plane: owned, native-endian pixels plus their `[Y, X]`
/// shape. Construct via [`Plane::new`] to enforce the length invariant.
///
/// # Color / multi-sample planes
///
/// [`samples`](Self::samples) is the number of interleaved samples per pixel:
/// `1` for grayscale (the default, byte-identical to a single-channel plane) and
/// `3` for RGB. The shape stays `[Y, X]` (pixel dimensions, NOT sample count);
/// the buffer is interleaved row-major, so `data.len() == Y * X * samples`. A
/// color plane's [`DType`] is the PER-SAMPLE type (RGB24 → `U8, samples=3`,
/// RGB48 → `U16, samples=3`). Samples are emitted in R,G,B order regardless of
/// the source container's byte order (CZI stores BGR and is swapped on decode).
#[derive(Debug, Clone, PartialEq)]
pub struct Plane {
    /// Pixel samples in row-major (Y-major, X-minor) order. For a multi-sample
    /// plane, samples are interleaved within each pixel (`R,G,B, R,G,B, …`).
    pub data: PlaneBuf,
    /// Plane shape as `[Y, X]` (height, width), in pixels — independent of
    /// [`samples`](Self::samples).
    pub shape: [usize; 2],
    /// Samples per pixel (`1` = grayscale, `3` = RGB). Interleaved in `data`.
    pub samples: u32,
}

impl Plane {
    /// Build a grayscale plane (1 sample per pixel), checking that
    /// `data.len() == shape[0] * shape[1]`.
    ///
    /// Errors with [`Error::Decode`] on a shape/length mismatch or if the
    /// shape product overflows `usize`.
    pub fn new(data: PlaneBuf, shape: [usize; 2]) -> Result<Plane> {
        Plane::new_samples(data, shape, 1)
    }

    /// Build a plane carrying `samples` interleaved samples per pixel, checking
    /// that `data.len() == shape[0] * shape[1] * samples`.
    ///
    /// `samples == 1` is a grayscale plane, identical to [`Plane::new`]. Errors
    /// with [`Error::Decode`] on a shape/length mismatch, on `samples == 0`, or
    /// if the length product overflows `usize`.
    pub fn new_samples(data: PlaneBuf, shape: [usize; 2], samples: u32) -> Result<Plane> {
        if samples == 0 {
            return Err(Error::Decode("plane samples per pixel must be >= 1".to_string()));
        }
        let expected = shape[0]
            .checked_mul(shape[1])
            .and_then(|px| px.checked_mul(samples as usize))
            .ok_or_else(|| {
                Error::Decode(format!(
                    "plane shape {}x{}x{} overflows usize",
                    shape[0], shape[1], samples
                ))
            })?;
        if data.len() != expected {
            return Err(Error::Decode(format!(
                "plane data length {} does not match shape {}x{} ({} samples/px, {} elements)",
                data.len(),
                shape[0],
                shape[1],
                samples,
                expected
            )));
        }
        Ok(Plane { data, shape, samples })
    }

    /// The pixel [`DType`] of this plane (the per-sample type for color planes).
    pub fn dtype(&self) -> DType {
        self.data.dtype()
    }

    /// Samples per pixel (`1` = grayscale, `3` = RGB).
    pub fn samples(&self) -> u32 {
        self.samples
    }

    /// Height (Y) in pixels.
    pub fn height(&self) -> usize {
        self.shape[0]
    }

    /// Width (X) in pixels.
    pub fn width(&self) -> usize {
        self.shape[1]
    }

    /// Crop this plane to sub-region `rect`, producing a new owned [`Plane`]
    /// of shape `[rect.h, rect.w]` with the same [`dtype`](Self::dtype) /
    /// [`samples`](Self::samples).
    ///
    /// Samples-aware: for a multi-sample (e.g. RGB) plane all interleaved
    /// samples of each covered pixel are copied together, so the result is
    /// byte-exact with the corresponding window of the source plane. Errors
    /// with [`Error::Decode`] if `rect` extends past this plane's
    /// [`width`](Self::width)/[`height`](Self::height) (checked adds; an
    /// overflowing `x + w` / `y + h` is also rejected, never wraps/panics).
    ///
    /// This is the same crop used by [`ImageReader::region`]'s provided
    /// default; a format backend that overrides `region` to fetch a
    /// slightly-larger tile directly from disk can still call this to trim it
    /// down to the exact requested `rect`.
    pub fn crop(&self, rect: PixelRect) -> Result<Plane> {
        let full_w = self.width();
        let full_h = self.height();

        let x_end = rect.x.checked_add(rect.w).ok_or_else(|| {
            Error::Decode(format!(
                "crop rect x+w overflows usize (x={}, w={})",
                rect.x, rect.w
            ))
        })?;
        let y_end = rect.y.checked_add(rect.h).ok_or_else(|| {
            Error::Decode(format!(
                "crop rect y+h overflows usize (y={}, h={})",
                rect.y, rect.h
            ))
        })?;
        if x_end > full_w || y_end > full_h {
            return Err(Error::Decode(format!(
                "crop rect {{x:{}, y:{}, w:{}, h:{}}} out of range for plane {}x{} (w x h)",
                rect.x, rect.y, rect.w, rect.h, full_w, full_h
            )));
        }

        let samples = self.samples as usize;

        // One macro arm per `PlaneBuf` variant: copy `rect.h` rows, each row a
        // contiguous samples-aware slice `[row_start..row_start + rect.w * samples)`
        // taken from the source row `rect.y + row` at column offset `rect.x`.
        macro_rules! crop_rows {
            ($v:expr) => {{
                let mut out = Vec::with_capacity(rect.h * rect.w * samples);
                for row in 0..rect.h {
                    let src_row = rect.y + row;
                    let row_start = (src_row * full_w + rect.x) * samples;
                    let row_end = row_start + rect.w * samples;
                    out.extend_from_slice(&$v[row_start..row_end]);
                }
                out
            }};
        }

        let data = match &self.data {
            PlaneBuf::U8(v) => PlaneBuf::U8(crop_rows!(v)),
            PlaneBuf::U16(v) => PlaneBuf::U16(crop_rows!(v)),
            PlaneBuf::I16(v) => PlaneBuf::I16(crop_rows!(v)),
            PlaneBuf::F32(v) => PlaneBuf::F32(crop_rows!(v)),
        };

        Plane::new_samples(data, [rect.h, rect.w], self.samples)
    }

    /// Convert to a typed `ndarray::Array2` (dispatched on dtype).
    ///
    /// Feature-gated so `ndarray` never leaks into the default public surface.
    /// Only grayscale (`samples == 1`) planes map to a 2-D `[Y, X]` array; a
    /// multi-sample (color) plane is refused with [`Error::Decode`] rather than
    /// silently reshaping interleaved samples.
    #[cfg(feature = "ndarray")]
    pub fn to_ndarray(&self) -> Result<PlaneArray> {
        if self.samples != 1 {
            return Err(Error::Decode(format!(
                "to_ndarray supports only grayscale planes (samples == 1), got {}",
                self.samples
            )));
        }
        let shape = (self.shape[0], self.shape[1]);
        let map_err = |e: ndarray::ShapeError| Error::Decode(format!("ndarray shape error: {e}"));
        Ok(match &self.data {
            PlaneBuf::U8(v) => {
                PlaneArray::U8(ndarray::Array2::from_shape_vec(shape, v.clone()).map_err(map_err)?)
            }
            PlaneBuf::U16(v) => {
                PlaneArray::U16(ndarray::Array2::from_shape_vec(shape, v.clone()).map_err(map_err)?)
            }
            PlaneBuf::I16(v) => {
                PlaneArray::I16(ndarray::Array2::from_shape_vec(shape, v.clone()).map_err(map_err)?)
            }
            PlaneBuf::F32(v) => {
                PlaneArray::F32(ndarray::Array2::from_shape_vec(shape, v.clone()).map_err(map_err)?)
            }
        })
    }
}

/// Typed 2-D array view of a [`Plane`], one variant per [`DType`].
///
/// Only present with the `ndarray` feature so the dependency stays optional.
#[cfg(feature = "ndarray")]
#[derive(Debug, Clone, PartialEq)]
pub enum PlaneArray {
    /// Unsigned 8-bit array.
    U8(ndarray::Array2<u8>),
    /// Unsigned 16-bit array.
    U16(ndarray::Array2<u16>),
    /// Signed 16-bit array.
    I16(ndarray::Array2<i16>),
    /// 32-bit float array.
    F32(ndarray::Array2<f32>),
}

/// Per-channel metadata.
///
/// Channel identity is the position in [`SceneMeta::channels`]: channel `c` in
/// [`ImageReader::plane`] / `dims.c` refers to `channels[c]`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChannelMeta {
    /// Human-readable channel name, if the format provides one.
    pub name: Option<String>,
    /// Display color as RGB, if the format provides one.
    pub color: Option<[u8; 3]>,
}

/// Physical pixel size in micrometres. A named struct (not a bare `[f32; 3]`)
/// so the Z/Y/X axes can never be silently transposed by a caller.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PxSizeUm {
    /// Spacing between focal planes (Z) in micrometres.
    pub z: f32,
    /// Vertical pixel size (Y) in micrometres.
    pub y: f32,
    /// Horizontal pixel size (X) in micrometres.
    pub x: f32,
}

/// Metadata for a single scene (a.k.a. series / image) within a file.
///
/// One file can hold multiple scenes with DIFFERENT dims, dtype and channel
/// counts (e.g. a Leica LIF with one scene 72×3×2048×2048 and another
/// 55×2×1024×1024), so every scene carries its own copy of these fields.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneMeta {
    /// Dimensions in canonical TCZYX order.
    pub dims: Dims5,
    /// Pixel sample type. For a color scene this is the PER-SAMPLE type (RGB24 →
    /// `U8`, RGB48 → `U16`); see [`samples`](Self::samples).
    pub dtype: DType,
    /// Samples per pixel for this scene's planes (`1` = grayscale, `3` = RGB).
    /// Every [`Plane`] this scene yields carries this same `samples` value.
    ///
    /// NOTE: `samples > 1` is the convention of the readers that keep native
    /// colour: the CZI reader keeps colour (Bgr24/Bgr48) as interleaved
    /// samples rather than exploding it into `dims.c`, and the plain-TIFF
    /// reader does the same for an RGB page. The LSM/LIF readers instead map
    /// colour onto `dims.c` and always set `samples == 1`. (Unifying all
    /// readers' colour handling is out of scope; this field is the per-scene
    /// truth regardless of which convention produced it.)
    pub samples: u32,
    /// Physical pixel size in micrometres. `None` when the format does not
    /// record calibration.
    pub px_size_um: Option<PxSizeUm>,
    /// Per-channel metadata (length should equal `dims.c`).
    pub channels: Vec<ChannelMeta>,
    /// The colour palette of an INDEXED scene: `palette[i]` is the RGB colour
    /// of index `i`, and the scene's planes carry the index values.
    ///
    /// `None` for a scene that is not indexed. An indexed scene keeps its
    /// index values in the planes (the way Bio-Formats keeps them) and
    /// publishes the palette beside them.
    pub palette: Option<Vec<[u8; 3]>>,
    /// Scene / image name (as embedded in the container).
    pub name: String,
}

/// Image-level metadata, cached at `open()` time by a reader.
///
/// Most files hold one scene and build a one-element [`scenes`](Self::scenes)
/// vector; the multi-scene formats (LIF/CZI/ND2) build one entry per scene, as
/// does an LSM tile or multi-position file, whose CZ-LSM-Info counts state how
/// many separate acquisitions it holds.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageMeta {
    /// File-level display name (typically the file name).
    pub name: String,
    /// Per-scene metadata; always at least one entry.
    pub scenes: Vec<SceneMeta>,
}

impl ImageMeta {
    /// Number of scenes / series in the file (always `>= 1`).
    pub fn scene_count(&self) -> usize {
        self.scenes.len()
    }

    /// Borrow the [`SceneMeta`] for `scene`, or [`Error::SceneOutOfRange`] if
    /// `scene >= scene_count()`.
    pub fn scene(&self, scene: usize) -> Result<&SceneMeta> {
        self.scenes.get(scene).ok_or(Error::SceneOutOfRange {
            scene,
            count: self.scenes.len(),
        })
    }
}

/// An `<Image>` of a file that its reader refused, with the reason named. The
/// readable scenes of the same file are still read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusedScene {
    /// Position of the refused `<Image>` in the file, counting every `<Image>`.
    pub image_index: usize,
    /// The `<Image>`'s name, or `"unnamed"` when it has none.
    pub name: String,
    /// Why the scene cannot be read.
    pub reason: String,
}

impl std::fmt::Display for RefusedScene {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<Image> {} \"{}\": {}", self.image_index, self.name, self.reason)
    }
}

/// The reader contract every format backend implements.
///
/// `plane` takes `&self` (not `&mut self`): backends wrapping a `&mut self`-only
/// decoder (e.g. `tiff::Decoder`) store the path/mmap and build a fresh decoder
/// per call. `Send + Sync` lets a reader be shared across threads.
pub trait ImageReader: Send + Sync {
    // TODO(perf): add a defaulted `planes(&self, scene, t, z, channels: &[usize])
    // -> Result<Vec<Plane>>` bulk method to avoid re-decoding an interleaved-RGB
    // page once per channel. Non-breaking to add later as a provided method.

    /// Open a file, eagerly parsing and caching [`ImageMeta`].
    fn open(path: &std::path::Path) -> Result<Self>
    where
        Self: Sized;

    /// Cached image metadata for the whole file (cheap; no I/O).
    fn meta(&self) -> &ImageMeta;

    /// Cached metadata for a single scene (cheap; no I/O). Provided default
    /// delegates to [`ImageMeta::scene`]; returns [`Error::SceneOutOfRange`]
    /// for an invalid `scene`.
    fn scene_meta(&self, scene: usize) -> Result<&SceneMeta> {
        self.meta().scene(scene)
    }

    /// The `<Image>`s of the file that this reader refused, in file order, each
    /// with its reason. LIF, CZI and OME-TIFF fill it. An empty list from any
    /// other reader means that reader never refuses a scene, not that the file
    /// is clean.
    fn refused_scenes(&self) -> &[RefusedScene] {
        &[]
    }

    /// The position of scene `scene` in the file's own list of `<Image>`s. It
    /// equals `scene` unless the reader refused an earlier scene of the file.
    fn scene_file_index(&self, scene: usize) -> usize {
        scene
    }

    /// Lazily decode the plane at `(scene, t, c, z)` into an owned,
    /// native-endian [`Plane`].
    ///
    /// Implementors MUST validate `(scene, t, c, z)` against
    /// [`scene_meta`](Self::scene_meta) and return [`Error::OutOfRange`] (not a
    /// panic, not a silently wrapped index) when any coordinate is out of range.
    fn plane(&self, scene: usize, t: usize, c: usize, z: usize) -> Result<Plane>;

    /// True if this reader implements [`region`](Self::region) as a genuine
    /// out-of-core sub-region decode (only CZI today). Readers that fall back to
    /// the default [`region`](Self::region) (which decodes the whole plane then
    /// crops) return `false`, so a caller doing many sub-region reads of one
    /// plane can decode the plane ONCE and crop in memory instead of re-decoding
    /// per tile.
    fn supports_region(&self) -> bool {
        false
    }

    /// Decode sub-region `rect` of the plane at `(scene, t, c, z)`.
    ///
    /// The PROVIDED default here is correct but **NOT out-of-core**: it
    /// decodes the FULL plane via [`plane`](Self::plane) and crops it in
    /// memory down to `rect` (reusing [`Plane::crop`], so the result is
    /// byte-exact vs the corresponding window of the full plane). Format
    /// backends that can read a sub-region cheaply from disk (e.g. a tiled
    /// container that can seek directly to the tiles overlapping `rect`)
    /// SHOULD override this method — the whole point being to avoid decoding
    /// pixels outside `rect`, which matters for whole-slide scenes whose full
    /// plane may itself be refused by [`crate::guard::MAX_DECODED_PLANE_BYTES`]
    /// (the CZI reader overrides this: `CziReader::region`).
    ///
    /// Validates the coordinate FIRST, then `rect`, matching an overriding
    /// backend's order (e.g. `CziReader::region`) so a doubly-invalid call
    /// returns a consistent variant: [`Error::OutOfRange`] if `(t, c, z)` is
    /// out of range for [`scene_meta`](Self::scene_meta)'s `dims`, otherwise
    /// [`Error::Decode`] if `rect.x + rect.w > dims.x` or `rect.y + rect.h >
    /// dims.y` (checked adds; an overflowing `x + w` / `y + h` is also
    /// rejected rather than wrapping).
    fn region(&self, scene: usize, t: usize, c: usize, z: usize, rect: PixelRect) -> Result<Plane> {
        let dims = self.scene_meta(scene)?.dims;

        // Validate `(t, c, z)` BEFORE `rect`, matching `CziReader::region`'s
        // `scene_checked`-first order. Without this the default deferred coord
        // validation to the inner `plane()`, so a call that was invalid on BOTH
        // the coordinate AND the rect returned `Decode` (rect) here but
        // `OutOfRange` from the CZI override — an order-inconsistent contract.
        if t >= dims.t || c >= dims.c || z >= dims.z {
            return Err(Error::OutOfRange { scene, t, c, z });
        }

        let x_end = rect.x.checked_add(rect.w).ok_or_else(|| {
            Error::Decode(format!(
                "region rect x+w overflows usize (x={}, w={})",
                rect.x, rect.w
            ))
        })?;
        let y_end = rect.y.checked_add(rect.h).ok_or_else(|| {
            Error::Decode(format!(
                "region rect y+h overflows usize (y={}, h={})",
                rect.y, rect.h
            ))
        })?;
        if x_end > dims.x || y_end > dims.y {
            return Err(Error::Decode(format!(
                "region rect {{x:{}, y:{}, w:{}, h:{}}} out of range for scene {} dims (x:{}, y:{})",
                rect.x, rect.y, rect.w, rect.h, scene, dims.x, dims.y
            )));
        }

        self.plane(scene, t, c, z)?.crop(rect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dtype_sizes() {
        assert_eq!(DType::U8.size_bytes(), 1);
        assert_eq!(DType::U16.size_bytes(), 2);
        assert_eq!(DType::I16.size_bytes(), 2);
        assert_eq!(DType::F32.size_bytes(), 4);
    }

    #[test]
    fn from_bytes_u16_endianness() {
        // Big-endian [0x12, 0x34] => 0x1234.
        let be = PlaneBuf::from_bytes(DType::U16, Endian::Big, &[0x12, 0x34]).unwrap();
        assert_eq!(be, PlaneBuf::U16(vec![0x1234]));
        // Little-endian [0x12, 0x34] => 0x3412.
        let le = PlaneBuf::from_bytes(DType::U16, Endian::Little, &[0x12, 0x34]).unwrap();
        assert_eq!(le, PlaneBuf::U16(vec![0x3412]));
    }

    #[test]
    fn from_bytes_i16_endianness() {
        // 0xFFFF == -1 in i16, regardless of endianness for this symmetric value.
        let be = PlaneBuf::from_bytes(DType::I16, Endian::Big, &[0xFF, 0xFF]).unwrap();
        assert_eq!(be, PlaneBuf::I16(vec![-1]));
        // Big-endian [0x80, 0x00] => i16::MIN.
        let min = PlaneBuf::from_bytes(DType::I16, Endian::Big, &[0x80, 0x00]).unwrap();
        assert_eq!(min, PlaneBuf::I16(vec![i16::MIN]));
    }

    #[test]
    fn from_bytes_f32_endianness() {
        // 1.0f32 == 0x3F800000.
        let le = PlaneBuf::from_bytes(DType::F32, Endian::Little, &[0x00, 0x00, 0x80, 0x3F]).unwrap();
        assert_eq!(le, PlaneBuf::F32(vec![1.0]));
        let be = PlaneBuf::from_bytes(DType::F32, Endian::Big, &[0x3F, 0x80, 0x00, 0x00]).unwrap();
        assert_eq!(be, PlaneBuf::F32(vec![1.0]));
    }

    #[test]
    fn from_bytes_multi_element() {
        let le =
            PlaneBuf::from_bytes(DType::U16, Endian::Little, &[0x01, 0x00, 0x02, 0x00]).unwrap();
        assert_eq!(le, PlaneBuf::U16(vec![1, 2]));
        assert_eq!(le.len(), 2);
        assert!(!le.is_empty());
    }

    #[test]
    fn from_bytes_length_not_multiple_errors() {
        // 3 bytes is not a multiple of 2 (u16).
        let err = PlaneBuf::from_bytes(DType::U16, Endian::Little, &[0x00, 0x01, 0x02]);
        assert!(matches!(err, Err(Error::Decode(_))));
        // 3 bytes is not a multiple of 4 (f32).
        let err = PlaneBuf::from_bytes(DType::F32, Endian::Big, &[0x00, 0x01, 0x02]);
        assert!(matches!(err, Err(Error::Decode(_))));
    }

    #[test]
    fn from_bytes_empty_is_empty() {
        let e = PlaneBuf::from_bytes(DType::U16, Endian::Little, &[]).unwrap();
        assert!(e.is_empty());
        assert_eq!(e.len(), 0);
        assert_eq!(e.dtype(), DType::U16);
    }

    #[test]
    fn dims_num_planes_and_plane_elems() {
        let d = Dims5 { t: 2, c: 3, z: 4, y: 5, x: 6 };
        assert_eq!(d.num_planes(), 24);
        assert_eq!(d.plane_elems(), 30);
    }

    #[test]
    fn dims_checked_plane_bytes() {
        let d = Dims5 { t: 1, c: 1, z: 1, y: 2048, x: 2048 };
        assert_eq!(d.checked_plane_bytes(DType::U16).unwrap(), 2048 * 2048 * 2);

        // Oversized: past the 4 GiB cap.
        let big = Dims5 { t: 1, c: 1, z: 1, y: 100_000, x: 100_000 };
        assert!(matches!(
            big.checked_plane_bytes(DType::F32),
            Err(Error::OversizedAllocation { .. })
        ));

        // Overflow must be a clean error, not a panic.
        let overflow = Dims5 { t: 1, c: 1, z: 1, y: usize::MAX, x: 2 };
        match overflow.checked_plane_bytes(DType::U8) {
            Err(Error::OversizedAllocation { requested, cap }) => {
                assert_eq!(requested, usize::MAX);
                assert_eq!(cap, crate::guard::MAX_DECODED_PLANE_BYTES);
            }
            other => panic!("expected OversizedAllocation, got {other:?}"),
        }
    }

    #[test]
    fn dims_iter_planes_matches_index_order() {
        let d = Dims5 { t: 2, c: 2, z: 3, y: 1, x: 1 };
        let coords: Vec<_> = d.iter_planes().collect();
        assert_eq!(coords.len(), d.num_planes());
        // Z fastest, then C, then T; and it must agree with plane_index.
        for (i, &(t, c, z)) in coords.iter().enumerate() {
            assert_eq!(d.plane_index(t, c, z), i);
        }
        assert_eq!(coords[0], (0, 0, 0));
        assert_eq!(coords[1], (0, 0, 1));
        assert_eq!(coords[3], (0, 1, 0));
        assert_eq!(coords.last().copied(), Some((1, 1, 2)));
    }

    #[test]
    fn plane_index_saturates_without_panicking() {
        // Saturating plane index: plane_index uses plain `*`/`+` unlike num_planes/plane_elems,
        // so it panics (debug overflow-checks) / wraps (release) on pathological
        // dims instead of saturating like its siblings.
        let d = Dims5 { t: 1, c: usize::MAX, z: usize::MAX, y: 1, x: 1 };
        let idx = d.plane_index(2, 2, 2);
        assert_eq!(idx, usize::MAX);
    }

    #[test]
    fn plane_new_enforces_length() {
        let buf = PlaneBuf::U8(vec![0; 6]);
        let p = Plane::new(buf, [2, 3]).unwrap();
        assert_eq!(p.height(), 2);
        assert_eq!(p.width(), 3);
        assert_eq!(p.dtype(), DType::U8);

        let bad = Plane::new(PlaneBuf::U8(vec![0; 5]), [2, 3]);
        assert!(matches!(bad, Err(Error::Decode(_))));

        // Grayscale default: samples == 1.
        assert_eq!(p.samples, 1);
    }

    #[test]
    fn plane_new_samples_enforces_length_and_interleave() {
        // 2x2 RGB (samples=3) => 12 interleaved U8 samples.
        let rgb = PlaneBuf::U8(vec![0; 12]);
        let p = Plane::new_samples(rgb, [2, 2], 3).unwrap();
        assert_eq!(p.shape, [2, 2]);
        assert_eq!(p.samples(), 3);
        assert_eq!(p.data.len(), 12);

        // Wrong length for the sample count is rejected.
        let bad = Plane::new_samples(PlaneBuf::U8(vec![0; 4]), [2, 2], 3);
        assert!(matches!(bad, Err(Error::Decode(_))));

        // Zero samples per pixel is rejected.
        let zero = Plane::new_samples(PlaneBuf::U8(vec![]), [2, 2], 0);
        assert!(matches!(zero, Err(Error::Decode(_))));
    }

    #[test]
    fn checked_plane_bytes_accounts_for_samples() {
        let d = Dims5 { t: 1, c: 1, z: 1, y: 174, x: 275 };
        // Grayscale U8 vs RGB24 (U8, 3 spp): the color plane is exactly 3x.
        assert_eq!(d.checked_plane_bytes(DType::U8).unwrap(), 174 * 275);
        assert_eq!(
            d.checked_plane_bytes_samples(DType::U8, 3).unwrap(),
            174 * 275 * 3
        );
        // RGB48 (U16, 3 spp): 6 bytes per pixel.
        assert_eq!(
            d.checked_plane_bytes_samples(DType::U16, 3).unwrap(),
            174 * 275 * 6
        );
    }

    #[cfg(feature = "ndarray")]
    #[test]
    fn plane_to_ndarray_roundtrip() {
        let p = Plane::new(PlaneBuf::U16(vec![1, 2, 3, 4, 5, 6]), [2, 3]).unwrap();
        match p.to_ndarray().unwrap() {
            PlaneArray::U16(a) => {
                assert_eq!(a.dim(), (2, 3));
                assert_eq!(a[[0, 0]], 1);
                assert_eq!(a[[1, 2]], 6);
            }
            _ => panic!("wrong ndarray variant"),
        }
    }

    // --- region() / PixelRect / Plane::crop --------------------------------------------

    /// Minimal in-memory [`ImageReader`] used only to exercise the `region()`
    /// provided default: one scene, one plane per `(t, c, z)`, synthetic
    /// deterministic pixels so a manual (independently-computed) crop can be
    /// compared bit-exactly against `region()`'s output.
    struct FakeReader {
        meta: ImageMeta,
    }

    impl FakeReader {
        /// `dtype` is fixed to `U8` (the byte-exactness logic is dtype-agnostic;
        /// endianness/dtype conversion is already covered by `PlaneBuf` tests).
        fn new(y: usize, x: usize, samples: u32) -> FakeReader {
            let dims = Dims5 { t: 1, c: 1, z: 1, y, x };
            let scene = SceneMeta {
                dims,
                dtype: DType::U8,
                samples,
                px_size_um: None,
                channels: vec![ChannelMeta::default()],
                palette: None,
                name: "fake".to_string(),
            };
            FakeReader {
                meta: ImageMeta { name: "fake".to_string(), scenes: vec![scene] },
            }
        }

        /// Deterministic synthetic sample value, distinct per pixel and per
        /// interleaved sample index so a wrong row/col/sample offset in a crop
        /// is guaranteed to produce a mismatch.
        fn synthetic(x: usize, y: usize, s: usize) -> u8 {
            ((x.wrapping_mul(7) ^ y.wrapping_mul(13)).wrapping_add(s * 31) % 251) as u8
        }
    }

    impl ImageReader for FakeReader {
        fn open(_path: &std::path::Path) -> Result<Self> {
            unimplemented!("FakeReader is test-only and never constructed via open()")
        }

        fn meta(&self) -> &ImageMeta {
            &self.meta
        }

        fn plane(&self, scene: usize, t: usize, c: usize, z: usize) -> Result<Plane> {
            let sm = self.scene_meta(scene)?;
            if t >= sm.dims.t || c >= sm.dims.c || z >= sm.dims.z {
                return Err(Error::OutOfRange { scene, t, c, z });
            }
            let (w, h, samples) = (sm.dims.x, sm.dims.y, sm.samples as usize);
            let mut data = vec![0u8; w * h * samples];
            for yy in 0..h {
                for xx in 0..w {
                    for s in 0..samples {
                        data[(yy * w + xx) * samples + s] = FakeReader::synthetic(xx, yy, s);
                    }
                }
            }
            Plane::new_samples(PlaneBuf::U8(data), [h, w], sm.samples)
        }
    }

    /// Independent (not reusing `Plane::crop`) manual crop of a full
    /// grayscale/RGB synthetic plane, used as the ground truth to compare
    /// `region()`'s output against.
    fn manual_crop(_full_w: usize, samples: usize, rect: PixelRect) -> Vec<u8> {
        let mut out = Vec::with_capacity(rect.h * rect.w * samples);
        for row in 0..rect.h {
            let yy = rect.y + row;
            for col in 0..rect.w {
                let xx = rect.x + col;
                for s in 0..samples {
                    out.push(FakeReader::synthetic(xx, yy, s));
                }
            }
        }
        out
    }

    fn assert_region_matches(reader: &FakeReader, w: usize, samples: u32, rect: PixelRect) {
        let region = reader.region(0, 0, 0, 0, rect).unwrap();
        assert_eq!(region.shape, [rect.h, rect.w]);
        assert_eq!(region.samples, samples);
        assert_eq!(region.dtype(), DType::U8);
        let expected = manual_crop(w, samples as usize, rect);
        match region.data {
            PlaneBuf::U8(ref v) => assert_eq!(*v, expected),
            other => panic!("expected U8 plane data, got {other:?}"),
        }
    }

    #[test]
    fn supports_region_defaults_to_false() {
        // A reader that does not override `supports_region()` (like every reader
        // that also falls back to the default `region()`) must report `false`,
        // which is what routes a consumer that builds pyramids onto its
        // plane-once path.
        let reader = FakeReader::new(4, 4, 1);
        assert!(!reader.supports_region());
    }

    #[test]
    fn region_grayscale_full_plane_matches_manual_crop() {
        let (h, w) = (10, 8);
        let reader = FakeReader::new(h, w, 1);
        assert_region_matches(&reader, w, 1, PixelRect::new(0, 0, w, h));
    }

    #[test]
    fn region_grayscale_single_pixel() {
        let (h, w) = (10, 8);
        let reader = FakeReader::new(h, w, 1);
        assert_region_matches(&reader, w, 1, PixelRect::new(3, 4, 1, 1));
    }

    #[test]
    fn region_grayscale_top_left_corner() {
        let (h, w) = (10, 8);
        let reader = FakeReader::new(h, w, 1);
        assert_region_matches(&reader, w, 1, PixelRect::new(0, 0, 3, 2));
    }

    #[test]
    fn region_grayscale_bottom_right_corner() {
        let (h, w) = (10, 8);
        let reader = FakeReader::new(h, w, 1);
        // Rect that touches both the right edge (x+w == w) and bottom edge (y+h == h).
        assert_region_matches(&reader, w, 1, PixelRect::new(w - 2, h - 3, 2, 3));
    }

    #[test]
    fn region_grayscale_right_edge_column() {
        let (h, w) = (10, 8);
        let reader = FakeReader::new(h, w, 1);
        assert_region_matches(&reader, w, 1, PixelRect::new(w - 1, 0, 1, h));
    }

    #[test]
    fn region_grayscale_bottom_edge_row() {
        let (h, w) = (10, 8);
        let reader = FakeReader::new(h, w, 1);
        assert_region_matches(&reader, w, 1, PixelRect::new(0, h - 1, w, 1));
    }

    #[test]
    fn region_rgb_samples_aware_crop() {
        // RGB (samples = 3): a wrong row/col/sample offset in a naive crop
        // would desync the interleave and mismatch immediately.
        let (h, w) = (12, 9);
        let reader = FakeReader::new(h, w, 3);
        assert_region_matches(&reader, w, 3, PixelRect::new(2, 3, 4, 5));
        // Also check an edge case for the color path specifically.
        assert_region_matches(&reader, w, 3, PixelRect::new(w - 1, h - 1, 1, 1));
    }

    #[test]
    fn region_out_of_bounds_x_errors() {
        let reader = FakeReader::new(10, 8, 1);
        let err = reader.region(0, 0, 0, 0, PixelRect::new(6, 0, 3, 1));
        assert!(matches!(err, Err(Error::Decode(_))));
    }

    #[test]
    fn region_out_of_bounds_y_errors() {
        let reader = FakeReader::new(10, 8, 1);
        let err = reader.region(0, 0, 0, 0, PixelRect::new(0, 9, 1, 3));
        assert!(matches!(err, Err(Error::Decode(_))));
    }

    #[test]
    fn region_checked_add_overflow_errors_cleanly() {
        let reader = FakeReader::new(10, 8, 1);
        // x + w overflows usize; must be a clean Decode error, never a panic.
        let err = reader.region(0, 0, 0, 0, PixelRect::new(usize::MAX, 0, 2, 1));
        assert!(matches!(err, Err(Error::Decode(_))));
        let err = reader.region(0, 0, 0, 0, PixelRect::new(0, usize::MAX, 1, 2));
        assert!(matches!(err, Err(Error::Decode(_))));
    }

    #[test]
    fn region_scene_out_of_range_errors() {
        let reader = FakeReader::new(10, 8, 1);
        let err = reader.region(1, 0, 0, 0, PixelRect::new(0, 0, 1, 1));
        assert!(matches!(err, Err(Error::SceneOutOfRange { .. })));
    }

    #[test]
    fn pixel_rect_is_empty() {
        assert!(PixelRect::new(0, 0, 0, 5).is_empty());
        assert!(PixelRect::new(0, 0, 5, 0).is_empty());
        assert!(!PixelRect::new(0, 0, 1, 1).is_empty());
    }

    #[test]
    fn plane_crop_matches_region_default() {
        // Plane::crop is the same machinery region() uses internally; exercise
        // it directly too (a format backend overriding region() would call it
        // on an already-decoded/over-fetched plane).
        let reader = FakeReader::new(6, 5, 1);
        let full = reader.plane(0, 0, 0, 0).unwrap();
        let rect = PixelRect::new(1, 2, 3, 3);
        let cropped = full.crop(rect).unwrap();
        let via_region = reader.region(0, 0, 0, 0, rect).unwrap();
        assert_eq!(cropped, via_region);
    }

    #[test]
    fn plane_crop_out_of_bounds_errors() {
        let full = Plane::new(PlaneBuf::U8(vec![0; 6]), [2, 3]).unwrap();
        let err = full.crop(PixelRect::new(2, 0, 2, 1));
        assert!(matches!(err, Err(Error::Decode(_))));
        let err = full.crop(PixelRect::new(0, 1, 1, 2));
        assert!(matches!(err, Err(Error::Decode(_))));
    }
}
