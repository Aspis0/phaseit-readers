// SPDX-License-Identifier: Apache-2.0
//! The geometry of an ICS scene: the axis list with its sizes, the strides a sample offset follows, the sample type
//! and byte order, and where the pixels are. All of it is settled before the first sample is decoded.

use crate::error::{Error, Result};
use crate::types::{DType, Dims5, Endian};

use super::header::{IcsHeader, Storage};

/// `representation compression gzip`: the stream has no index, so a plane is inflated from its first byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Compression {
    None,
    Gzip,
}

pub(super) struct Layout {
    pub dims: Dims5,
    pub dtype: DType,
    pub endian: Endian,
    compression: Compression,
    /// Byte offset of the first sample: the end of the header, or 0 for a `.ids` file of its own.
    data_offset: usize,
    axes: Vec<String>,
    /// Strides in samples, aligned with `axes`.
    strides: Vec<usize>,
    /// x and y are the two fastest axes, so a plane is contiguous rows rather than gathered sample by sample.
    contiguous: bool,
}

impl Layout {
    pub fn compression(&self) -> Compression {
        self.compression
    }

    /// Byte offset of the file's first sample; a deflated stream is inflated from here too.
    pub fn data_offset(&self) -> usize {
        self.data_offset
    }

    /// Sample offset of the plane at `(t, c, z)` from the first sample of the stream; an absent axis has no stride.
    pub fn plane_samples(&self, t: usize, c: usize, z: usize) -> Option<usize> {
        let mut offset = 0usize;
        for (index, axis) in [(t, "t"), (c, "ch"), (z, "z")] {
            offset = offset.checked_add(index.checked_mul(self.stride(axis).unwrap_or(0))?)?;
        }
        Some(offset)
    }

    /// Samples from a plane's first sample to its last: one block covers the plane, gathered or contiguous.
    pub fn plane_span_samples(&self) -> Option<usize> {
        let rows = self.dims.y.checked_sub(1)?.checked_mul(self.stride("y")?)?;
        let columns = self.dims.x.checked_sub(1)?.checked_mul(self.stride("x")?)?;
        rows.checked_add(columns)?.checked_add(1)
    }

    /// Bytes one plane needs in memory.
    pub fn plane_bytes(&self) -> Result<usize> {
        let plane = Dims5 {
            t: 1,
            c: 1,
            z: 1,
            y: self.dims.y,
            x: self.dims.x,
        };
        plane.checked_plane_bytes(self.dtype)
    }

    pub fn stride(&self, axis: &str) -> Option<usize> {
        let index = self.axes.iter().position(|name| name == axis)?;
        self.strides.get(index).copied()
    }

    pub fn contiguous(&self) -> bool {
        self.contiguous
    }
}

/// `file_bytes` is the length of the file the pixels live in; `None` for a deflated stream, whose length only the
/// data itself knows.
pub(super) fn layout(header: &IcsHeader, file_bytes: Option<usize>) -> Result<Layout> {
    let dtype = dtype_of(header)?;
    let endian = endian_of(header, dtype)?;
    let compression = compression_of(header)?;
    let sizes = extents(header)?;
    let strides = strides(&sizes)?;
    let dims = dims_of(&header.axes, &sizes, dtype)?;
    let contiguous = header.axes.first().map(String::as_str) == Some("x")
        && header.axes.get(1).map(String::as_str) == Some("y");
    if compression == Compression::Gzip && !contiguous {
        return Err(Error::Unsupported(
            "ICS gzip data whose plane is not contiguous (x and y are not the two fastest axes): serving a plane \
             would mean inflating the whole stream to gather its samples"
                .to_string(),
        ));
    }
    // Stored pixels are a file of their own, so their first sample is that file's byte 0.
    let data_offset = match header.storage {
        Storage::Ids => 0,
        Storage::Inline => header.data_offset,
    };
    if compression == Compression::None {
        if let Some(file) = file_bytes {
            check_extent(&strides, &sizes, dtype, data_offset, file)?;
        }
    }
    Ok(Layout {
        dims,
        dtype,
        endian,
        compression,
        data_offset,
        axes: header.axes.clone(),
        strides,
        contiguous,
    })
}

/// The data the layout needs must lie inside the file: checked before the first sample is read.
fn check_extent(
    strides: &[usize],
    sizes: &[usize],
    dtype: DType,
    data_offset: usize,
    file: usize,
) -> Result<()> {
    let bytes = total_bytes(strides, sizes, dtype)?;
    let end = data_offset
        .checked_add(bytes)
        .ok_or_else(|| Error::Parse("ICS data size overflows usize".to_string()))?;
    if end > file {
        return Err(Error::Parse(format!(
            "ICS data ends at byte {end} ({} sample(s) of {} byte(s) at offset {data_offset}), past the end of the \
             {file}-byte file",
            bytes / dtype.size_bytes(),
            dtype.size_bytes()
        )));
    }
    Ok(())
}

