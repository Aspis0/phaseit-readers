// SPDX-License-Identifier: Apache-2.0
//! JPEG-XR decode for CZI compressed subblocks (compression id 4), via FFI to
//! the vendored jxrlib (glencoesoftware fork, BSD) built by `build.rs` behind the
//! opt-in `jpegxr` cargo feature.
//!
//! Only [`decode_jpegxr`] is public to the crate. Every `unsafe` line and every
//! raw jxrlib type lives in the private [`ffi`] submodule below; the safe wrapper
//! owns all lifetimes, checks every `ERR`, and cleans up on every path (including
//! errors) through an RAII guard, so a decode failure can never leak the C
//! objects or the memory stream and can never panic across the FFI boundary.
//!
//! Endianness: jxrlib emits multi-byte samples in the host's native byte order.
//! We target little-endian hosts (as the rest of the CZI reader does), so the
//! returned bytes are little-endian, matching the `decode_tile` contract. A
//! big-endian host would additionally need jxrlib built with `-D_BIG__ENDIAN_`.

use crate::error::{Error, Result};
use crate::types::{DType, Dims5};

/// Decode a complete JPEG-XR codestream (`raw`) for one CZI tile into
/// little-endian, interleaved sample bytes in the container's NATIVE order.
///
/// The returned buffer is exactly `stored_x * stored_y * samples *
/// dtype.size_bytes()` bytes. For color tiles (`samples == 3`) the samples are
/// in native **BGR** order — the caller (`decode_tile`) performs the BGR→RGB
/// swap afterwards, so this function must NOT swap.
///
/// `dtype` is the per-sample type and `samples` the samples-per-pixel (1 for
/// grayscale, 3 for color), matching how the rest of the CZI reader models
/// pixels. Supported layouts (CZI PixelType → jxrlib output format):
///
/// | samples | dtype | jxrlib out format         | bytes/px |
/// |---------|-------|---------------------------|----------|
/// | 1       | U8    | `8bppGray`                | 1        |
/// | 1       | U16   | `16bppGray` (LE)          | 2        |
/// | 3       | U8    | `24bppBGR` (native BGR)   | 3        |
///
/// Two layouts are recognized but deliberately rejected with [`Error::Unsupported`]
/// pending real-sample verification, rather than shipped unverified:
/// - Gray32Float (`1 × F32`): JPEG-XR float is lossy even at "lossless" level, so no
///   byte-exact fixture is possible and no real Gray32Float CZI sample exists yet.
/// - Bgr48 (`3 × U16`): jxrlib has no `48bppBGR` output and the `48bppRGB`→BGR path
///   cannot be verified without a real sample.
pub(crate) fn decode_jpegxr(
    raw: &[u8],
    stored_x: usize,
    stored_y: usize,
    dtype: DType,
    samples: u32,
) -> Result<Vec<u8>> {
    // --- pick the jxlib output pixel format for this CZI pixel layout ---
    let (out_fmt, bytes_per_pixel) = match (samples, dtype) {
        (1, DType::U8) => (ffi::GUID_8BPP_GRAY, 1usize),
        (1, DType::U16) => (ffi::GUID_16BPP_GRAY, 2),
        // TODO: verify 32bppGrayFloat against a real sample before enabling.
        // JPEG-XR float is lossy even at "lossless" level, so no byte-exact fixture
        // is possible and no real Gray32Float CZI sample exists yet. Reject rather
        // than ship an unverified pixel decode.
        (1, DType::F32) => {
            return Err(Error::Unsupported(
                "CZI JpegXR Gray32Float not yet supported".to_string(),
            ))
        }
        (3, DType::U8) => (ffi::GUID_24BPP_BGR, 3),
        // TODO: Bgr48 needs decode to 48bppRGB then an in-decoder R↔B swap
        // (jxrlib has no 48bppBGR). No real/synthetic sample exists to verify
        // channel order, so reject rather than emit unverified pixels.
        (3, DType::U16) => {
            return Err(Error::Unsupported(
                "CZI JpegXR Bgr48 not yet supported".to_string(),
            ))
        }
        _ => {
            return Err(Error::Unsupported(format!(
                "CZI JpegXR unsupported pixel layout: {samples} sample(s) of {dtype:?}"
            )))
        }
    };

    // --- guard the output size BEFORE allocating (same cap the caller uses) ---
    let tile_dims = Dims5 {
        t: 1,
        c: 1,
        z: 1,
        y: stored_y,
        x: stored_x,
    };
    let expected = tile_dims.checked_plane_bytes_samples(dtype, samples)?;

    // jxlib expresses width/height as I32 and the row stride as U32. Reject a
    // tile whose geometry cannot be represented (or whose stride overflows)
    // before touching the C API.
    let width = i32::try_from(stored_x)
        .map_err(|_| Error::Decode("CZI JpegXR tile width too large".to_string()))?;
    let height = i32::try_from(stored_y)
        .map_err(|_| Error::Decode("CZI JpegXR tile height too large".to_string()))?;
    let stride: u32 = stored_x
        .checked_mul(bytes_per_pixel)
        .and_then(|s| u32::try_from(s).ok())
        .ok_or_else(|| Error::Decode("CZI JpegXR tile stride overflow".to_string()))?;

    // Sanity: full-plane byte count must equal the guarded expectation. This
    // holds by construction (`bytes_per_pixel == samples * size_bytes`), but a
    // mismatch would mean a bad mapping, so fail loudly rather than under-fill.
    if (height as usize).checked_mul(stride as usize) != Some(expected) {
        return Err(Error::Decode(
            "CZI JpegXR internal stride/size mismatch".to_string(),
        ));
    }

    // Output buffer, zero-filled; `Copy` writes exactly `height * stride` bytes.
    let mut out = vec![0u8; expected];

    // All raw pointers live for the duration of this call; `raw` (borrowed) and
    // `out` (owned local) both outlive every FFI call below.
    let mut guard = ffi::DecodeGuard::new();

    // SAFETY: every call goes through jxlib's documented PK* API. Each `ERR` is
    // checked (`< 0` == failure) and on the first failure we return early; the
    // `DecodeGuard` Drop then releases whatever was created, in reverse order,
    // with no double-free (each field is null-checked and Release nulls it).
    unsafe {
        ffi::check(
            ffi::PKCreateFactory(&mut guard.factory, ffi::PK_SDK_VERSION),
            "PKCreateFactory",
        )?;
        let factory = &*guard.factory;

        ffi::check(
            (factory.create_stream_from_memory)(
                &mut guard.stream,
                raw.as_ptr() as *mut core::ffi::c_void,
                raw.len(),
            ),
            "CreateStreamFromMemory",
        )?;

        ffi::check(
            ffi::PKImageDecode_Create_WMP(&mut guard.decoder),
            "PKImageDecode_Create_WMP",
        )?;
        let decoder = &*guard.decoder;

        ffi::check(
            (decoder.initialize)(guard.decoder, guard.stream),
            "PKImageDecode.Initialize",
        )?;

        // Validate the codestream geometry matches the directory-declared tile.
        let mut w: i32 = 0;
        let mut h: i32 = 0;
        ffi::check(
            (decoder.get_size)(guard.decoder, &mut w, &mut h),
            "PKImageDecode.GetSize",
        )?;
        if w != width || h != height {
            return Err(Error::Decode(format!(
                "CZI JpegXR tile is {w}x{h}, expected {width}x{height}"
            )));
        }

        ffi::check(
            ffi::PKCreateCodecFactory(&mut guard.codec_factory, ffi::WMP_SDK_VERSION),
            "PKCreateCodecFactory",
        )?;
        let codec_factory = &*guard.codec_factory;

        ffi::check(
            (codec_factory.create_format_converter)(&mut guard.converter),
            "CreateFormatConverter",
        )?;
        let converter = &*guard.converter;

        // pExt is NULL: jxlib derives the source format from the decoder.
        ffi::check(
            (converter.initialize)(
                guard.converter,
                guard.decoder,
                core::ptr::null_mut(),
                out_fmt,
            ),
            "PKFormatConverter.Initialize",
        )?;

        let rect = ffi::PkRect {
            x: 0,
            y: 0,
            width,
            height,
        };
        ffi::check(
            (converter.copy)(guard.converter, &rect, out.as_mut_ptr(), stride),
            "PKFormatConverter.Copy",
        )?;
    }

    // `guard` drops here (or on any early `?`), releasing all C resources.
    Ok(out)
}

