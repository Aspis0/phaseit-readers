use crate::czi_reader::CziReader;
use crate::error::Result;
use crate::types::{ImageReader, PixelRect, PlaneBuf};

use super::super::directory::{DIM_ENTRY_LEN, DIR_ENTRY_FIXED};
use super::super::metadata::METADATA_FIXED;
use super::super::segment::SEGMENT_HEADER_LEN;
use super::super::subblock::SUBBLOCK_MIN_FIXED;

pub(super) struct TileSpec {
    pub(super) pixel_type: i32,
    pub(super) xs: i32,
    pub(super) xsz: i32,
    pub(super) ys: i32,
    pub(super) ysz: i32,
    pub(super) c: i32,
    pub(super) z: i32,
    pub(super) m: i32,
    pub(super) pixels: Vec<u8>,
}

pub(super) struct RawTile {
    pub(super) pixel_type: i32,
    pub(super) dims: Vec<(u8, i32, i32)>,
    pub(super) pixels: Vec<u8>,
    /// Points the entry at a non-subblock offset, so `subblock_pixel_span` fails.
    pub(super) bad_file_pos: Option<i64>,
}

pub(super) fn dim(ch: u8, start: i32, size: i32) -> [u8; 20] {
    let mut d = [0u8; 20];
    d[0] = ch;
    d[4..8].copy_from_slice(&start.to_le_bytes());
    d[8..12].copy_from_slice(&size.to_le_bytes());
    d[16..20].copy_from_slice(&size.to_le_bytes());
    d
}

#[allow(clippy::too_many_arguments)]
fn dir_entry_px(
    file_pos: i64,
    pixel_type: i32,
    xs: i32,
    xsz: i32,
    ys: i32,
    ysz: i32,
    c: i32,
    z: i32,
    m: i32,
) -> Vec<u8> {
    let mut e = vec![0u8; DIR_ENTRY_FIXED];
    e[0..2].copy_from_slice(b"DV");
    e[2..6].copy_from_slice(&pixel_type.to_le_bytes());
    e[6..14].copy_from_slice(&file_pos.to_le_bytes());
    e[28..32].copy_from_slice(&5i32.to_le_bytes());
    e.extend_from_slice(&dim(b'X', xs, xsz));
    e.extend_from_slice(&dim(b'Y', ys, ysz));
    e.extend_from_slice(&dim(b'C', c, 1));
    e.extend_from_slice(&dim(b'Z', z, 1));
    e.extend_from_slice(&dim(b'M', m, 1));
    e
}

#[allow(clippy::too_many_arguments)]
fn dir_entry(file_pos: i64, xs: i32, xsz: i32, ys: i32, ysz: i32, c: i32, z: i32, m: i32) -> Vec<u8> {
    dir_entry_px(file_pos, 0, xs, xsz, ys, ysz, c, z, m)
}

fn subblock_data(entry: &[u8], pixels: &[u8]) -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(&0i32.to_le_bytes());
    d.extend_from_slice(&0i32.to_le_bytes());
    d.extend_from_slice(&(pixels.len() as i64).to_le_bytes());
    d.extend_from_slice(entry);
    d.resize(SUBBLOCK_MIN_FIXED, 0);
    d.extend_from_slice(pixels);
    d
}

pub(super) fn seg(sid: &[u8], data: &[u8]) -> Vec<u8> {
    let mut s = vec![0u8; 16];
    s[..sid.len()].copy_from_slice(sid);
    s.extend_from_slice(&(data.len() as i64).to_le_bytes());
    s.extend_from_slice(&(data.len() as i64).to_le_bytes());
    s.extend_from_slice(data);
    s
}

