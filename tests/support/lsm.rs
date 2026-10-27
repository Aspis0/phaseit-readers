// SPDX-License-Identifier: Apache-2.0
//! Writes small synthetic Zeiss LSM files for the reader tests.
//!
//! A hand-rolled classic TIFF with the private **CZ-LSM-Info** tag (34412) the
//! way a Zeiss file carries it: the magic and the struct's size at +0/+4, the
//! dimensions at +8..+28, the voxel sizes at +40/+48/+56, the channel-colours
//! pointer at +108, and the position/tile counts at +264/+268. A real file's
//! struct is 464 bytes long, and the tag declares exactly that many.
//!
//! Two blocks carry channel names, and they disagree on purpose — as they do
//! in the files themselves:
//!
//! * the **channel-colours block** holds the names Zeiss published for the
//!   acquisition, with the acquisition's own suffixes (`Ch3-T1`);
//! * the **ScanInfo** holds the bare detection-channel names (`Ch3`).
//!
//! The reader publishes the channel-colours block's names and colours; the
//! ScanInfo is there so the fixture has the disagreement a real file has.

use std::path::Path;

/// The CZ-LSM-Info struct's length, as a real file states it.
const CZ_LEN: usize = 464;

/// The magic every LSM file's CZ-LSM-Info struct starts with.
const CZ_MAGIC: i32 = 0x0400_494C;

/// Offset of the channel-colours pointer inside the CZ-LSM-Info struct.
const CZ_CHANNEL_COLORS: usize = 108;

/// Offset of `DimensionP` (stage positions) inside the CZ-LSM-Info struct.
const CZ_DIMENSION_P: usize = 264;

/// Offset of `DimensionM` (tiles) inside the CZ-LSM-Info struct.
const CZ_DIMENSION_M: usize = 268;

/// Offset of the colour table inside the channel-colours block, as a real file
/// states it.
const COLOR_TABLE_AT: usize = 40;

/// The display colours the fixture writes, one per channel.
const COLORS: [[u8; 3]; 4] = [
    [255, 0, 0],
    [0, 255, 0],
    [0, 0, 255],
    [255, 255, 255],
];

/// The shape of a synthetic LSM file.
#[derive(Debug, Clone)]
pub struct LsmSpec {
    /// The image width.
    pub x: u32,
    /// The image height.
    pub y: u32,
    /// The number of Z planes per time point.
    pub z: usize,
    /// The number of time points.
    pub t: usize,
    /// The number of channels.
    pub channels: usize,
    /// `DimensionP`: the number of stage positions.
    pub positions: i32,
    /// `DimensionM`: the number of tiles.
    pub tiles: i32,
    /// The names in the channel-colours block. `None` writes no
    /// channel-colours block at all: the file states no pointer to one.
    pub channel_names: Option<Vec<String>>,
    /// The names in the ScanInfo's detection channels.
    pub scan_info_names: Vec<String>,
    /// How many real pages to write. `None` writes `t * z * P * M` pages, the
    /// count the counts imply; a smaller number makes the file inconsistent.
    pub pages: Option<usize>,
    /// The byte count the CZ-LSM-Info tag declares, and the struct repeats. A
    /// shorter struct states no channel-colours block and no tile counts,
    /// however much the bytes beyond it hold.
    pub cz_bytes: usize,
}

impl LsmSpec {
    /// A plain file: one scene, one channel, no tiles.
    pub fn plain(x: u32, y: u32, z: usize) -> Self {
        Self {
            x,
            y,
            z,
            t: 1,
            channels: 1,
            positions: 1,
            tiles: 1,
            channel_names: None,
            scan_info_names: Vec::new(),
            pages: None,
            cz_bytes: CZ_LEN,
        }
    }

    /// The number of pages the counts imply.
    fn implied_pages(&self) -> usize {
        let scenes = (self.positions.max(0) as usize) * (self.tiles.max(0) as usize);
        let scenes = if scenes == 0 { 1 } else { scenes };
        scenes * self.t * self.z
    }
}

/// Where the builder put the parts of a file a test may want to patch.
#[derive(Debug, Clone, Copy)]
pub struct LsmOffsets {
    /// The file offset of the CZ-LSM-Info struct.
    pub cz: usize,
    /// The file offset of the channel-colours block, when the file has one.
    pub colors: Option<usize>,
}

/// One little-endian classic-TIFF IFD entry (tag, type, count, value/offset).
fn ifd_entry(tag: u16, ty: u16, count: u32, value: u32) -> [u8; 12] {
    let mut entry = [0u8; 12];
    entry[0..2].copy_from_slice(&tag.to_le_bytes());
    entry[2..4].copy_from_slice(&ty.to_le_bytes());
    entry[4..8].copy_from_slice(&count.to_le_bytes());
    entry[8..12].copy_from_slice(&value.to_le_bytes());
    entry
}

