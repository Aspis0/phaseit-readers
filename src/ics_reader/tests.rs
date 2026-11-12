// SPDX-License-Identifier: Apache-2.0
//! Unit tests of the ICS header parse, the layout's own arithmetic and the unit table: no disk, one builder of header
//! bytes, so a field can be moved or dropped without writing a file.

use crate::error::Error;
use crate::types::{DType, Endian};

use super::header::{end_of, parse_fields, IcsHeader, Storage};
use super::layout::{layout, Compression};

/// A header with the field and line separators the file uses, its lines given as `category<TAB>subcategory<TAB>values`.
fn header_text(lines: &[&str], field: char, crlf: bool) -> Vec<u8> {
    let mut out = vec![field as u8, b'\n'];
    for line in lines {
        out.extend_from_slice(line.replace('\t', &field.to_string()).as_bytes());
        out.push(b'\n');
        if crlf {
            let at = out.len() - 1;
            out.insert(at, b'\r');
        }
    }
    out
}

/// The lines a minimal 2.0 file carries, x by y by z unsigned 16-bit.
fn minimal() -> Vec<&'static str> {
    vec![
        "ics_version\t2.0",
        "layout\tparameters\t4",
        "layout\torder\tbits\tx\ty\tz",
        "layout\tsizes\t16\t4\t3\t2",
        "representation\tformat\tinteger",
        "representation\tsign\tunsigned",
        "representation\tcompression\tuncompressed",
        "representation\tbyte_order\t1 2",
        "parameter\tscale\t3\t0.5\t0.5\t0.2",
        "parameter\tunits\t3\tmicrometers\tmicrometers\tmicrometers",
        "end",
    ]
}

/// `lines` up to `end`, which the caller drops from the data offset.
fn parse(lines: &[&str]) -> Result<IcsHeader, Error> {
    let buf = header_text(lines, '\t', false);
    let end = end_of(&buf, false).expect("the header has an end");
    parse_fields(&buf, end)
}

/// The layout of a parsed header, with the file length the pixels live in.
fn geometry(
    lines: &[&str],
    file_bytes: Option<usize>,
) -> Result<(IcsHeader, super::layout::Layout), Error> {
    let header = parse(lines)?;
    let layout = layout(&header, file_bytes)?;
    Ok((header, layout))
}

fn refusal(lines: &[&str]) -> String {
    match parse(lines) {
        Err(Error::Unsupported(message)) | Err(Error::Parse(message)) => message,
        Err(other) => panic!("expected a refusal, got {other:?}"),
        Ok(_) => panic!("expected a refusal, got a header"),
    }
}

#[test]
fn the_end_field_ends_the_header_and_the_pixels_start_after_it() {
    let buf = header_text(&minimal(), '\t', false);
    let end = end_of(&buf, false).expect("the header has an end");
    assert_eq!(&buf[end - 4..end], b"end\n");
    let header = parse_fields(&buf, end).expect("the header parses");
    assert_eq!(header.storage, Storage::Inline);
    assert_eq!(header.data_offset, end);
    assert_eq!(header.bits, 16);
    assert_eq!(header.axes, vec!["x", "y", "z"]);
    assert_eq!(header.sizes, vec![4, 3, 2]);
    assert_eq!(header.scale, vec![0.5, 0.5, 0.2]);
    assert_eq!(header.units, vec!["micrometers"; 3]);
    assert_eq!(header.byte_order.as_deref(), Some("1 2"));
    assert_eq!(header.version, "2.0");
}

#[test]
fn cr_lf_lines_and_another_field_separator_parse_the_same_fields() {
    let buf = header_text(&minimal(), ';', true);
    let end = end_of(&buf, false).expect("the header has an end");
    let header = parse_fields(&buf, end).expect("the header parses");
    assert_eq!(header.axes, vec!["x", "y", "z"]);
    assert_eq!(header.units, vec!["micrometers"; 3]);
}

