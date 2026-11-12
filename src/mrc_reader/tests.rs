use crate::error::Error;
use crate::types::{DType, Endian};

use super::header::{parse, MrcHeader, HEADER_BYTES};
use super::layout::{layout, Layout};

/// A little-endian 4x3x2 MODE 1 header; a test sets only the words it is about.
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
        for axis in 0..3 {
            hdr.word(7 + axis, 1);
            hdr.real(10 + axis, 10.0);
        }
        hdr.word(16, 1);
        hdr.word(17, 2);
        hdr.word(18, 3);
        hdr.buf[208..212].copy_from_slice(b"MAP ");
        let stamp: [u8; 2] = match endian {
            Endian::Little => [0x44, 0x44],
            Endian::Big => [0x11, 0x11],
        };
        hdr.buf[212..214].copy_from_slice(&stamp);
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

    fn real(&mut self, index: usize, value: f32) {
        self.word(index, value.to_bits() as i32);
    }
}

/// Parses `hdr` and lays it out against a file exactly as long as the data it declares.
fn geometry(hdr: &Hdr) -> (MrcHeader, Layout) {
    let header = parse(&hdr.buf).expect("the header parses");
    let section = header.nx * header.ny * header.dtype.size_bytes();
    let len = HEADER_BYTES + header.nsymbt + header.nz * section;
    let layout = layout(&header, len).expect("the layout fits the file");
    (header, layout)
}

/// The sentence a file is refused with, from the header check or the layout check.
fn refusal(hdr: &Hdr) -> String {
    let header = match parse(&hdr.buf) {
        Err(Error::Unsupported(message)) => return message,
        Err(other) => panic!("expected Unsupported, got {other}"),
        Ok(header) => header,
    };
    match layout(&header, 0) {
        Err(Error::Unsupported(message)) => message,
        Err(other) => panic!("expected Unsupported, got {other}"),
        Ok(_) => panic!("expected Unsupported, got a layout"),
    }
}

#[test]
fn a_little_endian_header_parses_every_field() {
    let hdr = Hdr::new(Endian::Little);
    let (header, _) = geometry(&hdr);
    assert_eq!((header.nx, header.ny, header.nz), (4, 3, 2));
    assert_eq!((header.mx, header.my, header.mz), (1, 1, 1));
    assert_eq!(header.cella, [10.0, 10.0, 10.0]);
    assert_eq!(header.ispg, 0);
    assert_eq!(header.nsymbt, 0);
    assert_eq!(header.dtype, DType::I16);
    assert_eq!(header.endian, Endian::Little);
}

#[test]
fn a_big_endian_header_parses_in_its_own_byte_order() {
    let mut hdr = Hdr::new(Endian::Big);
    hdr.word(0, 8);
    hdr.real(10, 2.5);
    let (header, _) = geometry(&hdr);
    assert_eq!(header.nx, 8);
    assert_eq!(header.cella[0], 2.5);
    assert_eq!(header.dtype, DType::I16);
    assert_eq!(header.endian, Endian::Big);
}

#[test]
fn a_header_without_the_map_signature_is_refused() {
    let mut hdr = Hdr::new(Endian::Little);
    hdr.buf[208..212].copy_from_slice(b"MRX ");
    assert!(refusal(&hdr).contains("`MAP `"), "{}", refusal(&hdr));
}

#[test]
fn an_unknown_machst_is_refused() {
    let mut hdr = Hdr::new(Endian::Little);
    hdr.buf[212] = 0x22;
    let message = refusal(&hdr);
    assert!(message.contains("MACHST"), "{message}");
}

#[test]
fn mode_0_is_refused_rather_than_read_as_u8() {
    let mut hdr = Hdr::new(Endian::Little);
    hdr.word(3, 0);
    let message = refusal(&hdr);
    assert!(message.contains("MODE 0"), "{message}");
}

#[test]
fn the_modes_the_crate_cannot_represent_are_refused_by_name() {
    for mode in [3, 4, 12, 101] {
        let mut hdr = Hdr::new(Endian::Little);
        hdr.word(3, mode);
        let message = refusal(&hdr);
        assert!(message.contains(&format!("MODE {mode}")), "{message}");
    }
}

#[test]
fn the_accepted_modes_map_to_their_dtype() {
    for (mode, dtype) in [(1, DType::I16), (2, DType::F32), (6, DType::U16)] {
        let mut hdr = Hdr::new(Endian::Little);
        hdr.word(3, mode);
        assert_eq!(parse(&hdr.buf).expect("a supported mode").dtype, dtype);
    }
}