/// Raw jxlib FFI: hand-written, minimal — only the PK* vtable slots and exported
/// functions [`decode_jpegxr`] actually calls, plus the pixel-format GUIDs.
///
/// The vtable structs mirror the leading `ERR (*...)` function-pointer members of
/// jxlib's structs in EXACT declaration order (from vendored `jxrgluelib/
/// JXRGlue.h`). Slots we never call are typed as opaque pointers so their only
/// contract is "pointer-sized, at this offset". Trailing data members are omitted
/// because the C side allocates every object (via `calloc`) and we only ever read
/// these function-pointer fields — never construct or index past them in Rust.
mod ffi {
    use crate::error::{Error, Result};
    use core::ffi::{c_char, c_long, c_void};

    /// jxlib `ERR` is C `long` (4 bytes on MSVC/LLP64, 8 on LP64). `< 0` == fail,
    /// `WMP_errSuccess == 0`.
    pub type Err = c_long;

    /// `PK_SDK_VERSION` / `WMP_SDK_VERSION` from JXRGlue.h.
    pub const PK_SDK_VERSION: u32 = 0x0101;
    pub const WMP_SDK_VERSION: u32 = 0x0101;

    /// Map a jxlib `ERR` into a crate error; `msg` names the failing call.
    pub fn check(err: Err, msg: &str) -> Result<()> {
        if err < 0 {
            Err(Error::Decode(format!("CZI JpegXR: {msg} failed (jxrlib ERR {err})")))
        } else {
            Ok(())
        }
    }