#[test]
fn a_field_that_holds_the_end_word_is_not_the_end() {
    let mut lines = minimal();
    lines.insert(1, "history\tend");
    let buf = header_text(&lines, '\t', false);
    // The `end` inside a value is a value: only the line whose only field is `end` ends the header.
    assert!(end_of(&buf, false).is_some(), "the real end line is found");
    let header = parse_fields(&buf, end_of(&buf, false).expect("the header has an end"))
        .expect("the header parses");
    assert_eq!(header.axes, vec!["x", "y", "z"]);
}

#[test]
fn the_end_line_must_be_present() {
    let lines: Vec<&str> = minimal().into_iter().filter(|l| *l != "end").collect();
    let buf = header_text(&lines, '\t', false);
    assert!(end_of(&buf, false).is_none(), "no end line, no header");
}

#[test]
fn an_end_line_with_a_trailing_separator_ends_the_header_too() {
    // `end_of` and `parse_fields` must agree on the same line: a writer that left a separator behind is not a
    // different file.
    for line in ["end", "end\t", "end\t\t"] {
        let lines: Vec<&str> = minimal()
            .into_iter()
            .filter(|l| *l != "end")
            .chain([line])
            .collect();
        let buf = header_text(&lines, '\t', false);
        let end = end_of(&buf, false).unwrap_or_else(|| panic!("{line:?} ends the header"));
        let header = parse_fields(&buf, end).unwrap_or_else(|_| panic!("{line:?} parses"));
        assert_eq!(header.axes, vec!["x", "y", "z"], "{line:?}");
        assert_eq!(buf[end - 1], b'\n', "the offset follows the line separator");
        assert!(line.starts_with("end"), "{line:?}");
    }
}

#[test]
fn an_unterminated_end_line_is_not_an_end_until_its_separator_arrives() {
    // The bytes of a header whose `end` is the last thing read: the offset would be one past what is known.
    let buf = header_text(&minimal(), '\t', false);
    let end = end_of(&buf, false).expect("the header has an end");
    assert!(end <= buf.len(), "the offset is inside the buffer");
    assert!(
        end_of(&buf[..end - 1], false).is_none(),
        "without the separator it is not an end"
    );
}

#[test]
fn an_end_line_at_the_end_of_the_file_ends_the_header_without_a_separator() {
    // The file stops at the `end` field, so there is no separator after it and the pixels start at the last byte read.
    let buf = header_text(&minimal(), '\t', false);
    let trimmed = &buf[..buf.len() - 1];
    let end = end_of(trimmed, true).expect("the end of the file ends the header");
    assert_eq!(end, trimmed.len(), "the offset is the last byte read");
    let header = parse_fields(trimmed, end).expect("the header parses");
    assert_eq!(header.axes, vec!["x", "y", "z"]);
}

#[test]
fn version_1_stores_the_pixels_in_a_file_of_its_own() {
    let mut lines = minimal();
    lines[0] = "ics_version\t1.0";
    let (header, _) = geometry(&lines, None).expect("a 1.0 header parses");
    assert_eq!(header.storage, Storage::Ids);
    assert_eq!(header.version, "1.0");
}

#[test]
fn an_unknown_version_is_refused_by_name() {
    for version in ["3.0", "0.9", "2"] {
        let mut lines = minimal();
        lines[0] = Box::leak(format!("ics_version\t{version}").into_boxed_str());
        let message = refusal(&lines);
        assert!(
            message.contains(&format!("ICS version {version}")),
            "{message}"
        );
    }
}

#[test]
fn a_missing_version_is_not_an_ics_header() {
    let lines: Vec<&str> = minimal()
        .into_iter()
        .filter(|l| !l.starts_with("ics_version"))
        .collect();
    assert!(refusal(&lines).contains("ics_version"));
}

#[test]
fn the_c_axis_is_read_as_ch() {
    let mut lines = minimal();
    lines[2] = "layout\torder\tbits\tc\tx\ty\tz";
    lines[3] = "layout\tsizes\t16\t3\t4\t3\t2";
    lines[1] = "layout\tparameters\t5";
    let header = parse(&lines).expect("the header parses");
    assert_eq!(header.axes, vec!["ch", "x", "y", "z"]);
}

