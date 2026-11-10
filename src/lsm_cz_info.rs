// SPDX-License-Identifier: Apache-2.0
//! The CZ-LSM-Info struct: the private block that the first IFD's tag 34412 points at. Its fields sit at fixed offsets
//! (magic 0, size 4, dimensions 8, voxel sizes 40, channel colours 108, tile counts 264/268).
//! The struct is only as long as the smaller of its tag's byte count and its own size: fields beyond that are absent,
//! because reading past it would take strip data for metadata.

use crate::calibration::Calibration;
use crate::error::{Error, Result};
use crate::lsm_bytes::{read_f64, read_i32};
use crate::types::Endian;

const MAGIC: i32 = 0x0400_494C;

const HEADER_BYTES: usize = 8;

const DIMENSIONS_AT: usize = 8;
const DIMENSIONS_END: usize = 28;

const VOXEL_AT: usize = 40;
const VOXEL_END: usize = 64;

const CHANNEL_COLORS_AT: usize = 108;
const CHANNEL_COLORS_END: usize = 112;

const DIM_P_AT: usize = 264;
const DIM_M_AT: usize = 268;
const COUNTS_END: usize = 272;

#[derive(Debug, Clone, Copy)]
pub struct CzTag {
    pub offset: usize,
    pub declared_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct CzInfo {
    pub dim_x: i32,
    pub dim_y: i32,
    pub dim_z: i32,
    pub dim_channels: i32,
    pub dim_time: i32,
    pub dim_p: i32,
    pub dim_m: i32,
    pub channel_colors_offset: Option<usize>,
    pub calibration: Calibration,
}

/// Reads the struct within the length it declares; a field beyond that length is absent.
/// A struct with the wrong magic, or one reaching past the file, is refused.
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

    // The smaller of the tag's byte count and the struct's own size bounds it.
    let structure_size = read_i32(buf, tag.offset + 4, e)?;
    let length = match usize::try_from(structure_size) {
        Ok(size) if size > 0 => tag.declared_bytes.min(size),
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

    let calibration = voxel_calibration(buf, tag.offset, length, e)?;

    // The channel-colours pointer is an absolute file offset.
    let channel_colors_offset = if length >= CHANNEL_COLORS_END {
        let at = read_i32(buf, tag.offset + CHANNEL_COLORS_AT, e)?;
        usize::try_from(at).ok().filter(|&at| at > 0)
    } else {
        None
    };

    // A struct too short for the counts states none: the file is a single scene.
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
        calibration,
    })
}

/// The voxel sizes are best-effort: X and Y must be positive for a pixel size; the Z step goes through the shared rule,
/// so a placeholder Z publishes `None` while the X/Y size is still published.
fn voxel_calibration(buf: &[u8], offset: usize, length: usize, e: Endian) -> Result<Calibration> {
    if length < VOXEL_END {
        return Ok(Calibration::default());
    }
    let vx = read_f64(buf, offset + VOXEL_AT, e)?;
    let vy = read_f64(buf, offset + VOXEL_AT + 8, e)?;
    let vz = read_f64(buf, offset + VOXEL_AT + 16, e)?;
    if vx > 0.0 && vy > 0.0 {
        Ok(Calibration::sized(
            (vx * 1e6) as f32,
            (vy * 1e6) as f32,
            Some((vz * 1e6) as f32),
        ))
    } else {
        Ok(Calibration::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A CZ-LSM-Info struct of `bytes` bytes: dimensions of a 2x2 one-channel, one-time-point image.
    fn struct_of(bytes: usize) -> Vec<u8> {
        let mut buf = vec![0u8; bytes];
        buf[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        buf[4..8].copy_from_slice(&(bytes as i32).to_le_bytes());
        if bytes < DIMENSIONS_END {
            return buf;
        }
        buf[8..12].copy_from_slice(&1024i32.to_le_bytes());
        buf[12..16].copy_from_slice(&1024i32.to_le_bytes());
        buf[16..20].copy_from_slice(&35i32.to_le_bytes());
        buf[20..24].copy_from_slice(&2i32.to_le_bytes());
        buf[24..28].copy_from_slice(&1i32.to_le_bytes());
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
        let px = info.calibration.px_size_um.expect("the voxel size");
        assert!((px.z.expect("the Z step") - 0.5).abs() < 1e-6);
        assert!((px.x - 0.06589411).abs() < 1e-4);
        assert!(info.calibration.notes.is_empty(), "nothing was dropped");
    }

    #[test]
    fn a_placeholder_z_step_is_dropped_and_says_so() {
        // The `1e-19 m` Z sentinel (1e-13 µm) is dropped; the real X/Y sizes are still published.
        let mut buf = struct_of(64);
        buf[40..48].copy_from_slice(&0.06589411e-6f64.to_le_bytes());
        buf[48..56].copy_from_slice(&0.06589411e-6f64.to_le_bytes());
        buf[56..64].copy_from_slice(&1e-19f64.to_le_bytes());

        let info = parse(&buf, tag(64), Endian::Little).unwrap();
        let px = info.calibration.px_size_um.expect("the voxel size");
        assert_eq!(px.z, None, "the placeholder is not a step");
        assert!((px.x - 0.06589411).abs() < 1e-4, "the X size is published");
        assert_eq!(
            info.calibration.notes,
            vec![crate::calibration::z_step_note(1e-13)
                .expect("a placeholder earns a note")],
            "the note names the placeholder the file stated"
        );
    }

    #[test]
    fn a_struct_that_does_not_reach_the_voxel_sizes_has_none() {
        let info = parse(&struct_of(40), tag(40), Endian::Little).unwrap();
        assert!(info.calibration.px_size_um.is_none());
    }

    #[test]
    fn a_struct_that_does_not_reach_the_counts_states_one_acquisition() {
        let mut buf = struct_of(112);
        buf[108..112].copy_from_slice(&4096i32.to_le_bytes());
        let info = parse(&buf, tag(112), Endian::Little).unwrap();
        assert_eq!(info.dim_p, 1);
        assert_eq!(info.dim_m, 1);
        assert_eq!(info.channel_colors_offset, Some(4096));
    }

    #[test]
    fn the_smaller_of_the_two_declared_lengths_wins() {
        // A tag declaring 464 bytes with a struct of 112: the fields beyond 112 stay absent.
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