pub(super) fn synthetic_czi() -> Vec<u8> {
    let file_total = SEGMENT_HEADER_LEN + 80;
    let xml = b"<ImageDocument/>";
    let md_data_len = METADATA_FIXED + xml.len();
    let meta_total = SEGMENT_HEADER_LEN + md_data_len;
    let dir_data_len = 4 + 124 + 2 * (DIR_ENTRY_FIXED + 5 * DIM_ENTRY_LEN);
    let dir_total = SEGMENT_HEADER_LEN + dir_data_len;
    let sub_total = SEGMENT_HEADER_LEN + (SUBBLOCK_MIN_FIXED + 4);

    let meta_pos = file_total as i64;
    let dir_pos = (file_total + meta_total) as i64;
    let sub_a = (file_total + meta_total + dir_total) as i64;
    let sub_b = sub_a + sub_total as i64;

    let mut fh = vec![0u8; 80];
    fh[52..60].copy_from_slice(&dir_pos.to_le_bytes());
    fh[60..68].copy_from_slice(&meta_pos.to_le_bytes());

    let mut md = Vec::new();
    md.extend_from_slice(&(xml.len() as i32).to_le_bytes());
    md.extend_from_slice(&0i32.to_le_bytes());
    md.resize(METADATA_FIXED, 0);
    md.extend_from_slice(xml);

    let mut dd = Vec::new();
    dd.extend_from_slice(&2i32.to_le_bytes());
    dd.resize(4 + 124, 0);
    dd.extend_from_slice(&dir_entry(sub_a, 0, 2, 0, 2, 0, 0, 0));
    dd.extend_from_slice(&dir_entry(sub_b, 2, 2, 0, 2, 0, 0, 1));

    let entry_a = dir_entry(sub_a, 0, 2, 0, 2, 0, 0, 0);
    let entry_b = dir_entry(sub_b, 2, 2, 0, 2, 0, 0, 1);
    let sda = subblock_data(&entry_a, &[1, 2, 3, 4]);
    let sdb = subblock_data(&entry_b, &[5, 6, 7, 8]);

    let mut file = Vec::new();
    file.extend_from_slice(&seg(b"ZISRAWFILE", &fh));
    file.extend_from_slice(&seg(b"ZISRAWMETADATA", &md));
    file.extend_from_slice(&seg(b"ZISRAWDIRECTORY", &dd));
    file.extend_from_slice(&seg(b"ZISRAWSUBBLOCK", &sda));
    file.extend_from_slice(&seg(b"ZISRAWSUBBLOCK", &sdb));
    file
}

pub(super) fn temp_path() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "phasit_synth_{:?}_{}_{}.czi",
        std::thread::current().id(),
        std::process::id(),
        n
    ))
}

/// Opens crafted bytes from a temp file, which is always removed afterwards.
pub(super) fn try_open_synth(bytes: &[u8]) -> Result<CziReader> {
    let path = temp_path();
    std::fs::write(&path, bytes).unwrap();
    let r = CziReader::open(&path);
    let _ = std::fs::remove_file(&path);
    r
}

pub(super) fn open_synth(bytes: &[u8]) -> CziReader {
    try_open_synth(bytes).unwrap()
}

pub(super) fn build_czi(tiles: &[TileSpec]) -> Vec<u8> {
    let n = tiles.len();
    let file_total = SEGMENT_HEADER_LEN + 80;
    let xml = b"<ImageDocument/>";
    let md_data_len = METADATA_FIXED + xml.len();
    let meta_total = SEGMENT_HEADER_LEN + md_data_len;
    let dir_data_len = 4 + 124 + n * (DIR_ENTRY_FIXED + 5 * DIM_ENTRY_LEN);
    let dir_total = SEGMENT_HEADER_LEN + dir_data_len;

    let mut sub_pos: Vec<i64> = Vec::with_capacity(n);
    let mut off = file_total + meta_total + dir_total;
    for t in tiles {
        sub_pos.push(off as i64);
        off += SEGMENT_HEADER_LEN + SUBBLOCK_MIN_FIXED + t.pixels.len();
    }

    let meta_pos = file_total as i64;
    let dir_pos = (file_total + meta_total) as i64;

    let mut fh = vec![0u8; 80];
    fh[52..60].copy_from_slice(&dir_pos.to_le_bytes());
    fh[60..68].copy_from_slice(&meta_pos.to_le_bytes());

    let mut md = Vec::new();
    md.extend_from_slice(&(xml.len() as i32).to_le_bytes());
    md.extend_from_slice(&0i32.to_le_bytes());
    md.resize(METADATA_FIXED, 0);
    md.extend_from_slice(xml);

    let mut dd = Vec::new();
    dd.extend_from_slice(&(n as i32).to_le_bytes());
    dd.resize(4 + 124, 0);
    for (t, &sp) in tiles.iter().zip(&sub_pos) {
        dd.extend_from_slice(&dir_entry_px(
            sp, t.pixel_type, t.xs, t.xsz, t.ys, t.ysz, t.c, t.z, t.m,
        ));
    }

    let mut file = Vec::new();
    file.extend_from_slice(&seg(b"ZISRAWFILE", &fh));
    file.extend_from_slice(&seg(b"ZISRAWMETADATA", &md));
    file.extend_from_slice(&seg(b"ZISRAWDIRECTORY", &dd));
    for (t, &sp) in tiles.iter().zip(&sub_pos) {
        let entry =
            dir_entry_px(sp, t.pixel_type, t.xs, t.xsz, t.ys, t.ysz, t.c, t.z, t.m);
        let sd = subblock_data(&entry, &t.pixels);
        file.extend_from_slice(&seg(b"ZISRAWSUBBLOCK", &sd));
    }
    file
}

