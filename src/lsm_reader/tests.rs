use crate::error::Error;
use crate::lsm_bytes::{read_u16, read_u32};
use crate::types::{DType, Endian};

use super::ifd::{
    read_tiff_header, TAG_BITS_PER_SAMPLE, TAG_COMPRESSION, TAG_CZ_LSM_INFO, TAG_IMAGE_LENGTH,
    TAG_IMAGE_WIDTH, TAG_NEW_SUBFILE_TYPE, TAG_PLANAR_CONFIGURATION, TAG_SAMPLES_PER_PIXEL,
    TAG_SAMPLE_FORMAT, TAG_STRIP_BYTE_COUNTS, TAG_STRIP_OFFSETS,
};
use super::parse::{map_dtype, parse_lsm};
use super::values::{read_int_array, read_scalar, tiff_type_size};

#[test]
fn dtype_mapping() {
    assert_eq!(map_dtype(8, 1).unwrap(), DType::U8);
    assert_eq!(map_dtype(16, 1).unwrap(), DType::U16);
    assert_eq!(map_dtype(16, 2).unwrap(), DType::I16);
    assert_eq!(map_dtype(32, 3).unwrap(), DType::F32);
    assert!(map_dtype(32, 1).is_err());
    assert!(map_dtype(64, 3).is_err());
}

#[test]
fn type_sizes() {
    assert_eq!(tiff_type_size(3), 2);
    assert_eq!(tiff_type_size(4), 4);
    assert_eq!(tiff_type_size(1), 1);
    assert_eq!(tiff_type_size(5), 8);
    assert_eq!(tiff_type_size(999), 0);
}

#[test]
fn primitive_readers_respect_endianness() {
    let le = [0x34, 0x12, 0x78, 0x56, 0x00, 0x00, 0x00, 0x00];
    assert_eq!(read_u16(&le, 0, Endian::Little).unwrap(), 0x1234);
    assert_eq!(read_u32(&le, 0, Endian::Little).unwrap(), 0x5678_1234);
    let be = [0x12, 0x34, 0x56, 0x78];
    assert_eq!(read_u16(&be, 0, Endian::Big).unwrap(), 0x1234);
    assert_eq!(read_u32(&be, 0, Endian::Big).unwrap(), 0x1234_5678);
}

#[test]
fn primitive_readers_bounds_checked() {
    let short = [0x00, 0x01];
    assert!(matches!(read_u32(&short, 0, Endian::Little), Err(Error::Parse(_))));
    assert!(matches!(read_u16(&short, 1, Endian::Little), Err(Error::Parse(_))));
}

#[test]
fn scalar_inline_and_via_offset() {
    let mut buf = vec![0u8; 32];
    buf[8] = 0x02;
    assert_eq!(read_scalar(&buf, 3, 1, 8, Endian::Little).unwrap(), 2);

    let mut buf2 = vec![0u8; 32];
    buf2[8..12].copy_from_slice(&16u32.to_le_bytes());
    buf2[16..20].copy_from_slice(&100u32.to_le_bytes());
    buf2[20..24].copy_from_slice(&200u32.to_le_bytes());
    let arr = read_int_array(&buf2, 4, 2, 8, Endian::Little).unwrap();
    assert_eq!(arr, vec![100, 200]);
    assert_eq!(read_scalar(&buf2, 4, 2, 8, Endian::Little).unwrap(), 100);
}

#[test]
fn read_int_array_caps_absurd_element_count() {
    // Element-count cap: a crafted u32 count would grow a Vec<u64> with only a capacity hint;
    // real LSM files have a handful of strips.
    let buf = vec![0u8; 8 * 1024 * 1024];
    let r = read_int_array(&buf, 1 /* BYTE */, 8_000_000, 0, Endian::Little);
    assert!(r.is_err(), "absurd element count must be refused before allocating");
}

