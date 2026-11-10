// SPDX-License-Identifier: Apache-2.0
//! Synthetic Zeiss LSM files for the reader tests: a classic TIFF with the private CZ-LSM-Info tag (34412).
//! The two channel-name blocks disagree, as in real files: the channel-colours block has the names with the
//! acquisition's suffixes (`Ch3-T1`), the ScanInfo the bare names (`Ch3`); the reader publishes the former.

use std::path::Path;

const CZ_LEN: usize = 464;

const CZ_MAGIC: i32 = 0x0400_494C;

const CZ_CHANNEL_COLORS: usize = 108;

const CZ_DIMENSION_P: usize = 264;

const CZ_DIMENSION_M: usize = 268;

const COLOR_TABLE_AT: usize = 40;

const COLORS: [[u8; 3]; 4] = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 255]];

#[derive(Debug, Clone)]
pub struct LsmSpec {
    pub x: u32,
    pub y: u32,
    pub z: usize,
    pub t: usize,
    pub channels: usize,
    pub positions: i32,
    pub tiles: i32,
    /// `None` writes no channel-colours block: the file states no pointer to one.
    pub channel_names: Option<Vec<String>>,
    pub scan_info_names: Vec<String>,
    /// `None` writes `t * z * P * M` pages, the count the counts imply.
    pub pages: Option<usize>,
    /// A window read must gather rows out of every strip of a multi-strip page.
    pub strips_per_channel: usize,
    /// A shorter struct states no channel-colours block and no tile counts.
    pub cz_bytes: usize,
}