/// Every axis size is positive: a zero extent would make the axes after it share one stride.
fn extents(header: &IcsHeader) -> Result<Vec<usize>> {
    for (axis, size) in header.axes.iter().zip(&header.sizes) {
        if *size == 0 {
            return Err(Error::Unsupported(format!(
                "ICS layout size of axis {axis:?} is 0: an image extent must be positive"
            )));
        }
    }
    Ok(header.sizes.clone())
}

/// The stride of the fastest axis is 1, and each next stride is the previous one times the previous size.
fn strides(sizes: &[usize]) -> Result<Vec<usize>> {
    let mut out = Vec::with_capacity(sizes.len());
    let mut stride = 1usize;
    for size in sizes {
        out.push(stride);
        stride = stride
            .checked_mul(*size)
            .ok_or_else(|| Error::Parse("ICS layout strides overflow usize".to_string()))?;
    }
    Ok(out)
}

/// Every sample the layout describes, and the bytes they need: checked, because the sizes come from the header.
fn total_bytes(strides: &[usize], sizes: &[usize], dtype: DType) -> Result<usize> {
    let samples = strides
        .last()
        .copied()
        .unwrap_or(1)
        .checked_mul(*sizes.last().unwrap_or(&1))
        .ok_or_else(|| Error::Parse("ICS sample count overflows usize".to_string()))?;
    samples
        .checked_mul(dtype.size_bytes())
        .ok_or_else(|| Error::Parse("ICS data size overflows usize".to_string()))
}

/// The one scene: an absent axis has size 1, so the plane is what the axes the file states leave.
fn dims_of(axes: &[String], sizes: &[usize], dtype: DType) -> Result<Dims5> {
    let mut dims = Dims5 {
        t: 1,
        c: 1,
        z: 1,
        y: 1,
        x: 1,
    };
    for (axis, size) in axes.iter().zip(sizes) {
        let slot = match axis.as_str() {
            "t" => &mut dims.t,
            "c" | "ch" => &mut dims.c,
            "z" => &mut dims.z,
            "y" => &mut dims.y,
            "x" => &mut dims.x,
            // `axis_names` refused an unknown name already; this arm keeps the mapping total.
            other => {
                return Err(Error::Unsupported(format!(
                    "ICS layout names axis {other:?}"
                )))
            }
        };
        *slot = *size;
    }
    // Overflow only: a plane over the ceiling is published here and refused by `plane()`, one at a time.
    dims.plane_bytes_no_ceiling(dtype)?;
    Ok(dims)
}

/// The sample type: only the four combinations the crate has a [`DType`] for are read.
fn dtype_of(header: &IcsHeader) -> Result<DType> {
    match (header.format.as_str(), header.sign.as_str(), header.bits) {
        ("integer", "unsigned", 8) => Ok(DType::U8),
        ("integer", "unsigned", 16) => Ok(DType::U16),
        ("integer", "signed", 16) => Ok(DType::I16),
        ("real", _, 32) => Ok(DType::F32),
        _ => Err(Error::Unsupported(format!(
            "ICS representation format {} sign {} with {} bits per sample is a sample type this crate cannot \
             represent (it reads unsigned 8 and 16, signed 16 and real 32)",
            header.format, header.sign, header.bits
        ))),
    }
}

/// `byte_order` lists the sample's bytes in file order: `1 … k` is little-endian, `k … 2 1` big-endian.
/// `dtype_of` accepted only whole-byte sample types, so the byte count below is the layout's own.
fn endian_of(header: &IcsHeader, dtype: DType) -> Result<Endian> {
    let bytes = dtype.size_bytes();
    let stated = header.byte_order.as_deref().unwrap_or("");
    let digits = stated
        .split_whitespace()
        .map(|v| {
            v.parse::<u32>().map_err(|_| {
                Error::Parse(format!("ICS byte_order value {v:?} is not a byte number"))
            })
        })
        .collect::<Result<Vec<u32>>>()?;
    let little: Vec<u32> = (1..=bytes as u32).collect();
    let big: Vec<u32> = (1..=bytes as u32).rev().collect();
    let order = match digits.as_slice() {
        [] => None,
        digits if digits == little => Some(Endian::Little),
        digits if digits == big => Some(Endian::Big),
        _ => {
            return Err(Error::Unsupported(format!(
                "ICS byte_order {stated:?} is neither 1..{bytes} (little-endian) nor {bytes}..1 (big-endian)"
            )))
        }
    };
    match (order, bytes) {
        (Some(endian), _) => Ok(endian),
        // A one-byte sample has no order to state, so an absent line is unambiguous.
        (None, 1) => Ok(Endian::Little),
        (None, _) => Err(Error::Unsupported(format!(
            "ICS representation states no byte_order for its {bytes}-byte samples"
        ))),
    }
}

fn compression_of(header: &IcsHeader) -> Result<Compression> {
    match header.compression.as_str() {
        "uncompressed" => Ok(Compression::None),
        "gzip" => Ok(Compression::Gzip),
        other => Err(Error::Unsupported(format!(
            "ICS compression {other:?} is not one this reader decodes (uncompressed and gzip)"
        ))),
    }
}