fn put_i32(buf: &mut [u8], at: usize, value: i32) {
    buf[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_f64(buf: &mut [u8], at: usize, value: f64) {
    buf[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

/// The channel-colours block: a header, the colour table and the name table.
///
/// The colour table holds one colour word per channel — three bytes of red,
/// green and blue, then a zero byte — and the name table one record per
/// channel: an `int32` length and that many bytes.
fn channel_colors_block(names: &[String]) -> Vec<u8> {
    let mut records = Vec::new();
    for name in names {
        let mut bytes = name.as_bytes().to_vec();
        bytes.push(0); // Zeiss terminates every name with a NUL.
        records.extend_from_slice(&(bytes.len() as i32).to_le_bytes());
        records.extend_from_slice(&bytes);
    }
    let name_table_at = COLOR_TABLE_AT + 4 * names.len();
    let mut block = vec![0u8; name_table_at + records.len()];
    put_i32(&mut block, 12, COLOR_TABLE_AT as i32);
    put_i32(&mut block, 16, name_table_at as i32);
    for (channel, color) in COLORS.iter().cycle().take(names.len()).enumerate() {
        let at = COLOR_TABLE_AT + 4 * channel;
        block[at..at + 3].copy_from_slice(color);
    }
    block[name_table_at..].copy_from_slice(&records);
    block
}

/// The ScanInfo block: the recording tree the format carries.
///
/// A real file nests the channel names under a recording block, a track block
/// and a detection-channel block; the fixture writes that nesting, with the
/// same key families and the same header shape (a block's header carries no
/// length — the extent comes from the tree).
fn scan_info_block(names: &[String]) -> Vec<u8> {
    let mut entries = Vec::new();
    for name in names {
        let bytes = name.as_bytes();
        entries.extend_from_slice(&0x7000_0014u32.to_le_bytes()); // detection channel name
        entries.extend_from_slice(&2u32.to_le_bytes()); // ASCII
        entries.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        entries.extend_from_slice(bytes);
    }
    let mut block = Vec::new();
    block.extend_from_slice(&0x1000_0000u32.to_le_bytes()); // the recording block
    block.extend_from_slice(&0u32.to_le_bytes());
    block.extend_from_slice(&0u32.to_le_bytes());
    block.extend_from_slice(&0x3000_0000u32.to_le_bytes()); // the track block
    block.extend_from_slice(&0u32.to_le_bytes());
    block.extend_from_slice(&0u32.to_le_bytes());
    block.extend_from_slice(&0x7000_0000u32.to_le_bytes()); // the detection channel block
    block.extend_from_slice(&0u32.to_le_bytes());
    block.extend_from_slice(&0u32.to_le_bytes());
    block.extend_from_slice(&entries);
    block
}

/// Build the whole file, and say where the parts a test may patch are.
pub fn build(spec: &LsmSpec) -> (Vec<u8>, LsmOffsets) {
    let pages = spec.pages.unwrap_or_else(|| spec.implied_pages());
    let plane_bytes = spec.x as usize * spec.y as usize;

    // The IFD chain: IFD0 carries the CZ-LSM-Info pointer, the rest do not.
    let ifd_entries = 11usize;
    let ifd_size = 2 + ifd_entries * 12 + 4;
    let ifds_at = 8usize;
    let ifds_end = ifds_at + ifd_size * pages;

    let cz_at = ifds_end;
    let colors_at = cz_at + CZ_LEN;
    let colors = spec.channel_names.as_ref().map(|names| channel_colors_block(names));
    let scan_info_at = colors_at + colors.as_ref().map_or(0, |c| c.len());
    let scan_info = scan_info_block(&spec.scan_info_names);

    // The strip tables: one entry per (page, channel) strip.
    let strips = pages * spec.channels;
    let strip_offsets_at = scan_info_at + scan_info.len();
    let strip_counts_at = strip_offsets_at + strips * 4;
    let data_at = strip_counts_at + strips * 4;

    // Each page's channel data: a flat, distinguishable value per strip.
    let mut data = Vec::with_capacity(strips * plane_bytes);
    for page in 0..pages {
        for channel in 0..spec.channels {
            let value = ((page * spec.channels + channel) % 251) as u8;
            data.extend(std::iter::repeat_n(value, plane_bytes));
        }
    }

    let mut buf = vec![0u8; data_at + data.len()];
    buf[0..2].copy_from_slice(b"II");
    buf[2..4].copy_from_slice(&42u16.to_le_bytes());
    buf[4..8].copy_from_slice(&(ifds_at as u32).to_le_bytes());

    // --- The CZ-LSM-Info struct. ---
    put_i32(&mut buf, cz_at, CZ_MAGIC);
    put_i32(&mut buf, cz_at + 4, spec.cz_bytes as i32); // structure size
    put_i32(&mut buf, cz_at + 8, spec.x as i32); // DimensionX
    put_i32(&mut buf, cz_at + 12, spec.y as i32); // DimensionY
    put_i32(&mut buf, cz_at + 16, spec.z as i32); // DimensionZ
    put_i32(&mut buf, cz_at + 20, spec.channels as i32); // DimensionChannels
    put_i32(&mut buf, cz_at + 24, spec.t as i32); // DimensionTime
    put_i32(&mut buf, cz_at + 28, 1); // data type: 8-bit unsigned
    put_f64(&mut buf, cz_at + 40, 1e-7); // voxel size X (metres)
    put_f64(&mut buf, cz_at + 48, 1e-7); // voxel size Y (metres)
    put_f64(&mut buf, cz_at + 56, 1e-7); // voxel size Z (metres)
    // A file with no channel-colours block states no pointer to one.
    let colors_offset = match &colors {
        Some(_) => colors_at as i32,
        None => 0,
    };
    put_i32(&mut buf, cz_at + CZ_CHANNEL_COLORS, colors_offset);
    put_i32(&mut buf, cz_at + 124, scan_info_at as i32); // ScanInformationOffset
    put_i32(&mut buf, cz_at + CZ_DIMENSION_P, spec.positions);
    put_i32(&mut buf, cz_at + CZ_DIMENSION_M, spec.tiles);

    // --- The two name blocks. ---
    if let Some(colors) = &colors {
        buf[colors_at..colors_at + colors.len()].copy_from_slice(colors);
    }
    buf[scan_info_at..scan_info_at + scan_info.len()].copy_from_slice(&scan_info);

    // --- The strip tables and the pixel data. ---
    for page in 0..pages {
        for channel in 0..spec.channels {
            let strip = page * spec.channels + channel;
            let at = data_at + strip * plane_bytes;
            buf[strip_offsets_at + strip * 4..strip_offsets_at + strip * 4 + 4]
                .copy_from_slice(&(at as u32).to_le_bytes());
            buf[strip_counts_at + strip * 4..strip_counts_at + strip * 4 + 4]
                .copy_from_slice(&(plane_bytes as u32).to_le_bytes());
        }
    }
    buf[data_at..].copy_from_slice(&data);

    // --- The IFD chain. ---
    for page in 0..pages {
        let at = ifds_at + page * ifd_size;
        let mut entries = Vec::new();
        entries.extend(ifd_entry(254, 3, 1, 0)); // NewSubfileType: full resolution
        entries.extend(ifd_entry(256, 3, 1, spec.x)); // ImageWidth
        entries.extend(ifd_entry(257, 3, 1, spec.y)); // ImageLength
        entries.extend(ifd_entry(258, 3, 1, 8)); // BitsPerSample
        entries.extend(ifd_entry(259, 3, 1, 1)); // Compression: none
        entries.extend(ifd_entry(277, 3, 1, spec.channels as u32)); // SamplesPerPixel
        entries.extend(ifd_entry(284, 3, 1, 2)); // PlanarConfiguration: planar
        entries.extend(ifd_entry(339, 3, 1, 1)); // SampleFormat: unsigned
        if page == 0 {
            // The CZ-LSM-Info struct: type 7 (undefined), so the value field
            // is a pointer to the struct, not the struct itself.
            entries.extend(ifd_entry(
                34412,
                7,
                spec.cz_bytes as u32,
                cz_at as u32,
            ));
        }
        // The strip tables: with one channel the single value is inline, with
        // more it is a pointer to the external array.
        let first = page * spec.channels;
        if spec.channels == 1 {
            let at = data_at + first * plane_bytes;
            entries.extend(ifd_entry(273, 4, 1, at as u32)); // StripOffsets
            entries.extend(ifd_entry(279, 4, 1, plane_bytes as u32)); // StripByteCounts
        } else {
            entries.extend(ifd_entry(
                273,
                4,
                spec.channels as u32,
                (strip_offsets_at + first * 4) as u32,
            ));
            entries.extend(ifd_entry(
                279,
                4,
                spec.channels as u32,
                (strip_counts_at + first * 4) as u32,
            ));
        }
        // IFD0 carries the CZ-LSM-Info pointer, the others do not.
        let entry_count = entries.len() / 12;
        debug_assert_eq!(entry_count, if page == 0 { 11 } else { 10 });

        buf[at..at + 2].copy_from_slice(&(entry_count as u16).to_le_bytes());
        buf[at + 2..at + 2 + entries.len()].copy_from_slice(&entries);
        let next = at + 2 + entries.len();
        let next_ifd = if page + 1 < pages {
            (ifds_at + (page + 1) * ifd_size) as u32
        } else {
            0
        };
        buf[next..next + 4].copy_from_slice(&next_ifd.to_le_bytes());
    }

    let offsets = LsmOffsets {
        cz: cz_at,
        colors: colors.as_ref().map(|_| colors_at),
    };
    (buf, offsets)
}

/// Write a synthetic LSM file to `path`.
pub fn write_lsm(path: &Path, spec: &LsmSpec) {
    let (buf, _) = build(spec);
    std::fs::write(path, buf).expect("write the synthetic LSM");
}