fn dir_entry_dims(file_pos: i64, pixel_type: i32, dims: &[(u8, i32, i32)]) -> Vec<u8> {
    let mut e = vec![0u8; DIR_ENTRY_FIXED];
    e[0..2].copy_from_slice(b"DV");
    e[2..6].copy_from_slice(&pixel_type.to_le_bytes());
    e[6..14].copy_from_slice(&file_pos.to_le_bytes());
    e[28..32].copy_from_slice(&(dims.len() as i32).to_le_bytes());
    for &(ch, start, size) in dims {
        e.extend_from_slice(&dim(ch, start, size));
    }
    e
}

pub(super) fn build_czi_raw(tiles: &[RawTile]) -> Vec<u8> {
    let n = tiles.len();
    let file_total = SEGMENT_HEADER_LEN + 80;
    let xml = b"<ImageDocument/>";
    let md_data_len = METADATA_FIXED + xml.len();
    let meta_total = SEGMENT_HEADER_LEN + md_data_len;
    let dir_data_len: usize = 4
        + 124
        + tiles
            .iter()
            .map(|t| DIR_ENTRY_FIXED + t.dims.len() * DIM_ENTRY_LEN)
            .sum::<usize>();
    let dir_total = SEGMENT_HEADER_LEN + dir_data_len;

    // Test entries are under 240 bytes, so the fixed part pads to 256 and pixels sit at offset 256.
    let mut sub_pos: Vec<i64> = Vec::with_capacity(n);
    let mut off = file_total + meta_total + dir_total;
    for t in tiles {
        sub_pos.push(off as i64);
        off += SEGMENT_HEADER_LEN + SUBBLOCK_MIN_FIXED + t.pixels.len();
    }

    let meta_pos = file_total as i64;
    let dir_pos = (file_total + meta_total) as i64;

    let mut fh = vec![0u8; 80];
    fh[52..60].copy_from_slice(&dir_pos.to_le_bytes());
    fh[60..68].copy_from_slice(&meta_pos.to_le_bytes());

    let mut md = Vec::new();
    md.extend_from_slice(&(xml.len() as i32).to_le_bytes());
    md.extend_from_slice(&0i32.to_le_bytes());
    md.resize(METADATA_FIXED, 0);
    md.extend_from_slice(xml);

    let mut dd = Vec::new();
    dd.extend_from_slice(&(n as i32).to_le_bytes());
    dd.resize(4 + 124, 0);
    for (t, &sp) in tiles.iter().zip(&sub_pos) {
        let declared = t.bad_file_pos.unwrap_or(sp);
        dd.extend_from_slice(&dir_entry_dims(declared, t.pixel_type, &t.dims));
    }

    let mut file = Vec::new();
    file.extend_from_slice(&seg(b"ZISRAWFILE", &fh));
    file.extend_from_slice(&seg(b"ZISRAWMETADATA", &md));
    file.extend_from_slice(&seg(b"ZISRAWDIRECTORY", &dd));
    for t in tiles {
        // The inner DirectoryEntryDV carries the same dim count, so the fixed part is sized identically.
        let entry = dir_entry_dims(0, t.pixel_type, &t.dims);
        let sd = subblock_data(&entry, &t.pixels);
        file.extend_from_slice(&seg(b"ZISRAWSUBBLOCK", &sd));
    }
    file
}

/// Distinct bytes per tile, so a wrong row or column offset in the clipped blit mismatches.
pub(super) fn g8(base: u8) -> Vec<u8> {
    vec![base, base + 1, base + 2, base + 3]
}

/// A 2x2 Gray16 tile: `base..base+4` as little-endian u16 samples; `base` is a `u8` to share `mosaic_tiles`.
pub(super) fn g16(base: u8) -> Vec<u8> {
    let base = base as u16;
    let mut v = Vec::with_capacity(8);
    for i in 0..4u16 {
        v.extend_from_slice(&(base + i).to_le_bytes());
    }
    v
}

pub(super) fn bgr(base: u8) -> Vec<u8> {
    let mut v = Vec::with_capacity(12);
    for i in 0..4u8 {
        let p = base + i * 3;
        v.extend_from_slice(&[p, p + 1, p + 2]);
    }
    v
}

/// Three 2x2 tiles at (0,0), (2,0) and (0,2): the bottom-right cell is an uncovered gap of zeros.
pub(super) fn mosaic_tiles(pixel_type: i32, mk: impl Fn(u8) -> Vec<u8>, bases: [u8; 3]) -> Vec<TileSpec> {
    vec![
        TileSpec { pixel_type, xs: 0, xsz: 2, ys: 0, ysz: 2, c: 0, z: 0, m: 0, pixels: mk(bases[0]) },
        TileSpec { pixel_type, xs: 2, xsz: 2, ys: 0, ysz: 2, c: 0, z: 0, m: 1, pixels: mk(bases[1]) },
        TileSpec { pixel_type, xs: 0, xsz: 2, ys: 2, ysz: 2, c: 0, z: 0, m: 2, pixels: mk(bases[2]) },
    ]
}