impl LsmSpec {
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
            strips_per_channel: 1,
            cz_bytes: CZ_LEN,
        }
    }

    fn implied_pages(&self) -> usize {
        let scenes = (self.positions.max(0) as usize) * (self.tiles.max(0) as usize);
        let scenes = if scenes == 0 { 1 } else { scenes };
        scenes * self.t * self.z
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LsmOffsets {
    pub cz: usize,
    pub colors: Option<usize>,
    /// The strips, in the order the file states them: every page's channels, page by page.
    pub data: usize,
}

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

/// Colour table: one RGB word per channel. Name table: per channel, an `int32` length, then the bytes.
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

/// The recording tree the format carries: a real file nests the channel names under three blocks.
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

struct Parts {
    pages: usize,
    ifds_at: usize,
    ifd_size: usize,
    cz_at: usize,
    colors_at: usize,
    scan_info_at: usize,
    strip_offsets_at: usize,
    strip_counts_at: usize,
    data_at: usize,
    strip_bytes: usize,
    strips_per_channel: usize,
}

pub fn build(spec: &LsmSpec) -> (Vec<u8>, LsmOffsets) {
    let pages = spec.pages.unwrap_or_else(|| spec.implied_pages());
    let plane_bytes = spec.x as usize * spec.y as usize;

    let ifd_entries = 11usize;
    let ifd_size = 2 + ifd_entries * 12 + 4;
    let ifds_at = 8usize;
    let ifds_end = ifds_at + ifd_size * pages;

    let cz_at = ifds_end;
    let colors_at = cz_at + CZ_LEN;
    let colors = spec
        .channel_names
        .as_ref()
        .map(|names| channel_colors_block(names));
    let scan_info_at = colors_at + colors.as_ref().map_or(0, |c| c.len());
    let scan_info = scan_info_block(&spec.scan_info_names);

    let strips_per_channel = spec.strips_per_channel.max(1);
    assert_eq!(
        plane_bytes % strips_per_channel,
        0,
        "the plane's bytes split evenly into the strips"
    );
    let strip_bytes = plane_bytes / strips_per_channel;
    let strips = pages * spec.channels * strips_per_channel;
    let strip_offsets_at = scan_info_at + scan_info.len();
    let strip_counts_at = strip_offsets_at + strips * 4;
    let data_at = strip_counts_at + strips * 4;
    let parts = Parts {
        pages,
        ifds_at,
        ifd_size,
        cz_at,
        colors_at,
        scan_info_at,
        strip_offsets_at,
        strip_counts_at,
        data_at,
        strip_bytes,
        strips_per_channel,
    };

    let data = pixel_data(spec, &parts);
    let mut buf = vec![0u8; data_at + data.len()];
    buf[0..2].copy_from_slice(b"II");
    buf[2..4].copy_from_slice(&42u16.to_le_bytes());
    buf[4..8].copy_from_slice(&(ifds_at as u32).to_le_bytes());

    write_cz_struct(&mut buf, spec, &parts, colors.is_some());

    if let Some(colors) = &colors {
        buf[colors_at..colors_at + colors.len()].copy_from_slice(colors);
    }
    buf[scan_info_at..scan_info_at + scan_info.len()].copy_from_slice(&scan_info);

    write_strip_tables(&mut buf, spec, &parts);
    buf[data_at..].copy_from_slice(&data);
    write_ifds(&mut buf, spec, &parts);

    let offsets = LsmOffsets {
        cz: cz_at,
        colors: colors.as_ref().map(|_| colors_at),
        data: data_at,
    };
    (buf, offsets)
}

/// Each page's channel data: a flat, distinguishable value per strip.
fn pixel_data(spec: &LsmSpec, parts: &Parts) -> Vec<u8> {
    let strips = parts.pages * spec.channels * parts.strips_per_channel;
    let mut data = Vec::with_capacity(strips * parts.strip_bytes);
    for page in 0..parts.pages {
        for channel in 0..spec.channels {
            let value = ((page * spec.channels + channel) % 251) as u8;
            for _ in 0..parts.strips_per_channel {
                data.extend(std::iter::repeat_n(value, parts.strip_bytes));
            }
        }
    }
    data
}

fn write_cz_struct(buf: &mut [u8], spec: &LsmSpec, parts: &Parts, has_colors: bool) {
    let cz_at = parts.cz_at;
    put_i32(buf, cz_at, CZ_MAGIC);
    put_i32(buf, cz_at + 4, spec.cz_bytes as i32); // structure size
    put_i32(buf, cz_at + 8, spec.x as i32); // DimensionX
    put_i32(buf, cz_at + 12, spec.y as i32); // DimensionY
    put_i32(buf, cz_at + 16, spec.z as i32); // DimensionZ
    put_i32(buf, cz_at + 20, spec.channels as i32); // DimensionChannels
    put_i32(buf, cz_at + 24, spec.t as i32); // DimensionTime
    put_i32(buf, cz_at + 28, 1); // data type: 8-bit unsigned
    put_f64(buf, cz_at + 40, 1e-7); // voxel size X (metres)
    put_f64(buf, cz_at + 48, 1e-7); // voxel size Y (metres)
    put_f64(buf, cz_at + 56, 1e-7); // voxel size Z (metres)
    // A file with no channel-colours block states no pointer to one.
    let colors_offset = if has_colors { parts.colors_at as i32 } else { 0 };
    put_i32(buf, cz_at + CZ_CHANNEL_COLORS, colors_offset);
    put_i32(buf, cz_at + 124, parts.scan_info_at as i32); // ScanInformationOffset
    put_i32(buf, cz_at + CZ_DIMENSION_P, spec.positions);
    put_i32(buf, cz_at + CZ_DIMENSION_M, spec.tiles);
}

fn write_strip_tables(buf: &mut [u8], spec: &LsmSpec, parts: &Parts) {
    for page in 0..parts.pages {
        for channel in 0..spec.channels {
            for strip in 0..parts.strips_per_channel {
                let index = (page * spec.channels + channel) * parts.strips_per_channel + strip;
                let at = parts.data_at + index * parts.strip_bytes;
                buf[parts.strip_offsets_at + index * 4..parts.strip_offsets_at + index * 4 + 4]
                    .copy_from_slice(&(at as u32).to_le_bytes());
                buf[parts.strip_counts_at + index * 4..parts.strip_counts_at + index * 4 + 4]
                    .copy_from_slice(&(parts.strip_bytes as u32).to_le_bytes());
            }
        }
    }
}

/// The IFD chain: one IFD per page, and IFD0 carries the CZ-LSM-Info pointer.
fn write_ifds(buf: &mut [u8], spec: &LsmSpec, parts: &Parts) {
    for page in 0..parts.pages {
        let at = parts.ifds_at + page * parts.ifd_size;
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
            // The CZ-LSM-Info struct: type 7 (undefined), so the value field is a pointer to the struct.
            entries.extend(ifd_entry(34412, 7, spec.cz_bytes as u32, parts.cz_at as u32));
        }
        // The strip tables: with ONE strip the value is inline, with more it is a pointer to the array.
        let first = page * spec.channels * parts.strips_per_channel;
        let count = (spec.channels * parts.strips_per_channel) as u32;
        if count == 1 {
            let at = parts.data_at + first * parts.strip_bytes;
            entries.extend(ifd_entry(273, 4, 1, at as u32)); // StripOffsets
            entries.extend(ifd_entry(279, 4, 1, parts.strip_bytes as u32)); // StripByteCounts
        } else {
            entries.extend(ifd_entry(
                273,
                4,
                count,
                (parts.strip_offsets_at + first * 4) as u32,
            ));
            entries.extend(ifd_entry(
                279,
                4,
                count,
                (parts.strip_counts_at + first * 4) as u32,
            ));
        }
        let entry_count = entries.len() / 12;
        debug_assert_eq!(entry_count, if page == 0 { 11 } else { 10 });

        buf[at..at + 2].copy_from_slice(&(entry_count as u16).to_le_bytes());
        buf[at + 2..at + 2 + entries.len()].copy_from_slice(&entries);
        let next = at + 2 + entries.len();
        let next_ifd = if page + 1 < parts.pages {
            (parts.ifds_at + (page + 1) * parts.ifd_size) as u32
        } else {
            0
        };
        buf[next..next + 4].copy_from_slice(&next_ifd.to_le_bytes());
    }
}

/// The file offset of an IFD entry's value field, by tag, in the page's IFD.
/// WHY: a test that patches a file must find the entry it changes; the entry order is the writer's business.
pub fn ifd_entry_value(bytes: &[u8], page: usize, tag: u16) -> Option<usize> {
    let ifd_at = 8 + page * (2 + 11 * 12 + 4);
    let count = u16::from_le_bytes(bytes.get(ifd_at..ifd_at + 2)?.try_into().ok()?) as usize;
    for index in 0..count {
        let entry = ifd_at + 2 + index * 12;
        let entry_tag = u16::from_le_bytes(bytes.get(entry..entry + 2)?.try_into().ok()?);
        if entry_tag == tag {
            return Some(entry + 8);
        }
    }
    None
}

pub fn write_lsm(path: &Path, spec: &LsmSpec) {
    let (buf, _) = build(spec);
    std::fs::write(path, buf).expect("write the synthetic LSM");
}