#[test]
fn an_unknown_or_repeated_axis_is_refused_by_name() {
    for (order, named) in [("x\ty\tq", "q"), ("x\ty\tx", "x")] {
        let mut lines = minimal();
        lines[2] = Box::leak(format!("layout\torder\tbits\t{order}").into_boxed_str());
        lines[1] = "layout\tparameters\t4";
        lines[3] = "layout\tsizes\t16\t4\t3\t2";
        let message = refusal(&lines);
        assert!(message.contains(&format!("axis \"{named}\"")), "{message}");
    }
}

#[test]
fn a_missing_x_or_y_is_refused_by_name() {
    for order in ["y\tz", "x\tz"] {
        let mut lines = minimal();
        lines[2] = Box::leak(format!("layout\torder\tbits\t{order}").into_boxed_str());
        lines[1] = "layout\tparameters\t3";
        lines[3] = "layout\tsizes\t16\t4\t3";
        let message = refusal(&lines);
        assert!(message.contains("does not name axis"), "{message}");
    }
}

#[test]
fn layout_parameters_must_count_the_entries() {
    for parameters in ["1", "2", "3", "6"] {
        let mut lines = minimal();
        lines[1] = Box::leak(format!("layout\tparameters\t{parameters}").into_boxed_str());
        let message = refusal(&lines);
        assert!(message.contains(&format!("is {parameters}")), "{message}");
    }
    let mut lines = minimal();
    lines[1] = "layout\tparameters\t4";
    assert!(parse(&lines).is_ok(), "the count the entries imply parses");
}

#[test]
fn the_stride_of_the_fastest_axis_is_one_and_each_next_one_multiplies() {
    let (_, layout) = geometry(&minimal(), None).expect("the layout builds");
    assert_eq!(layout.stride("x"), Some(1));
    assert_eq!(layout.stride("y"), Some(4));
    assert_eq!(layout.stride("z"), Some(12));
    assert_eq!(layout.stride("t"), None, "an absent axis has no stride");
    assert_eq!(layout.plane_samples(0, 0, 1), Some(12));
    assert!(layout.contiguous(), "x and y are the two fastest axes");
    let dims = layout.dims;
    assert_eq!((dims.x, dims.y, dims.z, dims.c, dims.t), (4, 3, 2, 1, 1));
}

#[test]
fn every_sample_type_maps_to_its_dtype_and_the_others_are_refused() {
    for (format, sign, bits, dtype) in [
        ("integer", "unsigned", 8, DType::U8),
        ("integer", "unsigned", 16, DType::U16),
        ("integer", "signed", 16, DType::I16),
        ("real", "signed", 32, DType::F32),
    ] {
        let mut lines = minimal();
        lines[3] = match bits {
            8 => "layout\tsizes\t8\t4\t3\t2",
            16 => "layout\tsizes\t16\t4\t3\t2",
            _ => "layout\tsizes\t32\t4\t3\t2",
        };
        lines[4] = Box::leak(format!("representation\tformat\t{format}").into_boxed_str());
        lines[5] = Box::leak(format!("representation\tsign\t{sign}").into_boxed_str());
        lines[7] = Box::leak(
            format!(
                "representation\tbyte_order\t{}",
                (1..=bits / 8)
                    .map(|b| b.to_string())
                    .collect::<Vec<String>>()
                    .join(" ")
            )
            .into_boxed_str(),
        );
        let (_, layout) = geometry(&lines, None).expect("the layout builds");
        assert_eq!(layout.dtype, dtype, "{format} {sign} {bits}");
    }
}