#[test]
fn tiff_header_parsing() {
    let mut le = vec![0u8; 8];
    le[0..2].copy_from_slice(b"II");
    le[2..4].copy_from_slice(&42u16.to_le_bytes());
    assert_eq!(read_tiff_header(&le).unwrap(), Endian::Little);

    let mut be = vec![0u8; 8];
    be[0..2].copy_from_slice(b"MM");
    be[2..4].copy_from_slice(&42u16.to_be_bytes());
    assert_eq!(read_tiff_header(&be).unwrap(), Endian::Big);

    let mut big = vec![0u8; 8];
    big[0..2].copy_from_slice(b"II");
    big[2..4].copy_from_slice(&43u16.to_le_bytes());
    assert!(matches!(read_tiff_header(&big), Err(Error::Parse(_))));

    assert!(matches!(read_tiff_header(b"XY\x2a\x00\x00\x00\x00\x00"), Err(Error::Parse(_))));
}

// A synthetic in-memory file drives the full `parse_lsm` path, unlike the fragment tests above.

/// A little-endian IFD entry: for SHORT/LONG with count 1 the value is inline, otherwise a pointer.
fn ifd_entry(tag: u16, ty: u16, count: u32, value: u32) -> [u8; 12] {
    let mut e = [0u8; 12];
    e[0..2].copy_from_slice(&tag.to_le_bytes());
    e[2..4].copy_from_slice(&ty.to_le_bytes());
    e[4..8].copy_from_slice(&count.to_le_bytes());
    e[8..12].copy_from_slice(&value.to_le_bytes());
    e
}