pub(super) fn mosaic_rects_4x4() -> Vec<PixelRect> {
    vec![
        PixelRect::new(0, 0, 4, 4),
        PixelRect::new(1, 0, 2, 2),
        PixelRect::new(0, 1, 2, 2),
        PixelRect::new(1, 1, 2, 2),
        PixelRect::new(0, 0, 1, 1),
        PixelRect::new(3, 0, 1, 1),
        PixelRect::new(0, 3, 1, 1),
        PixelRect::new(3, 3, 1, 1),
        PixelRect::new(2, 2, 2, 2),
        PixelRect::new(2, 2, 1, 1),
        PixelRect::new(3, 0, 1, 4),
        PixelRect::new(0, 3, 4, 1),
        PixelRect::new(0, 2, 4, 2),
        PixelRect::new(0, 0, 0, 0),
        PixelRect::new(2, 2, 0, 0),
    ]
}

pub(super) fn assert_region_eq_crop(reader: &CziReader, rects: &[PixelRect]) {
    let full = reader.plane(0, 0, 0, 0).expect("compose full plane");
    for &rect in rects {
        let via_crop = full.crop(rect).expect("crop full plane");
        let via_region = reader.region(0, 0, 0, 0, rect).expect("out-of-core region");
        assert_eq!(via_region.shape, [rect.h, rect.w], "shape for {rect:?}");
        assert_eq!(via_region.samples, via_crop.samples, "samples for {rect:?}");
        assert_eq!(
            via_region, via_crop,
            "region() != plane().crop() for rect {rect:?}"
        );
    }
}

pub(super) fn mosaic_tile(xs: i32, m_dims: &[(u8, i32, i32)], pixels: Vec<u8>) -> RawTile {
    let mut dims = vec![(b'X', xs, 2), (b'Y', 0, 2)];
    dims.extend_from_slice(m_dims);
    RawTile {
        pixel_type: 0,
        dims,
        pixels,
        bad_file_pos: None,
    }
}

pub(super) fn mosaic_plane(reader: &CziReader, wide: usize) -> Vec<u8> {
    let p = reader.plane(0, 0, 0, 0).expect("compose full plane");
    assert_eq!(p.shape, [2, wide]);
    match p.data {
        PlaneBuf::U8(v) => v,
        other => panic!("expected U8, got {:?}", other.dtype()),
    }
}

/// Two 2x2 tiles 70000 px apart: a ~4.56 GiB composed plane, past `MAX_DECODED_PLANE_BYTES`.
pub(super) fn oversize_scene() -> Vec<u8> {
    build_czi(&[
        TileSpec { pixel_type: 0, xs: 0, xsz: 2, ys: 0, ysz: 2, c: 0, z: 0, m: 0, pixels: vec![1, 2, 3, 4] },
        TileSpec { pixel_type: 0, xs: 70000, xsz: 2, ys: 70000, ysz: 2, c: 0, z: 0, m: 1, pixels: vec![5, 6, 7, 8] },
    ])
}

/// A minimal manifest parser: the only nested brackets are `shape` arrays, so splitting on `}` is enough.
pub(super) fn parse_manifest(txt: &str) -> Vec<(String, String, usize)> {
    let mut out = Vec::new();
    for chunk in txt.split('}') {
        if let (Some(name), Some(variant), Some(itemsize)) = (
            json_str(chunk, "name"),
            json_str(chunk, "variant"),
            json_num(chunk, "itemsize"),
        ) {
            out.push((name, variant, itemsize));
        }
    }
    out
}

fn json_str(chunk: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let after = &chunk[chunk.find(&pat)? + pat.len()..];
    let after = &after[after.find(':')? + 1..];
    let after = &after[after.find('"')? + 1..];
    Some(after[..after.find('"')?].to_string())
}

fn json_num(chunk: &str, key: &str) -> Option<usize> {
    let pat = format!("\"{key}\"");
    let after = &chunk[chunk.find(&pat)? + pat.len()..];
    let after = after[after.find(':')? + 1..].trim_start();
    let end = after.find(|c: char| !c.is_ascii_digit()).unwrap_or(after.len());
    after[..end].parse().ok()
}

pub(super) fn metadata_segment(xml: &[u8]) -> Vec<u8> {
    let mut md = Vec::new();
    md.extend_from_slice(&(xml.len() as i32).to_le_bytes());
    md.extend_from_slice(&0i32.to_le_bytes());
    md.resize(METADATA_FIXED, 0);
    md.extend_from_slice(xml);
    seg(b"ZISRAWMETADATA", &md)
}
