// SPDX-License-Identifier: Apache-2.0
//! Unit tests of the first-IFD walk, the UIC arithmetic and the plane window: hand-built bytes, no disk.

use crate::error::Error;
use crate::tiff_layout::ChunkTable;
use crate::types::{DType, Dims5, Endian};

use super::geometry::Geometry;
use super::ifd::{self, Entry, UIC1, UIC2};
use super::layout::layout;
use super::uic::{plane_count, scale};

/// A classic TIFF whose first IFD sits at byte 8 and holds `(tag, kind, count, value)`, where a value that does not
/// fit in the entry is an offset.
fn tiff(entries: &[(u16, u16, u32, u32)], big_endian: bool) -> Vec<u8> {
    let word = |value: u32| {
        if big_endian {
            value.to_be_bytes()
        } else {
            value.to_le_bytes()
        }
    };
    let short = |value: u16| {
        if big_endian {
            value.to_be_bytes()
        } else {
            value.to_le_bytes()
        }
    };
    let mut out = if big_endian {
        b"MM\0*".to_vec()
    } else {
        b"II*\0".to_vec()
    };
    out.extend_from_slice(&word(8));
    out.extend_from_slice(&short(entries.len() as u16));
    for (tag, kind, count, value) in entries {
        out.extend_from_slice(&short(*tag));
        out.extend_from_slice(&short(*kind));
        out.extend_from_slice(&word(*count));
        out.extend_from_slice(&word(*value));
    }
    out.extend_from_slice(&word(0));
    out
}

