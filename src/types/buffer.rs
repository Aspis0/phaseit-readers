use byteorder::{BigEndian, LittleEndian, ReadBytesExt};

use crate::error::{Error, Result};

use super::pixel::{host_endian, DType, Endian};

#[derive(Debug, Clone, PartialEq)]
pub enum PlaneBuf {
    U8(Vec<u8>),
    U16(Vec<u16>),
    I16(Vec<i16>),
    F32(Vec<f32>),
}

impl PlaneBuf {
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

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Decodes little- or big-endian bytes into a native-endian buffer, byteswapping on a host of the other order.
    /// Errors with [`Error::Decode`] if `raw.len()` is not a multiple of `dtype.size_bytes()`.
    // Plain `%`: `is_multiple_of` (Rust 1.87) would work, but the crate builds on older toolchains.
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

    /// Builds a buffer from bytes already in the host's order: a decoder that produces native samples is not swapped
    /// twice.
    /// Errors with [`Error::Decode`] if `raw.len()` is not a multiple of `dtype.size_bytes()`.
    pub(crate) fn from_native_bytes(dtype: DType, raw: &[u8]) -> Result<PlaneBuf> {
        Self::from_bytes(dtype, host_endian(), raw)
    }

    /// Takes over an owned little-endian byte vector as the buffer itself: a U8 vector becomes
    /// the buffer with no copy, the wider samples are converted into their own vectors.
    /// Errors with [`Error::Decode`] if `raw.len()` is not a multiple of `dtype.size_bytes()`.
    // Plain `%`: `is_multiple_of` (Rust 1.87) would work, but the crate builds on older toolchains.
    #[allow(clippy::manual_is_multiple_of)]
    pub(crate) fn from_vec(dtype: DType, raw: Vec<u8>) -> Result<PlaneBuf> {
        let sz = dtype.size_bytes();
        if raw.len() % sz != 0 {
            return Err(Error::Decode(format!(
                "raw byte length {} is not a multiple of {:?} sample size {}",
                raw.len(),
                dtype,
                sz
            )));
        }
        let mut cur = &raw[..];
        Ok(match dtype {
            DType::U8 => PlaneBuf::U8(raw),
            DType::U16 => {
                let mut out = vec![0u16; raw.len() / 2];
                cur.read_u16_into::<LittleEndian>(&mut out)?;
                PlaneBuf::U16(out)
            }
            DType::I16 => {
                let mut out = vec![0i16; raw.len() / 2];
                cur.read_i16_into::<LittleEndian>(&mut out)?;
                PlaneBuf::I16(out)
            }
            DType::F32 => {
                let mut out = vec![0f32; raw.len() / 4];
                cur.read_f32_into::<LittleEndian>(&mut out)?;
                PlaneBuf::F32(out)
            }
        })
    }
}
