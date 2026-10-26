// SPDX-License-Identifier: Apache-2.0
use super::*;

/// A block that states its IFD and, optionally, its first Z plane.
fn block(ifd: &str, first_z: Option<&str>) -> TiffData {
    TiffData {
        ifd: Some(ifd.to_string()),
        first_z: first_z.map(str::to_string),
        ..TiffData::default()
    }
}

const Z_STACK: (usize, usize, usize) = (1, 1, 2);

/// A file with enough pages for every IFD the tests below name.
const FILE_IFDS: usize = 16;

#[test]
fn ome_ifd_index_puts_z_fastest_then_c() {
    let order = parse_dim_order("XYZCT").unwrap();
    let (nt, nc, nz) = (1, 2, 3);
    let idx = |t, c, z| ome_ifd_index(&order, t, c, z, nt, nc, nz).unwrap();
    assert_eq!(idx(0, 0, 0), 0);
    assert_eq!(idx(0, 0, 1), 1); // z fastest
    assert_eq!(idx(0, 1, 0), 3); // c after all z
}

#[test]
fn ome_ifd_index_overflow_is_error() {
    // Huge sizes from malformed OME-XML must error, not panic/wrap.
    let order = parse_dim_order("XYCZT").unwrap();
    let r = ome_ifd_index(&order, 0, 1, 1, 1, usize::MAX, usize::MAX);
    assert!(matches!(r, Err(Error::Decode(_))));
}

#[test]
fn dim_order_rejects_malformed() {
    assert!(parse_dim_order("XY").is_err());
    assert!(parse_dim_order("XYCZTQ").is_err());
    assert!(parse_dim_order("XYCZ").is_err());
}

#[test]
fn dim_order_rejects_non_permutation() {
    // "XYZZT" collects [Z, Z, T] -- three axes but NOT a permutation of
    // {T, C, Z} (C missing, Z duplicated).
    assert!(parse_dim_order("XYZZT").is_err());
    assert!(parse_dim_order("XYTCZ").is_ok());
}

#[test]
fn blocks_listed_out_of_order_map_each_plane_to_its_ifd() {
    let order = parse_dim_order("XYCZT").unwrap();
    let blocks = [block("2", Some("1")), block("0", Some("0"))];
    let plan = scene_plan(&blocks, None, true, order, Z_STACK, FILE_IFDS).unwrap();
    assert_eq!(plan.ifds, vec![0, 2]);
}

#[test]
fn plane_count_covers_consecutive_ifds() {
    let order = parse_dim_order("XYCZT").unwrap();
    let blocks = [TiffData {
        plane_count: Some("3".to_string()),
        ..block("1", None)
    }];
    let plan = scene_plan(&blocks, None, true, order, (1, 1, 3), FILE_IFDS).unwrap();
    assert_eq!(plan.ifds, vec![1, 2, 3]);
}

#[test]
fn plane_count_continues_in_dimension_order() {
    // XYZCT: z is fastest, so the flat order is (z0,c0), (z1,c0), (z0,c1), (z1,c1).
    // A block that starts at (z=0, c=1) and covers two planes takes the last two,
    // and the IFDs continue from its IFD (8, 9).
    let order = parse_dim_order("XYZCT").unwrap();
    let blocks = [
        TiffData {
            first_c: Some("1".to_string()),
            plane_count: Some("2".to_string()),
            ..block("8", None)
        },
        block("0", Some("0")),
        block("10", Some("1")),
    ];
    let plan = scene_plan(&blocks, None, true, order, (1, 2, 2), FILE_IFDS).unwrap();
    assert_eq!(plan.ifds, vec![0, 10, 8, 9]);
}

#[test]
fn absent_tiff_data_takes_the_file_in_order_for_the_first_scene() {
    let order = parse_dim_order("XYCZT").unwrap();
    let plan = scene_plan(&[], None, true, order, Z_STACK, 2).unwrap();
    assert_eq!(plan.ifds, vec![0, 1]);
}

#[test]
fn absent_tiff_data_refuses_a_later_scene() {
    let order = parse_dim_order("XYCZT").unwrap();
    let r = scene_plan(&[], None, false, order, Z_STACK, FILE_IFDS);
    assert!(matches!(r, Err(Error::Unsupported(_))));
}

#[test]
fn an_uncovered_plane_refuses_the_scene() {
    let order = parse_dim_order("XYCZT").unwrap();
    let r = scene_plan(&[block("0", Some("0"))], None, true, order, Z_STACK, FILE_IFDS);
    assert!(matches!(r, Err(Error::Parse(_))));
}

#[test]
fn a_block_without_ifd_or_plane_count_takes_the_files_pages() {
    // The default run is the file's IFDs (two), cut at the scene's last plane:
    // FirstZ=1 is plane 1, at IFD 0, and plane 0 is the block naming IFD 1.
    let order = parse_dim_order("XYCZT").unwrap();
    let blocks = [
        block("1", Some("0")),
        TiffData {
            first_z: Some("1".to_string()),
            ..TiffData::default()
        },
    ];
    let plan = scene_plan(&blocks, None, true, order, Z_STACK, 2).unwrap();
    assert_eq!(plan.ifds, vec![1, 0]);
}