    /// `GUID` (== `PKPixelFormatGUID`) from common/include/guiddef.h: 16 bytes,
    /// `{u32, u16, u16, [u8; 8]}`. Passed BY VALUE to `PKFormatConverter.Initialize`.
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct PixelFormatGuid {
        pub data1: u32,
        pub data2: u16,
        pub data3: u16,
        pub data4: [u8; 8],
    }

    // Pixel-format GUIDs, byte-for-byte from JXRGlue.h `DEFINE_GUID(...)`. The
    // shared prefix is {0x6fddc324, 0x4e03, 0x4bfe, b1 85 3d 77 76 8d c9 XX};
    // only the final byte distinguishes these formats.
    const GUID_PREFIX_1: u32 = 0x6fdd_c324;
    const GUID_PREFIX_2: u16 = 0x4e03;
    const GUID_PREFIX_3: u16 = 0x4bfe;
    const fn pixfmt(last: u8) -> PixelFormatGuid {
        PixelFormatGuid {
            data1: GUID_PREFIX_1,
            data2: GUID_PREFIX_2,
            data3: GUID_PREFIX_3,
            data4: [0xb1, 0x85, 0x3d, 0x77, 0x76, 0x8d, 0xc9, last],
        }
    }
    /// `GUID_PKPixelFormat8bppGray`  (…c9,0x08)
    pub const GUID_8BPP_GRAY: PixelFormatGuid = pixfmt(0x08);
    /// `GUID_PKPixelFormat16bppGray` (…c9,0x0b)
    pub const GUID_16BPP_GRAY: PixelFormatGuid = pixfmt(0x0b);
    /// `GUID_PKPixelFormat24bppBGR`  (…c9,0x0c)
    pub const GUID_24BPP_BGR: PixelFormatGuid = pixfmt(0x0c);
    // `GUID_PKPixelFormat32bppGrayFloat` (…c9,0x11) intentionally omitted: the
    // Gray32Float path is rejected pending real-sample verification. Re-add
    // `pub const GUID_32BPP_GRAY_FLOAT: PixelFormatGuid = pixfmt(0x11);` when enabled.

    /// `PKRect { I32 X, Y, Width, Height }`.
    #[repr(C)]
    pub struct PkRect {
        pub x: i32,
        pub y: i32,
        pub width: i32,
        pub height: i32,
    }

    /// Opaque memory stream. We never dereference it in Rust; it is created by
    /// `CreateStreamFromMemory` and freed by the exported `CloseWS_Memory`.
    #[repr(C)]
    pub struct WmpStream {
        _opaque: [u8; 0],
    }

    /// `tagPKFactory` leading vtable (JXRGlue.h).
    #[repr(C)]
    pub struct PkFactory {
        create_stream: *const c_void,
        create_stream_from_filename: *const c_void,
        pub create_stream_from_memory:
            unsafe extern "C" fn(*mut *mut WmpStream, *mut c_void, usize) -> Err,
        pub release: unsafe extern "C" fn(*mut *mut PkFactory) -> Err,
    }

    /// `tagPKCodecFactory` leading vtable (JXRGlue.h).
    #[repr(C)]
    pub struct PkCodecFactory {
        create_codec: *const c_void,
        create_decoder_from_file: *const c_void,
        pub create_format_converter: unsafe extern "C" fn(*mut *mut PkFormatConverter) -> Err,
        pub release: unsafe extern "C" fn(*mut *mut PkCodecFactory) -> Err,
    }