#[test]
fn an_axis_order_other_than_one_two_three_is_refused() {
    for (mapc, mapr, maps) in [(1, 2, 4), (0, 2, 3), (1, 2, 3 + 1)] {
        let mut hdr = Hdr::new(Endian::Little);
        hdr.word(16, mapc);
        hdr.word(17, mapr);
        hdr.word(18, maps);
        let message = refusal(&hdr);
        assert!(message.contains("MAPC/MAPR/MAPS"), "{message}");
    }
}

#[test]
fn a_non_positive_extent_is_refused() {
    for index in [0, 1, 2] {
        for value in [0, -1] {
            let mut hdr = Hdr::new(Endian::Little);
            hdr.word(index, value);
            assert!(refusal(&hdr).contains("must be positive"));
        }
    }
}

#[test]
fn a_negative_extended_header_length_is_refused() {
    let mut hdr = Hdr::new(Endian::Little);
    hdr.word(23, -1);
    let message = refusal(&hdr);
    assert!(message.contains("NSYMBT"), "{message}");
}

#[test]
fn a_stack_of_images_has_no_z_calibration() {
    let hdr = Hdr::new(Endian::Little);
    let (header, layout) = geometry(&hdr);
    assert_eq!((layout.dims.t, layout.dims.z, layout.dims.c), (1, 2, 1));
    assert_eq!((layout.dims.y, layout.dims.x), (3, 4));
    assert_eq!(layout.z_um, None);
    assert_eq!(layout.x_um, Some(10.0 * 1e-4));
    assert_eq!(layout.y_um, Some(10.0 * 1e-4));
    assert_eq!(layout.data_offset, HEADER_BYTES);
    assert_eq!(layout.section_bytes, 4 * 3 * 2);
    assert_eq!(header.dtype, DType::I16);
}

#[test]
fn a_volume_states_a_z_step_from_cella_z_over_mz() {
    let mut hdr = Hdr::new(Endian::Little);
    hdr.word(22, 1);
    hdr.word(9, 2);
    hdr.real(12, 30.0);
    let (_, layout) = geometry(&hdr);
    assert_eq!(layout.z_um, Some(30.0 / 2.0 * 1e-4));
    assert_eq!(layout.dims.z, 2, "every section of the volume is a plane");
}

#[test]
fn a_stack_of_volumes_splits_time_from_z() {
    let mut hdr = Hdr::new(Endian::Little);
    hdr.word(2, 6);
    hdr.word(22, 401);
    hdr.word(9, 3);
    let (_, layout) = geometry(&hdr);
    assert_eq!((layout.dims.t, layout.dims.z), (2, 3));
    assert_eq!(layout.z_um, Some(10.0 / 3.0 * 1e-4));
}

#[test]
fn a_stack_of_volumes_refuses_a_zero_mz() {
    let mut hdr = Hdr::new(Endian::Little);
    hdr.word(2, 4);
    hdr.word(22, 401);
    hdr.word(9, 0);
    let message = refusal(&hdr);
    assert!(message.contains("MZ is 0"), "{message}");
}

#[test]
fn a_stack_of_volumes_refuses_an_indivisible_nz() {
    let mut hdr = Hdr::new(Endian::Little);
    hdr.word(2, 5);
    hdr.word(22, 401);
    hdr.word(9, 2);
    let message = refusal(&hdr);
    assert!(message.contains("does not divide NZ 5"), "{message}");
}

#[test]
fn an_unknown_ispg_is_refused() {
    for ispg in [-1, 231, 400, 631] {
        let mut hdr = Hdr::new(Endian::Little);
        hdr.word(22, ispg);
        let message = refusal(&hdr);
        assert!(message.contains(&format!("ISPG {ispg}")), "{message}");
    }
}

#[test]
fn a_missing_cell_size_leaves_the_axis_uncalibrated() {
    let mut hdr = Hdr::new(Endian::Little);
    hdr.real(10, 0.0);
    let (_, layout) = geometry(&hdr);
    assert_eq!(layout.x_um, None);
    assert_eq!(layout.y_um, Some(10.0 * 1e-4));
}

#[test]
fn data_past_the_end_of_the_file_is_refused() {
    let hdr = Hdr::new(Endian::Little);
    let header = parse(&hdr.buf).expect("the header parses");
    match layout(&header, HEADER_BYTES) {
        Err(Error::Parse(message)) => assert!(message.contains("past the end of the"), "{message}"),
        Err(other) => panic!("expected Parse, got {other}"),
        Ok(_) => panic!("expected Parse, got a layout"),
    }
}
