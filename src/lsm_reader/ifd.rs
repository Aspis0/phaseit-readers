use crate::error::{Error, Result};
use crate::lsm_bytes::{read_u16, read_u32};
use crate::lsm_cz_info::CzTag;
use crate::types::Endian;

use super::strips::PageStrips;
use super::values::{overflow, read_int_array, read_scalar, tiff_type_size};

pub(super) const TAG_CZ_LSM_INFO: u16 = 34412;

pub(super) const TAG_NEW_SUBFILE_TYPE: u16 = 254;

pub(super) const TAG_IMAGE_WIDTH: u16 = 256;

pub(super) const TAG_IMAGE_LENGTH: u16 = 257;

pub(super) const TAG_BITS_PER_SAMPLE: u16 = 258;

pub(super) const TAG_COMPRESSION: u16 = 259;

pub(super) const TAG_STRIP_OFFSETS: u16 = 273;

pub(super) const TAG_SAMPLES_PER_PIXEL: u16 = 277;

pub(super) const TAG_STRIP_BYTE_COUNTS: u16 = 279;

pub(super) const TAG_PLANAR_CONFIGURATION: u16 = 284;

pub(super) const TAG_SAMPLE_FORMAT: u16 = 339;

pub(super) struct IfdProps {
    pub(super) reduced: bool,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) samples: u16,
    pub(super) bits: u16,
    pub(super) sample_format: u16,
    pub(super) planar: bool,
    pub(super) compression: u16,
}

pub(super) struct FirstPage {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) samples: u16,
    pub(super) bits: u16,
    pub(super) sample_format: u16,
    pub(super) planar: bool,
    pub(super) compression: u16,
    pub(super) strips: usize,
}

/// LSM is classic TIFF: BigTIFF does not exist for it, so only magic `42` is accepted.
pub(super) fn read_tiff_header(buf: &[u8]) -> Result<Endian> {
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

pub(super) fn parse_ifd(
    buf: &[u8],
    ifd_off: usize,
    e: Endian,
) -> Result<(PageStrips, IfdProps, Option<CzTag>, u64)> {
    let count = read_u16(buf, ifd_off, e)? as usize;
    let entries_start = ifd_off + 2;
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
        match id {
            TAG_NEW_SUBFILE_TYPE => {
                // Bits 0 (reduced) and 2 (mask) mark non-planes, as in `tiff_reader::is_reduced`; bit 1 marks planes.
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
                // CZ-LSM-Info is far larger than 4 bytes, so its value field must be an offset, never inline.
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