    /// `tagPKImageDecode` leading vtable (JXRGlue.h). Only Initialize / GetSize /
    /// Release are typed precisely; the rest are opaque slots for correct offset.
    #[repr(C)]
    pub struct PkImageDecode {
        pub initialize: unsafe extern "C" fn(*mut PkImageDecode, *mut WmpStream) -> Err,
        get_pixel_format: *const c_void,
        pub get_size: unsafe extern "C" fn(*mut PkImageDecode, *mut i32, *mut i32) -> Err,
        get_resolution: *const c_void,
        get_color_context: *const c_void,
        get_descriptive_metadata: *const c_void,
        get_raw_stream: *const c_void,
        copy: *const c_void,
        get_frame_count: *const c_void,
        select_frame: *const c_void,
        pub release: unsafe extern "C" fn(*mut *mut PkImageDecode) -> Err,
        // trailing data members intentionally omitted (never accessed in Rust)
    }

    /// `tagPKFormatConverter` leading vtable (JXRGlue.h). Only Initialize / Copy /
    /// Release are typed precisely. Initialize takes the target GUID BY VALUE.
    #[repr(C)]
    pub struct PkFormatConverter {
        pub initialize: unsafe extern "C" fn(
            *mut PkFormatConverter,
            *mut PkImageDecode,
            *mut c_char,
            PixelFormatGuid,
        ) -> Err,
        initialize_convert: *const c_void,
        get_pixel_format: *const c_void,
        get_source_pixel_format: *const c_void,
        get_size: *const c_void,
        get_resolution: *const c_void,
        pub copy: unsafe extern "C" fn(*mut PkFormatConverter, *const PkRect, *mut u8, u32) -> Err,
        convert: *const c_void,
        pub release: unsafe extern "C" fn(*mut *mut PkFormatConverter) -> Err,
        // trailing data members intentionally omitted (never accessed in Rust)
    }

    extern "C" {
        #[link_name = "PKCreateFactory"]
        pub fn PKCreateFactory(pp: *mut *mut PkFactory, sdk_version: u32) -> Err;
        #[link_name = "PKCreateCodecFactory"]
        pub fn PKCreateCodecFactory(pp: *mut *mut PkCodecFactory, sdk_version: u32) -> Err;
        #[link_name = "PKImageDecode_Create_WMP"]
        pub fn PKImageDecode_Create_WMP(pp: *mut *mut PkImageDecode) -> Err;
        /// Frees a memory stream created by `CreateStreamFromMemory`. The decoder
        /// does NOT own it (its `fStreamOwner` stays false when we attach the
        /// stream ourselves), so we must close it explicitly.
        #[link_name = "CloseWS_Memory"]
        pub fn CloseWS_Memory(pp: *mut *mut WmpStream) -> Err;
    }

    /// RAII owner of every jxlib object created during a decode. Drop releases
    /// them in reverse dependency order (converter → decoder → stream → codec
    /// factory → factory). Each field is null-checked; jxlib's Release nulls the
    /// pointer, so a second drop is a no-op — no double free, no leak on any path.
    pub struct DecodeGuard {
        pub factory: *mut PkFactory,
        pub stream: *mut WmpStream,
        pub decoder: *mut PkImageDecode,
        pub codec_factory: *mut PkCodecFactory,
        pub converter: *mut PkFormatConverter,
    }

    impl DecodeGuard {
        pub fn new() -> Self {
            DecodeGuard {
                factory: core::ptr::null_mut(),
                stream: core::ptr::null_mut(),
                decoder: core::ptr::null_mut(),
                codec_factory: core::ptr::null_mut(),
                converter: core::ptr::null_mut(),
            }
        }
    }

