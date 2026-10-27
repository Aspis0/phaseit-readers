// SPDX-License-Identifier: Apache-2.0
//! The CZ-LSM-Info struct: what an LSM file states about itself.
//!
//! The struct is a private binary block that the file's first IFD points at
//! through the TIFF tag 34412. Its layout is fixed:
//!
//! | offset | field |
//! |--------|-------|
//! | 0      | the magic `0x0400494C` |
//! | 4      | the struct's size in bytes |
//! | 8..28  | `DimensionX`, `DimensionY`, `DimensionZ`, `DimensionChannels`, `DimensionTime` |
//! | 40..64 | the voxel sizes X, Y and Z (f64, metres) |
//! | 108    | the file offset of the channel-colours block |
//! | 264    | `DimensionP` (stage positions) |
//! | 268    | `DimensionM` (tiles) |
//!
//! The struct is only as long as it says it is. The TIFF tag declares a byte
//! count and the struct repeats that count at offset 4; the **smaller** of the
//! two is the struct's length. A field beyond that length is *absent*: the
//! file states no tile count and no channel-colours block, so the reader
//! publishes one scene and unnamed channels. Reading past the struct would
//! read the strip data that follows it and take it for metadata.

use crate::error::{Error, Result};
use crate::lsm_bytes::{read_f64, read_i32};
use crate::types::{Endian, PxSizeUm};

/// The magic every LSM file's CZ-LSM-Info struct starts with.
const MAGIC: i32 = 0x0400_494C;

/// The struct's header: the magic and the struct's size.
const HEADER_BYTES: usize = 8;

/// `DimensionX` .. `DimensionTime`: five i32s.
const DIMENSIONS_AT: usize = 8;
const DIMENSIONS_END: usize = 28;

/// The three voxel sizes (f64, metres).
const VOXEL_AT: usize = 40;
const VOXEL_END: usize = 64;

/// The file offset of the channel-colours block.
const CHANNEL_COLORS_AT: usize = 108;
const CHANNEL_COLORS_END: usize = 112;

/// `DimensionP` (stage positions) and `DimensionM` (tiles).
const DIM_P_AT: usize = 264;
const DIM_M_AT: usize = 268;
const COUNTS_END: usize = 272;

/// Where the CZ-LSM-Info tag points, and how many bytes it declares.
#[derive(Debug, Clone, Copy)]
pub struct CzTag {
    /// The file offset of the struct.
    pub offset: usize,
    /// The byte count the TIFF tag declares.
    pub declared_bytes: usize,
}

/// The geometry, calibration and pointers the CZ-LSM-Info struct states.
#[derive(Debug, Clone, Copy)]
pub struct CzInfo {
    /// `DimensionX`: the width in pixels.
    pub dim_x: i32,
    /// `DimensionY`: the height in pixels.
    pub dim_y: i32,
    /// `DimensionZ`: the planes per time point.
    pub dim_z: i32,
    /// `DimensionChannels`: the number of channels.
    pub dim_channels: i32,
    /// `DimensionTime`: the number of time points.
    pub dim_time: i32,
    /// `DimensionP`: how many stage positions the file holds.
    pub dim_p: i32,
    /// `DimensionM`: how many tiles the file holds.
    pub dim_m: i32,
    /// The file offset of the channel-colours block, when the struct states
    /// one.
    pub channel_colors_offset: Option<usize>,
    /// The voxel size, when the struct states one.
    pub px_size_um: Option<PxSizeUm>,
}

