// SPDX-License-Identifier: Apache-2.0

use crate::error::Error;
use crate::types::{DType, Endian};

use super::header::{parse, DvHeader, HEADER_BYTES};
use super::layout::{layout, Layout};

struct Hdr {
    buf: Vec<u8>,
    endian: Endian,
}

impl Hdr {
    fn new(endian: Endian) -> Hdr {
        let mut hdr = Hdr {
            buf: vec![0u8; HEADER_BYTES],
            endian,
        };
        hdr.word(0, 4);
        hdr.word(1, 3);
        hdr.word(2, 2);
        hdr.word(3, 1);
        hdr.real(10, 0.1);
        hdr.real(11, 0.1);
        hdr.real(12, 0.2);
        hdr.word(23, 0);
        hdr.short(96, -16224);
        hdr.short(180, 0);
        hdr.short(182, 0);
        hdr.short(196, 0);
        hdr
    }

    fn word(&mut self, index: usize, value: i32) {
        let at = index * 4;
        let raw = match self.endian {
            Endian::Little => value.to_le_bytes(),
            Endian::Big => value.to_be_bytes(),
        };
        self.buf[at..at + 4].copy_from_slice(&raw);
    }

    /// i16 at byte `offset`.
    fn short(&mut self, offset: usize, value: i16) {
        let raw = match self.endian {
            Endian::Little => value.to_le_bytes(),
            Endian::Big => value.to_be_bytes(),
        };
        self.buf[offset..offset + 2].copy_from_slice(&raw);
    }

    fn real(&mut self, index: usize, value: f32) {
        self.word(index, value.to_bits() as i32);
    }
}

fn geometry(hdr: &Hdr) -> (DvHeader, Layout) {
    let header = parse(&hdr.buf).expect("the header parses");
    let section = header.nx * header.ny * header.dtype.size_bytes();
    let len = HEADER_BYTES + header.next + header.nz * section;
    let layout = layout(&header, len).expect("the layout fits the file");
    (header, layout)
}

fn refusal(hdr: &Hdr) -> String {
    let header = match parse(&hdr.buf) {
        Err(Error::Unsupported(message)) => return message,
        Err(other) => panic!("expected Unsupported, got {other:?}"),
        Ok(header) => header,
    };
    match layout(&header, 0) {
        Err(Error::Unsupported(message)) => message,
        Err(other) => panic!("expected Unsupported, got {other:?}"),
        Ok(_) => panic!("expected Unsupported, got a layout"),
    }
}

/// A header that disagrees with itself is a `Parse` refusal, not an `Unsupported` one.
fn parse_refusal(hdr: &Hdr) -> String {
    match parse(&hdr.buf) {
        Err(Error::Parse(message)) => message,
        Err(other) => panic!("expected Parse, got {other:?}"),
        Ok(_) => panic!("expected Parse, got a header"),
    }
}

#[test]
fn a_little_endian_header_parses_every_field() {
    let mut hdr = Hdr::new(Endian::Little);
    hdr.word(2, 12);
    hdr.short(180, 3);
    hdr.short(182, 1);
    hdr.short(196, 2);
    hdr.word(23, 128);
    let (header, layout) = geometry(&hdr);
    assert_eq!((header.nx, header.ny, header.nz), (4, 3, 12));
    assert_eq!((header.dx, header.dy, header.dz), (0.1, 0.1, 0.2));
    assert_eq!(header.next, 128);
    assert_eq!(header.num_times, 3);
    assert_eq!(header.img_sequence, 1);
    assert_eq!(header.num_waves, 2);
    assert_eq!(header.dtype, DType::I16);
    assert_eq!(header.endian, Endian::Little);
    assert_eq!((layout.dims.t, layout.dims.c, layout.dims.z), (3, 2, 2));
}

#[test]
fn a_big_endian_header_parses_in_its_own_byte_order() {
    let mut hdr = Hdr::new(Endian::Big);
    hdr.word(0, 8);
    hdr.real(10, 2.5);
    let (header, _) = geometry(&hdr);
    assert_eq!(header.nx, 8);
    assert_eq!(header.dx, 2.5);
    assert_eq!(header.dtype, DType::I16);
    assert_eq!(header.endian, Endian::Big);
}

#[test]
fn a_header_without_the_ndvid_magic_is_refused() {
    let mut hdr = Hdr::new(Endian::Little);
    hdr.short(96, 0);
    let message = refusal(&hdr);
    assert!(message.contains("nDVID"), "{message}");
    assert!(message.contains("not a DeltaVision file"), "{message}");
}