/// A 2-page Z=2 classic-TIFF LSM: page 1 declares 2 strips when `mismatched_strips`, with every other scalar equal to
/// page 0.
/// That is the gap the strip-count check closes: the scalar checks never compare strip counts.
fn build_minimal_lsm(mismatched_strips: bool) -> Vec<u8> {
    const IFD0_ENTRIES: usize = 11;
    const IFD1_ENTRIES: usize = 10;
    let ifd0_size = 2 + IFD0_ENTRIES * 12 + 4;
    let ifd1_size = 2 + IFD1_ENTRIES * 12 + 4;

    let ifd0_off = 8usize;
    let cz_off = ifd0_off + ifd0_size;
    let cz_len = 28usize;
    let ifd1_off = cz_off + cz_len;
    let ext_off = ifd1_off + ifd1_size;
    let total_len = ext_off + if mismatched_strips { 16 } else { 0 };

    let mut buf = vec![0u8; total_len];
    buf[0..2].copy_from_slice(b"II");
    buf[2..4].copy_from_slice(&42u16.to_le_bytes());
    buf[4..8].copy_from_slice(&(ifd0_off as u32).to_le_bytes());

    let mut entries0: Vec<u8> = Vec::new();
    entries0.extend(ifd_entry(TAG_NEW_SUBFILE_TYPE, 3, 1, 0));
    entries0.extend(ifd_entry(TAG_IMAGE_WIDTH, 3, 1, 2));
    entries0.extend(ifd_entry(TAG_IMAGE_LENGTH, 3, 1, 2));
    entries0.extend(ifd_entry(TAG_BITS_PER_SAMPLE, 3, 1, 8));
    entries0.extend(ifd_entry(TAG_COMPRESSION, 3, 1, 1));
    entries0.extend(ifd_entry(TAG_STRIP_OFFSETS, 4, 1, 1000));
    entries0.extend(ifd_entry(TAG_SAMPLES_PER_PIXEL, 3, 1, 1));
    entries0.extend(ifd_entry(TAG_STRIP_BYTE_COUNTS, 4, 1, 4));
    entries0.extend(ifd_entry(TAG_PLANAR_CONFIGURATION, 3, 1, 1));
    entries0.extend(ifd_entry(TAG_SAMPLE_FORMAT, 3, 1, 1));
    entries0.extend(ifd_entry(TAG_CZ_LSM_INFO, 4, 7, cz_off as u32));
    assert_eq!(entries0.len(), IFD0_ENTRIES * 12);
    buf[ifd0_off..ifd0_off + 2].copy_from_slice(&(IFD0_ENTRIES as u16).to_le_bytes());
    buf[ifd0_off + 2..ifd0_off + 2 + entries0.len()].copy_from_slice(&entries0);
    let next0_off = ifd0_off + 2 + entries0.len();
    buf[next0_off..next0_off + 4].copy_from_slice(&(ifd1_off as u32).to_le_bytes());

    buf[cz_off..cz_off + 4].copy_from_slice(&0x0400_494Cu32.to_le_bytes());
    buf[cz_off + 4..cz_off + 8].copy_from_slice(&(cz_len as u32).to_le_bytes());
    buf[cz_off + 8..cz_off + 12].copy_from_slice(&2i32.to_le_bytes());
    buf[cz_off + 12..cz_off + 16].copy_from_slice(&2i32.to_le_bytes());
    buf[cz_off + 16..cz_off + 20].copy_from_slice(&2i32.to_le_bytes());
    buf[cz_off + 20..cz_off + 24].copy_from_slice(&1i32.to_le_bytes());
    buf[cz_off + 24..cz_off + 28].copy_from_slice(&1i32.to_le_bytes());

    let mut entries1: Vec<u8> = Vec::new();
    entries1.extend(ifd_entry(TAG_NEW_SUBFILE_TYPE, 3, 1, 0));
    entries1.extend(ifd_entry(TAG_IMAGE_WIDTH, 3, 1, 2));
    entries1.extend(ifd_entry(TAG_IMAGE_LENGTH, 3, 1, 2));
    entries1.extend(ifd_entry(TAG_BITS_PER_SAMPLE, 3, 1, 8));
    entries1.extend(ifd_entry(TAG_COMPRESSION, 3, 1, 1));
    if mismatched_strips {
        entries1.extend(ifd_entry(TAG_STRIP_OFFSETS, 4, 2, ext_off as u32));
        entries1.extend(ifd_entry(TAG_SAMPLES_PER_PIXEL, 3, 1, 1));
        entries1.extend(ifd_entry(TAG_STRIP_BYTE_COUNTS, 4, 2, (ext_off + 8) as u32));
    } else {
        entries1.extend(ifd_entry(TAG_STRIP_OFFSETS, 4, 1, 2000));
        entries1.extend(ifd_entry(TAG_SAMPLES_PER_PIXEL, 3, 1, 1));
        entries1.extend(ifd_entry(TAG_STRIP_BYTE_COUNTS, 4, 1, 4));
    }
    entries1.extend(ifd_entry(TAG_PLANAR_CONFIGURATION, 3, 1, 1));
    entries1.extend(ifd_entry(TAG_SAMPLE_FORMAT, 3, 1, 1));
    assert_eq!(entries1.len(), IFD1_ENTRIES * 12);
    buf[ifd1_off..ifd1_off + 2].copy_from_slice(&(IFD1_ENTRIES as u16).to_le_bytes());
    buf[ifd1_off + 2..ifd1_off + 2 + entries1.len()].copy_from_slice(&entries1);
    let next1_off = ifd1_off + 2 + entries1.len();
    buf[next1_off..next1_off + 4].copy_from_slice(&0u32.to_le_bytes());

    if mismatched_strips {
        buf[ext_off..ext_off + 4].copy_from_slice(&1000u32.to_le_bytes());
        buf[ext_off + 4..ext_off + 8].copy_from_slice(&1004u32.to_le_bytes());
        buf[ext_off + 8..ext_off + 12].copy_from_slice(&2u32.to_le_bytes());
        buf[ext_off + 12..ext_off + 16].copy_from_slice(&2u32.to_le_bytes());
    }

    buf
}

#[test]
fn mismatched_strip_count_on_later_page_is_rejected() {
    // Page 1 matches page 0 in every scalar field but has 2 strips: the strip-count check rejects it.
    let mismatched = build_minimal_lsm(true);
    assert!(
        matches!(parse_lsm(&mismatched), Err(Error::Decode(_))),
        "a later page with a different strip count than page 0 must be rejected"
    );

    // Control: the same file with matching strip counts must still parse.
    let consistent = build_minimal_lsm(false);
    assert!(
        parse_lsm(&consistent).is_ok(),
        "a consistent 2-page file must still parse: {:?}",
        parse_lsm(&consistent).err()
    );
}