/// One UIC1 value: a rational, or a length-prefixed string.
#[derive(Clone, Copy)]
enum Uic {
    Rational(u32, u32),
    Text(&'static str),
}

/// A UIC1 table at byte 8, its pairs pointing at the rationals and strings that follow them.
fn uic1(entries: &[(u32, Uic)]) -> (Vec<u8>, Entry) {
    let mut buf = vec![0u8; 8 + 8 * entries.len()];
    let mut at = buf.len();
    let mut values = Vec::with_capacity(entries.len());
    for (_, value) in entries {
        at = align2(at);
        values.push(at as u32);
        match value {
            Uic::Rational(numerator, denominator) => {
                buf.resize(at + 8, 0);
                buf[at..at + 4].copy_from_slice(&numerator.to_le_bytes());
                buf[at + 4..at + 8].copy_from_slice(&denominator.to_le_bytes());
                at += 8;
            }
            Uic::Text(text) => {
                buf.resize(at + 4 + text.len(), 0);
                buf[at..at + 4].copy_from_slice(&(text.len() as u32).to_le_bytes());
                buf[at + 4..at + 4 + text.len()].copy_from_slice(text.as_bytes());
                at += 4 + text.len();
            }
        }
    }
    for (index, (id, _)) in entries.iter().enumerate() {
        let at = 8 + index * 8;
        buf[at..at + 4].copy_from_slice(&id.to_le_bytes());
        buf[at + 4..at + 8].copy_from_slice(&values[index].to_le_bytes());
    }
    (
        buf,
        Entry {
            count: entries.len(),
            at: 8,
        },
    )
}

fn geometry(width: usize, height: usize, offsets: &[u64], bytes: &[u64]) -> Geometry {
    Geometry {
        width,
        height,
        dtype: DType::U16,
        strips: ChunkTable {
            offsets: offsets.to_vec(),
            bytes: bytes.to_vec(),
        },
    }
}

fn align2(at: usize) -> usize {
    if at % 2 == 1 {
        at + 1
    } else {
        at
    }
}

#[test]
fn an_entry_is_found_by_its_tag_in_either_byte_order() {
    for big_endian in [false, true] {
        let buf = tiff(&[(256, 4, 1, 5), (33629, 4, 4, 4096)], big_endian);
        let endian = if big_endian {
            Endian::Big
        } else {
            Endian::Little
        };
        let entry = ifd::entry(&buf, endian, UIC2)
            .unwrap()
            .expect("UIC2 is present");
        assert_eq!((entry.count, entry.at), (4, 4096));
        assert!(
            ifd::entry(&buf, endian, UIC1).unwrap().is_none(),
            "{big_endian}"
        );
    }
}

#[test]
fn a_file_too_short_for_its_ifd_is_refused() {
    let buf = tiff(&[], false);
    assert!(matches!(
        ifd::entry(&buf[..6], Endian::Little, UIC2),
        Err(Error::Parse(_))
    ));
}

#[test]
fn a_byte_order_that_is_not_a_classic_tiff_is_refused() {
    assert!(matches!(
        ifd::byte_order(b"II+\0\x08\0\0\0"),
        Err(Error::Unsupported(_))
    ));
}

#[test]
fn uic2_states_its_plane_count() {
    let entry = Entry { count: 3, at: 8 };
    assert_eq!(plane_count(&vec![0u8; 8 + 24 * 3], &entry).unwrap(), 3);
}

#[test]
fn uic2_of_no_plane_is_refused() {
    let entry = Entry { count: 0, at: 8 };
    assert!(matches!(
        plane_count(&[0u8; 64], &entry),
        Err(Error::Unsupported(_))
    ));
}

#[test]
fn uic2_value_past_the_end_of_the_file_is_refused() {
    // Eight planes need 192 bytes of records: this file holds 24.
    let entry = Entry { count: 8, at: 8 };
    assert!(matches!(
        plane_count(&[0u8; 24], &entry),
        Err(Error::Parse(message)) if message.contains("records")
    ));
}

#[test]
fn one_plane_still_needs_its_own_record() {
    // The record is six uint32 whatever the field type: a 4-byte value in the entry is not one.
    let entry = Entry { count: 1, at: 8 };
    assert_eq!(plane_count(&vec![0u8; 8 + 24], &entry).unwrap(), 1);
    assert!(matches!(
        plane_count(&[0u8; 24], &entry),
        Err(Error::Parse(message)) if message.contains("records")
    ));
}

#[test]
fn the_calibration_is_read_in_the_unit_its_entry_names() {
    for units in ["um", "micron", "micrometers"] {
        let (buf, entry) = uic1(&[
            (4, Uic::Rational(1, 2)),
            (5, Uic::Rational(1, 4)),
            (6, Uic::Text(units)),
        ]);
        let scale = scale(&buf, Some(entry), Endian::Little);
        assert_eq!((scale.x_um, scale.y_um), (Some(0.5), Some(0.25)), "{units}");
    }
}

#[test]
fn a_nanometre_calibration_is_converted_to_micrometres() {
    let (buf, entry) = uic1(&[
        (4, Uic::Rational(500, 1)),
        (5, Uic::Rational(250, 1)),
        (6, Uic::Text("nm")),
    ]);
    let scale = scale(&buf, Some(entry), Endian::Little);
    assert_eq!((scale.x_um, scale.y_um), (Some(0.5), Some(0.25)));
}

#[test]
fn a_unit_this_reader_does_not_convert_states_no_scale() {
    for units in ["mm", "inches", "nanometres", ""] {
        let (buf, entry) = uic1(&[
            (4, Uic::Rational(1, 2)),
            (5, Uic::Rational(1, 4)),
            (6, Uic::Text(units)),
        ]);
        let scale = scale(&buf, Some(entry), Endian::Little);
        assert_eq!((scale.x_um, scale.y_um), (None, None), "{units}");
    }
}

#[test]
fn a_file_without_uic1_states_no_scale() {
    assert_eq!(scale(&[0u8; 64], None, Endian::Little).x_um, None);
}

#[test]
fn a_uic1_table_outside_the_file_is_ignored() {
    let entry = Entry { count: 3, at: 4096 };
    let scale = scale(&[0u8; 64], Some(entry), Endian::Little);
    assert_eq!((scale.x_um, scale.y_um), (None, None));
}

#[test]
fn a_zero_denominator_is_not_a_distance() {
    let (buf, entry) = uic1(&[(4, Uic::Rational(1, 0)), (6, Uic::Text("um"))]);
    assert_eq!(scale(&buf, Some(entry), Endian::Little).x_um, None);
}

#[test]
fn the_planes_follow_plane_0_with_no_gap() {
    let geometry = geometry(5, 3, &[1024, 1024 + 30], &[30, 30]);
    let layout = layout(&geometry, 4, 1024 + 4 * 30).unwrap();
    assert_eq!(
        layout.dims,
        Dims5 {
            t: 1,
            c: 1,
            z: 4,
            y: 3,
            x: 5
        }
    );
    assert_eq!(layout.data_offset, 1024);
    assert_eq!(layout.plane_bytes, 30);
    assert_eq!(layout.plane_offset(3).unwrap(), 1024 + 90);
}

#[test]
fn strips_that_do_not_touch_are_refused() {
    let geometry = geometry(5, 3, &[1024, 1100], &[30, 30]);
    assert!(matches!(
        layout(&geometry, 2, 4096),
        Err(Error::Unsupported(message)) if message.contains("not contiguous")
    ));
}

#[test]
fn planes_past_the_end_of_the_file_are_refused() {
    let geometry = geometry(5, 3, &[1024], &[30]);
    assert!(matches!(
        layout(&geometry, 4, 1024 + 90),
        Err(Error::Parse(message)) if message.contains("past the end")
    ));
}

#[test]
fn a_plane_size_that_overflows_is_refused() {
    let geometry = geometry(usize::MAX, 2, &[1024], &[30]);
    assert!(layout(&geometry, 2, 4096).is_err());
}