/// Read the CZ-LSM-Info struct at `tag.offset`.
///
/// The struct is read within the length it declares; a field beyond that
/// length is absent. A struct whose magic is not the LSM magic, or one that
/// reaches past the end of the file, is refused.
pub fn parse(buf: &[u8], tag: CzTag, e: Endian) -> Result<CzInfo> {
    let present = buf.len().checked_sub(tag.offset).ok_or_else(|| {
        Error::Decode(format!(
            "LSM CZ-LSM-Info starts at {} but the file is {} bytes",
            tag.offset,
            buf.len()
        ))
    })?;
    if tag.declared_bytes < HEADER_BYTES {
        return Err(Error::Decode(format!(
            "LSM CZ-LSM-Info declares {} bytes, too short for its header",
            tag.declared_bytes
        )));
    }
    if present < tag.declared_bytes {
        return Err(Error::Decode(format!(
            "LSM CZ-LSM-Info declares {} bytes but only {present} are present",
            tag.declared_bytes
        )));
    }

    let magic = read_i32(buf, tag.offset, e)?;
    if magic != MAGIC {
        return Err(Error::Decode(format!(
            "LSM CZ-LSM-Info magic 0x{magic:08X} is not the LSM magic 0x{MAGIC:08X}"
        )));
    }

    // The struct repeats its size; the tag declares one too. The smaller of
    // the two is the struct's length.
    let structure_size = read_i32(buf, tag.offset + 4, e)?;
    let length = match usize::try_from(structure_size) {
        Ok(size) if size > 0 => tag.declared_bytes.min(size),
        // A struct that states no size of its own is read within the byte
        // count its tag declares.
        _ => tag.declared_bytes,
    };

    if length < DIMENSIONS_END {
        return Err(Error::Decode(format!(
            "LSM CZ-LSM-Info is {length} bytes, too short for the dimension fields"
        )));
    }
    let dim_x = read_i32(buf, tag.offset + DIMENSIONS_AT, e)?;
    let dim_y = read_i32(buf, tag.offset + DIMENSIONS_AT + 4, e)?;
    let dim_z = read_i32(buf, tag.offset + DIMENSIONS_AT + 8, e)?;
    let dim_channels = read_i32(buf, tag.offset + DIMENSIONS_AT + 12, e)?;
    let dim_time = read_i32(buf, tag.offset + DIMENSIONS_AT + 16, e)?;
    if dim_x <= 0 || dim_y <= 0 || dim_z < 0 || dim_channels <= 0 || dim_time < 0 {
        return Err(Error::Unsupported(format!(
            "LSM CZ-LSM-Info has non-positive dimensions \
             (X={dim_x} Y={dim_y} Z={dim_z} C={dim_channels} T={dim_time})"
        )));
    }

    // The voxel sizes are best-effort: present only if the struct reaches
    // them, and only if all three are strictly positive.
    let px_size_um = if length >= VOXEL_END {
        let vx = read_f64(buf, tag.offset + VOXEL_AT, e)?;
        let vy = read_f64(buf, tag.offset + VOXEL_AT + 8, e)?;
        let vz = read_f64(buf, tag.offset + VOXEL_AT + 16, e)?;
        if vx > 0.0 && vy > 0.0 && vz > 0.0 {
            Some(PxSizeUm {
                z: (vz * 1e6) as f32,
                y: (vy * 1e6) as f32,
                x: (vx * 1e6) as f32,
            })
        } else {
            None
        }
    } else {
        None
    };

    // The channel-colours pointer is an absolute file offset.
    let channel_colors_offset = if length >= CHANNEL_COLORS_END {
        let at = read_i32(buf, tag.offset + CHANNEL_COLORS_AT, e)?;
        usize::try_from(at).ok().filter(|&at| at > 0)
    } else {
        None
    };

    // The tile/position counts live deep in the struct. A struct that does
    // not reach them states no counts, and the file is a single-scene file.
    let (dim_p, dim_m) = if length >= COUNTS_END {
        (
            read_i32(buf, tag.offset + DIM_P_AT, e)?,
            read_i32(buf, tag.offset + DIM_M_AT, e)?,
        )
    } else {
        (1, 1)
    };

    Ok(CzInfo {
        dim_x,
        dim_y,
        dim_z,
        dim_channels,
        dim_time,
        dim_p,
        dim_m,
        channel_colors_offset,
        px_size_um,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A CZ-LSM-Info struct of `bytes` bytes with the dimensions of a 2×2
    /// image with one channel and one time point.
    fn struct_of(bytes: usize) -> Vec<u8> {
        let mut buf = vec![0u8; bytes];
        buf[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        buf[4..8].copy_from_slice(&(bytes as i32).to_le_bytes());
        if bytes < DIMENSIONS_END {
            return buf;
        }
        buf[8..12].copy_from_slice(&1024i32.to_le_bytes()); // DimensionX
        buf[12..16].copy_from_slice(&1024i32.to_le_bytes()); // DimensionY
        buf[16..20].copy_from_slice(&35i32.to_le_bytes()); // DimensionZ
        buf[20..24].copy_from_slice(&2i32.to_le_bytes()); // DimensionChannels
        buf[24..28].copy_from_slice(&1i32.to_le_bytes()); // DimensionTime
        buf
    }

    fn tag(bytes: usize) -> CzTag {
        CzTag {
            offset: 0,
            declared_bytes: bytes,
        }
    }

    #[test]
    fn the_dimensions_and_the_voxel_size_are_read() {
        let mut buf = struct_of(64);
        buf[40..48].copy_from_slice(&0.06589411e-6f64.to_le_bytes());
        buf[48..56].copy_from_slice(&0.06589411e-6f64.to_le_bytes());
        buf[56..64].copy_from_slice(&0.5e-6f64.to_le_bytes());

        let info = parse(&buf, tag(64), Endian::Little).unwrap();
        assert_eq!(info.dim_x, 1024);
        assert_eq!(info.dim_y, 1024);
        assert_eq!(info.dim_z, 35);
        assert_eq!(info.dim_channels, 2);
        assert_eq!(info.dim_time, 1);
        let px = info.px_size_um.expect("the voxel size");
        assert!((px.z - 0.5).abs() < 1e-6);
        assert!((px.x - 0.06589411).abs() < 1e-4);
    }

    #[test]
    fn a_struct_that_does_not_reach_the_voxel_sizes_has_none() {
        // 40 bytes: the dimensions are there, the voxel sizes are not.
        let info = parse(&struct_of(40), tag(40), Endian::Little).unwrap();
        assert!(info.px_size_um.is_none());
    }

    #[test]
    fn a_struct_that_does_not_reach_the_counts_states_one_acquisition() {
        // 112 bytes: the channel-colours pointer is the last field in reach.
        let mut buf = struct_of(112);
        buf[108..112].copy_from_slice(&4096i32.to_le_bytes());
        let info = parse(&buf, tag(112), Endian::Little).unwrap();
        assert_eq!(info.dim_p, 1);
        assert_eq!(info.dim_m, 1);
        assert_eq!(info.channel_colors_offset, Some(4096));
    }

    #[test]
    fn the_smaller_of_the_two_declared_lengths_wins() {
        // The tag declares 464 bytes, the struct itself says 112: the fields
        // beyond 112 are absent, however much the tag allows.
        let mut buf = struct_of(464);
        buf[4..8].copy_from_slice(&112i32.to_le_bytes());
        buf[264..268].copy_from_slice(&1i32.to_le_bytes());
        buf[268..272].copy_from_slice(&49i32.to_le_bytes());
        let info = parse(&buf, tag(464), Endian::Little).unwrap();
        assert_eq!(info.dim_p, 1, "the tile count beyond the struct is not read");
        assert_eq!(info.dim_m, 1);
        assert_eq!(info.channel_colors_offset, None);
    }

    #[test]
    fn a_struct_with_the_wrong_magic_is_refused() {
        let mut buf = struct_of(464);
        buf[0..4].copy_from_slice(&0x0400_0000i32.to_le_bytes());
        assert!(matches!(
            parse(&buf, tag(464), Endian::Little),
            Err(Error::Decode(_))
        ));
    }

    #[test]
    fn a_struct_that_reaches_past_the_file_is_refused() {
        let buf = struct_of(464);
        assert!(matches!(
            parse(&buf[..200], tag(464), Endian::Little),
            Err(Error::Decode(_))
        ));
    }

    #[test]
    fn a_struct_too_short_for_its_header_is_refused() {
        let buf = vec![0u8; 8];
        assert!(matches!(
            parse(&buf, tag(4), Endian::Little),
            Err(Error::Decode(_))
        ));
    }

    #[test]
    fn a_struct_too_short_for_the_dimensions_is_refused() {
        let buf = struct_of(20);
        assert!(matches!(
            parse(&buf, tag(20), Endian::Little),
            Err(Error::Decode(_))
        ));
    }
}