#[test]
fn a_block_without_ifd_or_plane_count_is_refused_past_the_files_pages() {
    // Three planes but two pages in the file: the default run covers two of
    // them, so plane 2 has no IFD and the scene is refused.
    let order = parse_dim_order("XYCZT").unwrap();
    let r = scene_plan(&[TiffData::default()], None, true, order, (1, 1, 3), 2);
    assert!(matches!(r, Err(Error::Parse(_))));
}

#[test]
fn an_ifd_past_the_files_pages_refuses_the_scene() {
    // Four pages in the file; the block names IFD 4 for its only plane.
    let order = parse_dim_order("XYCZT").unwrap();
    let r = scene_plan(&[block("4", None)], None, true, order, (1, 1, 1), 4);
    assert!(matches!(r, Err(Error::Parse(_))));
}

#[test]
fn a_block_naming_an_ifd_without_a_plane_count_states_one_plane() {
    let order = parse_dim_order("XYCZT").unwrap();
    let blocks = [block("3", Some("0")), block("0", Some("1"))];
    let plan = scene_plan(&blocks, None, true, order, Z_STACK, FILE_IFDS).unwrap();
    assert_eq!(plan.ifds, vec![3, 0]);
}

#[test]
fn two_blocks_for_one_plane_refuse_the_scene() {
    let order = parse_dim_order("XYCZT").unwrap();
    let blocks = [block("0", Some("0")), block("5", Some("0")), block("1", Some("1"))];
    let r = scene_plan(&blocks, None, true, order, Z_STACK, FILE_IFDS);
    assert!(matches!(r, Err(Error::Parse(_))));
}

#[test]
fn a_block_running_past_the_scene_refuses_it() {
    let order = parse_dim_order("XYCZT").unwrap();
    let blocks = [TiffData {
        plane_count: Some("3".to_string()),
        ..block("0", None)
    }];
    let r = scene_plan(&blocks, None, true, order, Z_STACK, FILE_IFDS);
    assert!(matches!(r, Err(Error::Parse(_))));
}

#[test]
fn a_block_starting_outside_the_scene_refuses_it() {
    let order = parse_dim_order("XYCZT").unwrap();
    let r = scene_plan(&[block("0", Some("2"))], None, true, order, Z_STACK, FILE_IFDS);
    assert!(matches!(r, Err(Error::Parse(_))));
}

#[test]
fn a_non_numeric_tiff_data_attribute_refuses_the_scene() {
    // "x" must not read as the default 0.
    let order = parse_dim_order("XYCZT").unwrap();
    let r = scene_plan(&[block("x", None)], None, true, order, (1, 1, 1), FILE_IFDS);
    assert!(matches!(r, Err(Error::Parse(_))));
}

#[test]
fn a_block_naming_another_file_refuses_the_scene() {
    let order = parse_dim_order("XYCZT").unwrap();
    let blocks = [TiffData {
        uuid: Some("urn:uuid:other".to_string()),
        ..block("0", Some("0"))
    }];
    let r = scene_plan(&blocks, Some("urn:uuid:this"), true, order, (1, 1, 1), FILE_IFDS);
    assert!(matches!(r, Err(Error::Unsupported(_))));
}

#[test]
fn a_plane_table_past_the_cap_is_refused_before_it_is_allocated() {
    let order = parse_dim_order("XYCZT").unwrap();
    let r = scene_plan(&[], None, true, order, (1, 1, 1_000_000_000), FILE_IFDS);
    match r {
        Err(Error::OversizedAllocation { requested, cap }) => {
            assert_eq!(requested, 1_000_000_000 * PLANE_ENTRY_BYTES);
            assert_eq!(cap, MAX_PLANE_TABLE_BYTES);
        }
        other => panic!("expected OversizedAllocation, got {other:?}"),
    }
}

#[test]
fn a_declared_plane_count_that_overflows_is_refused_as_oversized() {
    let r = declared_planes(usize::MAX, 2, 1);
    assert!(matches!(
        r,
        Err(Error::OversizedAllocation { requested: usize::MAX, .. })
    ));
}

#[test]
fn a_plane_table_exactly_at_the_cap_is_allowed() {
    let planes = MAX_PLANE_TABLE_BYTES / PLANE_ENTRY_BYTES;
    assert_eq!(declared_planes(1, 1, planes).unwrap(), planes);
}

#[test]
fn a_block_naming_this_file_is_read() {
    let order = parse_dim_order("XYCZT").unwrap();
    let blocks = [TiffData {
        uuid: Some("urn:uuid:this".to_string()),
        ..block("0", Some("0"))
    }];
    let plan = scene_plan(&blocks, Some("urn:uuid:this"), true, order, (1, 1, 1), FILE_IFDS).unwrap();
    assert_eq!(plan.ifds, vec![0]);
}