    impl Drop for DecodeGuard {
        fn drop(&mut self) {
            // SAFETY: each pointer is either null (skipped) or a live jxlib object
            // this guard created. Release takes `**` and nulls the pointee. The
            // decoder never owns the stream (we set it, leaving fStreamOwner
            // false), so the stream is closed here exactly once, after the decoder
            // that referenced it is gone.
            unsafe {
                if !self.converter.is_null() {
                    ((*self.converter).release)(&mut self.converter);
                }
                if !self.decoder.is_null() {
                    ((*self.decoder).release)(&mut self.decoder);
                }
                if !self.stream.is_null() {
                    CloseWS_Memory(&mut self.stream);
                }
                if !self.codec_factory.is_null() {
                    ((*self.codec_factory).release)(&mut self.codec_factory);
                }
                if !self.factory.is_null() {
                    ((*self.factory).release)(&mut self.factory);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::decode_jpegxr;
    use crate::types::DType;
    use std::path::PathBuf;

    fn fixtures_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jpegxr")
    }

    /// Minimal parser for the fixture manifest.json (pretty-printed by the gen
    /// script; the only nested `[...]` is the `shape` array). Yields, per object,
    /// `(name, dtype, samples, stored_x, stored_y)`.
    fn parse_manifest(txt: &str) -> Vec<(String, DType, u32, usize, usize)> {
        let mut out = Vec::new();
        for chunk in txt.split('{').skip(1) {
            let name = str_field(chunk, "name");
            let dtype = str_field(chunk, "dtype");
            let (Some(name), Some(dtype)) = (name, dtype) else {
                continue;
            };
            let dtype = match dtype.as_str() {
                "u8" => DType::U8,
                "u16" => DType::U16,
                "f32" => DType::F32,
                other => panic!("unknown dtype in manifest: {other}"),
            };
            let samples = uint_field(chunk, "samples").unwrap_or(1) as u32;
            let stored_x = uint_field(chunk, "stored_x").expect("stored_x");
            let stored_y = uint_field(chunk, "stored_y").expect("stored_y");
            out.push((name, dtype, samples, stored_x, stored_y));
        }
        out
    }

    fn str_field(chunk: &str, key: &str) -> Option<String> {
        let pat = format!("\"{key}\"");
        let i = chunk.find(&pat)?;
        let rest = &chunk[i + pat.len()..];
        let colon = rest.find(':')?;
        let after = &rest[colon + 1..];
        let q1 = after.find('"')?;
        let after = &after[q1 + 1..];
        let q2 = after.find('"')?;
        Some(after[..q2].to_string())
    }

    fn uint_field(chunk: &str, key: &str) -> Option<usize> {
        let pat = format!("\"{key}\"");
        let i = chunk.find(&pat)?;
        let rest = &chunk[i + pat.len()..];
        let colon = rest.find(':')?;
        let after = rest[colon + 1..].trim_start();
        let end = after
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after.len());
        after[..end].parse().ok()
    }

    /// Decode every tile listed in `dir/manifest_name` and assert the bytes match
    /// the sibling `.raw` ground truth exactly. Returns the number of tiles checked.
    fn verify_fixture_dir(dir: &std::path::Path, manifest_name: &str) -> usize {
        let manifest = std::fs::read_to_string(dir.join(manifest_name))
            .unwrap_or_else(|e| panic!("read {manifest_name}: {e}"));
        let entries = parse_manifest(&manifest);
        assert!(!entries.is_empty(), "{manifest_name} parsed no entries");
        let n = entries.len();

        for (name, dtype, samples, stored_x, stored_y) in entries {
            let jxr = std::fs::read(dir.join(format!("{name}.jxr")))
                .unwrap_or_else(|e| panic!("read {name}.jxr: {e}"));
            let expected = std::fs::read(dir.join(format!("{name}.raw")))
                .unwrap_or_else(|e| panic!("read {name}.raw: {e}"));

            let got = decode_jpegxr(&jxr, stored_x, stored_y, dtype, samples)
                .unwrap_or_else(|e| panic!("decode_jpegxr({name}) failed: {e}"));

            assert_eq!(
                got.len(),
                expected.len(),
                "{name}: decoded length {} != ground-truth {}",
                got.len(),
                expected.len()
            );
            assert!(
                got == expected,
                "{name}: decoded bytes differ from ground truth (dtype {dtype:?}, \
                 samples {samples}, {stored_x}x{stored_y})"
            );
        }
        n
    }

    #[test]
    fn jpegxr_fixtures_match_ground_truth() {
        // Synthetic imagecodecs-encoded fixtures (committed, always present).
        verify_fixture_dir(&fixtures_dir(), "manifest.json");

        // Real Zeiss Axioscan JPEG-XR tiles: opt-in and NOT committed (derived from
        // a 673 MB third-party sample). They are extracted from that sample into
        // this folder by hand. When present, this proves the FFI decodes real
        // Zeiss codestreams byte-exact (varied real tile sizes, not just imagecodecs
        // synthetic streams), and skips cleanly otherwise so plain checkouts pass.
        let real =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jpegxr_real");
        if real.join("manifest_real.json").is_file() {
            let n = verify_fixture_dir(&real, "manifest_real.json");
            eprintln!("verified {n} real Axioscan JPEG-XR tiles against imagecodecs");
        }
    }
}