#[test]
fn mode_0_is_u8_and_the_others_are_refused() {
    for mode in [0, 1, 2, 6] {
        let mut hdr = Hdr::new(Endian::Little);
        hdr.word(3, mode);
        assert!(parse(&hdr.buf).is_ok(), "mode {mode} is supported");
    }
    for mode in [3, 4, 5, 7] {
        let mut hdr = Hdr::new(Endian::Little);
        hdr.word(3, mode);
        let message = refusal(&hdr);
        assert!(message.contains(&format!("MODE {mode}")), "{message}");
    }
}

#[test]
fn the_accepted_modes_map_to_their_dtype() {
    for (mode, dtype) in [
        (0, DType::U8),
        (1, DType::I16),
        (2, DType::F32),
        (6, DType::U16),
    ] {
        let mut hdr = Hdr::new(Endian::Little);
        hdr.word(3, mode);
        assert_eq!(parse(&hdr.buf).expect("a supported mode").dtype, dtype);
    }
}

#[test]
fn a_non_positive_extent_is_refused() {
    for index in [0, 1, 2] {
        for value in [0, -1] {
            let mut hdr = Hdr::new(Endian::Little);
            hdr.word(index, value);
            assert!(
                refusal(&hdr).contains("must be positive"),
                "{index}={value}"
            );
        }
    }
}

#[test]
fn a_negative_next_is_refused() {
    let mut hdr = Hdr::new(Endian::Little);
    hdr.word(23, -1);
    let message = refusal(&hdr);
    assert!(message.contains("next"), "{message}");
    assert!(!message.contains("1844674407370955"), "{message}");
}

#[test]
fn a_negative_count_is_refused_with_its_own_value() {
    for (offset, name) in [(180, "NumTimes"), (196, "NumWaves")] {
        for value in [-1, -2, i16::MIN] {
            let mut hdr = Hdr::new(Endian::Little);
            hdr.short(offset, value);
            let message = refusal(&hdr);
            assert!(
                message.contains(&format!("{name} is {value}")),
                "{name}={value}: {message}"
            );
            assert!(!message.contains("1844674407370955"), "{message}");
        }
    }
}

#[test]
fn an_extended_header_past_next_is_refused() {
    // 2 sections of 1 integer and 1 float word need 2 * (1 + 1) * 4 = 16 bytes of `next`.
    let mut hdr = Hdr::new(Endian::Little);
    hdr.short(128, 1);
    hdr.short(130, 1);
    hdr.word(23, 16);
    assert!(parse(&hdr.buf).is_ok(), "exactly 16 bytes fit");

    let mut hdr = Hdr::new(Endian::Little);
    hdr.short(128, 1);
    hdr.short(130, 1);
    hdr.word(23, 15);
    let message = parse_refusal(&hdr);
    assert!(message.contains("NEXT is 15"), "{message}");
    assert!(message.contains("16 bytes"), "{message}");

    for (offset, name) in [(128, "NumIntegers"), (130, "NumFloats")] {
        let mut hdr = Hdr::new(Endian::Little);
        hdr.short(offset, -1);
        let message = refusal(&hdr);
        assert!(message.contains(&format!("{name} is -1")), "{message}");
        assert!(!message.contains("1844674407370955"), "{message}");
    }
}

#[test]
fn num_waves_above_five_is_refused() {
    let mut hdr = Hdr::new(Endian::Little);
    hdr.short(196, 6);
    let message = refusal(&hdr);
    assert!(message.contains("NumWaves is 6"), "{message}");
    assert!(message.contains("five wavelength"), "{message}");
}

#[test]
fn an_indivisible_nz_is_refused() {
    let mut hdr = Hdr::new(Endian::Little);
    hdr.word(2, 5);
    hdr.short(180, 0);
    hdr.short(196, 2);
    let message = refusal(&hdr);
    assert!(message.contains("not divisible"), "{message}");
}

#[test]
fn an_unknown_img_sequence_is_refused() {
    for seq in [-1, 3, 4] {
        let mut hdr = Hdr::new(Endian::Little);
        hdr.short(182, seq);
        let message = refusal(&hdr);
        assert!(message.contains(&format!("ImgSequence {seq}")), "{message}");
    }
}

#[test]
fn a_missing_calibration_leaves_the_axis_uncalibrated() {
    let mut hdr = Hdr::new(Endian::Little);
    hdr.real(10, 0.0);
    let (_, layout) = geometry(&hdr);
    assert_eq!(layout.x_um, None);
    assert_eq!(layout.y_um, Some(0.1));
}

#[test]
fn data_past_the_end_of_the_file_is_refused() {
    let hdr = Hdr::new(Endian::Little);
    let header = parse(&hdr.buf).expect("the header parses");
    match layout(&header, HEADER_BYTES) {
        Err(Error::Parse(message)) => assert!(message.contains("past the end of the"), "{message}"),
        Err(other) => panic!("expected Parse, got {other:?}"),
        Ok(_) => panic!("expected Parse, got a layout"),
    }
}
