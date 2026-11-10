// SPDX-License-Identifier: Apache-2.0
//! JPEG-XR decode for CZI compression id 4 via FFI to the vendored jxrlib, built by `build.rs` behind `jpegxr`.
//! Only [`decode_jpegxr`] is public; every `unsafe` line and raw jxrlib type lives in the private `ffi` module.
//! The RAII guard releases every C object on every path: a decode error never leaks or panics across the FFI.
//! Output is little-endian (jxrlib emits host order): a big-endian host would need jxrlib built with `-D_BIG__ENDIAN_`.

use crate::error::{Error, Result};
use crate::types::{DType, Dims5};

mod ffi;

#[cfg(test)]
mod tests;

/// Decodes one CZI JPEG-XR codestream into exactly `stored_x * stored_y * samples * size` little-endian bytes.
/// Colour tiles come out in native BGR: the caller swaps to RGB, so this function must NOT swap.
/// Supported: (1, U8) `8bppGray`, (1, U16) `16bppGray` (LE), (3, U8) `24bppBGR`; Gray32Float and Bgr48 are
/// refused as `Unsupported` pending real-sample verification, rather than shipped unverified.
pub(crate) fn decode_jpegxr(
    raw: &[u8],
    stored_x: usize,
    stored_y: usize,
    dtype: DType,
    samples: u32,
) -> Result<Vec<u8>> {
    let (out_fmt, bytes_per_pixel) = pixel_format(samples, dtype)?;
    let tile_dims = Dims5 {
        t: 1,
        c: 1,
        z: 1,
        y: stored_y,
        x: stored_x,
    };
    let expected = tile_dims.checked_plane_bytes_samples(dtype, samples)?;
    let tile = tile_geometry(stored_x, stored_y, bytes_per_pixel, expected)?;

    let mut out = vec![0u8; expected];
    let mut guard = ffi::DecodeGuard::new();
    run_decoder(&mut guard, raw, &mut out, &tile, out_fmt)?;
    Ok(out)
}

fn pixel_format(samples: u32, dtype: DType) -> Result<(ffi::PixelFormatGuid, usize)> {
    match (samples, dtype) {
        (1, DType::U8) => Ok((ffi::GUID_8BPP_GRAY, 1)),
        (1, DType::U16) => Ok((ffi::GUID_16BPP_GRAY, 2)),
        // TODO: verify 32bppGrayFloat against a real sample before enabling; reject until then.
        (1, DType::F32) => Err(Error::Unsupported(
            "CZI JpegXR Gray32Float not yet supported".to_string(),
        )),
        (3, DType::U8) => Ok((ffi::GUID_24BPP_BGR, 3)),
        // TODO: Bgr48 needs a 48bppRGB decode and an in-decoder R/B swap, unverifiable without a sample.
        (3, DType::U16) => Err(Error::Unsupported(
            "CZI JpegXR Bgr48 not yet supported".to_string(),
        )),
        _ => Err(Error::Unsupported(format!(
            "CZI JpegXR unsupported pixel layout: {samples} sample(s) of {dtype:?}"
        ))),
    }
}

struct Tile {
    width: i32,
    height: i32,
    stride: u32,
}

fn tile_geometry(
    stored_x: usize,
    stored_y: usize,
    bytes_per_pixel: usize,
    expected: usize,
) -> Result<Tile> {
    // jxlib takes width and height as i32 and stride as u32: reject what cannot be represented.
    let width = i32::try_from(stored_x)
        .map_err(|_| Error::Decode("CZI JpegXR tile width too large".to_string()))?;
    let height = i32::try_from(stored_y)
        .map_err(|_| Error::Decode("CZI JpegXR tile height too large".to_string()))?;
    let stride: u32 = stored_x
        .checked_mul(bytes_per_pixel)
        .and_then(|s| u32::try_from(s).ok())
        .ok_or_else(|| Error::Decode("CZI JpegXR tile stride overflow".to_string()))?;

    // Checked against the guarded size: a mismatch means a bad mapping, so it fails loudly.
    if (height as usize).checked_mul(stride as usize) != Some(expected) {
        return Err(Error::Decode(
            "CZI JpegXR internal stride/size mismatch".to_string(),
        ));
    }
    Ok(Tile {
        width,
        height,
        stride,
    })
}

fn run_decoder(
    guard: &mut ffi::DecodeGuard,
    raw: &[u8],
    out: &mut [u8],
    tile: &Tile,
    out_fmt: ffi::PixelFormatGuid,
) -> Result<()> {
    let &Tile {
        width,
        height,
        stride,
    } = tile;

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
    Ok(())
}
