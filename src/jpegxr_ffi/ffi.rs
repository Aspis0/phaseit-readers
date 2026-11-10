//! Hand-written raw jxlib FFI: only the vtable slots and functions `decode_jpegxr` calls, plus the pixel-format GUIDs.
//! Vtable structs mirror jxrgluelib's `ERR` members in exact declaration order; slots never called are opaque pointers.

use crate::error::{Error, Result};
use core::ffi::{c_char, c_long, c_void};

/// jxlib `ERR` is C `long` (4 bytes on MSVC, 8 on LP64): `< 0` is failure.
pub type Err = c_long;

pub const PK_SDK_VERSION: u32 = 0x0101;
pub const WMP_SDK_VERSION: u32 = 0x0101;

pub fn check(err: Err, msg: &str) -> Result<()> {
    if err < 0 {
        Err(Error::Decode(format!("CZI JpegXR: {msg} failed (jxrlib ERR {err})")))
    } else {
        Ok(())
    }
}

/// `GUID` from `guiddef.h`, passed BY VALUE to `PKFormatConverter.Initialize`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PixelFormatGuid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

// Pixel-format GUIDs from JXRGlue.h: they share a prefix, and only the last byte differs.
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
pub const GUID_8BPP_GRAY: PixelFormatGuid = pixfmt(0x08);
pub const GUID_16BPP_GRAY: PixelFormatGuid = pixfmt(0x0b);
pub const GUID_24BPP_BGR: PixelFormatGuid = pixfmt(0x0c);
// No Gray32Float GUID: that path is rejected until a real sample verifies it.

#[repr(C)]
pub struct PkRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[repr(C)]
pub struct WmpStream {
    _opaque: [u8; 0],
}

#[repr(C)]
pub struct PkFactory {
    create_stream: *const c_void,
    create_stream_from_filename: *const c_void,
    pub create_stream_from_memory:
        unsafe extern "C" fn(*mut *mut WmpStream, *mut c_void, usize) -> Err,
    pub release: unsafe extern "C" fn(*mut *mut PkFactory) -> Err,
}

#[repr(C)]
pub struct PkCodecFactory {
    create_codec: *const c_void,
    create_decoder_from_file: *const c_void,
    pub create_format_converter: unsafe extern "C" fn(*mut *mut PkFormatConverter) -> Err,
    pub release: unsafe extern "C" fn(*mut *mut PkCodecFactory) -> Err,
}

/// Only Initialize, GetSize and Release are typed; the rest are opaque slots that keep the offsets right.
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
    /// The decoder does not own the stream (`fStreamOwner` stays false), so it is closed explicitly.
    #[link_name = "CloseWS_Memory"]
    pub fn CloseWS_Memory(pp: *mut *mut WmpStream) -> Err;
}

/// RAII owner of every jxlib object of one decode: Release nulls the pointer, so a second drop is a no-op.
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