#[test]
fn the_byte_order_line_tells_the_endianness() {
    let mut lines = minimal();
    lines[7] = "representation\tbyte_order\t2 1";
    let (_, layout) = geometry(&lines, None).expect("the layout builds");
    assert_eq!(layout.endian, Endian::Big);

    let mut lines = minimal();
    lines[3] = "layout\tsizes\t8\t4\t3\t2";
    lines[7] = "representation\tbyte_order\t1";
    let (_, layout) = geometry(&lines, None).expect("the layout builds");
    assert_eq!(layout.endian, Endian::Little);

    // A one-byte sample has no order to state, so an absent line is unambiguous.
    let mut lines = minimal();
    lines[3] = "layout\tsizes\t8\t4\t3\t2";
    lines[7] = "";
    lines.retain(|l| !l.is_empty());
    let (_, layout) = geometry(&lines, None).expect("the layout builds");
    assert_eq!(layout.dtype, DType::U8);
}

#[test]
fn the_data_must_fit_the_file() {
    let header = parse(&minimal()).expect("the header parses");
    let pixels = 4 * 3 * 2 * 2;
    let (header, geometry_layout) =
        geometry(&minimal(), Some(header.data_offset + pixels)).expect("the data fits");
    assert_eq!(geometry_layout.compression(), Compression::None);
    assert_eq!(
        geometry_layout.data_offset(),
        header.data_offset,
        "the pixels follow the header"
    );

    let lines = minimal();
    match layout(&parse(&lines).expect("the header parses"), Some(10)) {
        Err(Error::Parse(message)) => assert!(message.contains("past the end of the"), "{message}"),
        Err(other) => panic!("expected Parse, got {other:?}"),
        Ok(_) => panic!("expected Parse, got a layout"),
    }
}

#[test]
fn an_uncompressed_plane_over_the_decode_ceiling_is_refused_before_it_is_allocated() {
    // A stored plane of 4.9 GB: the ceiling prices it before a byte of it is reserved. Such a file would have to
    // hold the whole plane, so the layout is the level that refuses it.
    let mut lines = minimal();
    lines[1] = "layout\tparameters\t3";
    lines[2] = "layout\torder\tbits\tx\ty";
    lines[3] = "layout\tsizes\t8\t70000\t70000";
    lines[7] = "representation\tbyte_order\t1";
    let header = parse(&lines).expect("the header parses");
    let layout = layout(&header, None).expect("the layout builds");
    match layout.plane_bytes() {
        Err(Error::OversizedAllocation { requested, cap }) => {
            assert!(requested > cap, "the plane is over the ceiling");
        }
        other => panic!("expected OversizedAllocation, got {other:?}"),
    }
}

#[test]
fn the_stated_sizes_convert_to_um_only_with_a_known_unit() {
    for (unit, expected) in [
        ("micrometers", 0.5f32),
        ("micrometer", 0.5),
        ("microns", 0.5),
        ("um", 0.5),
        ("\u{b5}m", 0.5),
        ("nanometers", 0.0005),
        ("millimeters", 500.0),
    ] {
        let mut lines = minimal();
        lines[9] =
            Box::leak(format!("parameter\tunits\t3\t{unit}\t{unit}\t{unit}").into_boxed_str());
        let (header, _) = geometry(&lines, None).expect("the layout builds");
        let calibration = super::calibration::calibrate(&header);
        let px = calibration.px_size_um.expect("a pixel size");
        assert!((px.x - expected).abs() < 1e-9, "{unit}: {}", px.x);
        assert!((px.y - expected).abs() < 1e-9, "{unit}: {}", px.y);
    }
}

#[test]
fn an_unknown_unit_or_a_non_positive_scale_leaves_the_axis_uncalibrated() {
    for (units, scale) in [
        ("furlongs", "0.5"),
        ("micrometers", "0"),
        ("micrometers", "-1"),
    ] {
        let mut lines = minimal();
        lines[8] =
            Box::leak(format!("parameter\tscale\t3\t{scale}\t{scale}\t{scale}").into_boxed_str());
        lines[9] =
            Box::leak(format!("parameter\tunits\t3\t{units}\t{units}\t{units}").into_boxed_str());
        let (header, _) = geometry(&lines, None).expect("the layout builds");
        let calibration = super::calibration::calibrate(&header);
        assert!(calibration.px_size_um.is_none(), "{units} {scale}");
    }
}
