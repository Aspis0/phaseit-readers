use crate::error::Error;
use crate::types::{DType, Dims5, ImageReader, PlaneBuf};

use super::support::*;

// 2000 single-tile scenes: the O(N) grouping must pick the right scene, including a high-S one.
#[test]
fn many_scenes_group_linearly_and_high_scene_decodes() {
    const N: i32 = 2000;
    let mut tiles = Vec::with_capacity(N as usize);
    for s in 0..N {
        tiles.push(RawTile {
            pixel_type: 0,
            dims: vec![(b'X', 0, 1), (b'Y', 0, 1), (b'S', s, 1)],
            pixels: vec![(s % 256) as u8],
            bad_file_pos: None,
        });
    }
    let reader = open_synth(&build_czi_raw(&tiles));
    assert_eq!(reader.meta().scene_count(), N as usize, "one scene per S value");

    let top = (N - 1) as usize;
    let expect_px = ((N - 1) % 256) as u8;
    let p = reader.plane(top, 0, 0, 0).expect("decode plane from highest-S scene");
    assert_eq!(p.shape, [1, 1]);
    match p.data {
        PlaneBuf::U8(v) => assert_eq!(v, vec![expect_px]),
        other => panic!("expected U8, got {:?}", other.dtype()),
    }
}

#[test]
fn heterogeneous_scene_is_refused_not_aborted() {
    let tiles = vec![
        RawTile {
            pixel_type: 0,
            dims: vec![(b'X', 0, 2), (b'Y', 0, 2), (b'S', 0, 1)],
            pixels: vec![10, 20, 30, 40],
            bad_file_pos: None,
        },
        RawTile {
            pixel_type: 0,
            dims: vec![(b'X', 0, 2), (b'Y', 0, 2), (b'S', 1, 1), (b'M', 0, 1)],
            pixels: vec![1, 2, 3, 4],
            bad_file_pos: None,
        },
        RawTile {
            pixel_type: 3,
            dims: vec![(b'X', 0, 2), (b'Y', 0, 2), (b'S', 1, 1), (b'M', 1, 1)],
            pixels: vec![0u8; 12],
            bad_file_pos: None,
        },
    ];
    let reader = open_synth(&build_czi_raw(&tiles));
    assert_eq!(reader.meta().scene_count(), 1, "heterogeneous scene refused");
    let scene = reader.meta().scene(0).unwrap();
    assert_eq!(scene.dims, Dims5 { t: 1, c: 1, z: 1, y: 2, x: 2 });
    let p = reader.plane(0, 0, 0, 0).unwrap();
    match p.data {
        PlaneBuf::U8(v) => assert_eq!(v, vec![10, 20, 30, 40]),
        other => panic!("expected U8, got {:?}", other.dtype()),
    }
}

#[test]
fn all_scenes_heterogeneous_fails_closed() {
    let tiles = vec![
        RawTile { pixel_type: 0, dims: vec![(b'X', 0, 2), (b'Y', 0, 2), (b'S', 0, 1), (b'M', 0, 1)], pixels: vec![1, 2, 3, 4], bad_file_pos: None },
        RawTile { pixel_type: 3, dims: vec![(b'X', 0, 2), (b'Y', 0, 2), (b'S', 0, 1), (b'M', 1, 1)], pixels: vec![0u8; 12], bad_file_pos: None },
    ];
    let res = try_open_synth(&build_czi_raw(&tiles));
    assert!(matches!(res, Err(Error::Unsupported(_))), "all-heterogeneous must fail closed");
}

// A non-subblock FilePosition (0 is the file header) loses one tile: that scene is refused, the other still opens.
#[test]
fn a_scene_that_lost_a_corrupt_tile_is_refused_not_aborted() {
    let tiles = vec![
        RawTile {
            pixel_type: 0,
            dims: vec![(b'X', 0, 2), (b'Y', 0, 2), (b'S', 0, 1)],
            pixels: vec![10, 20, 30, 40],
            bad_file_pos: None,
        },
        RawTile {
            pixel_type: 0,
            dims: vec![(b'X', 0, 2), (b'Y', 0, 2), (b'S', 1, 1), (b'C', 0, 1)],
            pixels: vec![50, 60, 70, 80],
            bad_file_pos: None,
        },
        RawTile {
            pixel_type: 0,
            dims: vec![(b'X', 0, 2), (b'Y', 0, 2), (b'S', 1, 1), (b'C', 1, 1)],
            pixels: vec![90, 100, 110, 120],
            bad_file_pos: Some(0),
        },
    ];
    let reader = open_synth(&build_czi_raw(&tiles));
    assert_eq!(
        reader.meta().scene_count(),
        1,
        "the scene that lost a tile is refused, not published smaller"
    );
    let scene = reader.meta().scene(0).unwrap();
    assert_eq!(scene.dims, Dims5 { t: 1, c: 1, z: 1, y: 2, x: 2 });
    let p = reader.plane(0, 0, 0, 0).unwrap();
    match p.data {
        PlaneBuf::U8(v) => assert_eq!(v, vec![10, 20, 30, 40]),
        other => panic!("expected U8, got {:?}", other.dtype()),
    }
    let refused = reader.refused_scenes();
    assert_eq!(refused.len(), 1, "the scene that lost a tile is refused");
    assert!(
        refused[0].reason.contains("out of bounds"),
        "the reason names the cause: {}",
        refused[0].reason
    );
}

#[test]
fn all_subblock_spans_corrupt_fails_closed() {
    let tiles = vec![RawTile {
        pixel_type: 0,
        dims: vec![(b'X', 0, 2), (b'Y', 0, 2)],
        pixels: vec![1, 2, 3, 4],
        bad_file_pos: Some(0),
    }];
    let res = try_open_synth(&build_czi_raw(&tiles));
    assert!(matches!(res, Err(Error::Unsupported(_))), "no usable subblock => fail closed");
}

// Packed T (T.Size = 2, line-scan style) must fail loud, never collapse to T = 1.
#[test]
fn packed_axis_subblock_is_unsupported_not_silent() {
    let tiles = vec![RawTile {
        pixel_type: 0,
        dims: vec![(b'X', 0, 2), (b'Y', 0, 2), (b'T', 0, 2)],
        pixels: vec![1, 2, 3, 4],
        bad_file_pos: None,
    }];
    let res = try_open_synth(&build_czi_raw(&tiles));
    assert!(
        matches!(res, Err(Error::Unsupported(_))),
        "packed T.Size>1 must be Unsupported"
    );
}

// An unmodelled PixelType loses its tile, not the file: that scene is refused, the other still opens.
#[test]
fn a_scene_that_lost_an_unmodelled_tile_is_refused_not_aborted() {
    let tiles = vec![
        RawTile {
            pixel_type: 0,
            dims: vec![(b'X', 0, 2), (b'Y', 0, 2), (b'S', 0, 1)],
            pixels: vec![10, 20, 30, 40],
            bad_file_pos: None,
        },
        RawTile {
            pixel_type: 0,
            dims: vec![(b'X', 0, 2), (b'Y', 0, 2), (b'S', 1, 1)],
            pixels: vec![50, 60, 70, 80],
            bad_file_pos: None,
        },
        RawTile {
            pixel_type: 10,
            dims: vec![(b'X', 0, 2), (b'Y', 0, 2), (b'S', 1, 1)],
            pixels: vec![0u8; 16],
            bad_file_pos: None,
        },
    ];
    let reader = open_synth(&build_czi_raw(&tiles));
    assert_eq!(
        reader.meta().scene_count(),
        1,
        "the scene that lost a tile is refused, not published smaller"
    );
    let scene = reader.meta().scene(0).unwrap();
    assert_eq!(scene.dims, Dims5 { t: 1, c: 1, z: 1, y: 2, x: 2 });
    assert_eq!(scene.dtype, DType::U8);
    let p = reader.plane(0, 0, 0, 0).unwrap();
    match p.data {
        PlaneBuf::U8(v) => assert_eq!(v, vec![10, 20, 30, 40]),
        other => panic!("expected U8, got {:?}", other.dtype()),
    }
    let refused = reader.refused_scenes();
    assert_eq!(refused.len(), 1, "the scene that lost a tile is refused");
    assert!(
        refused[0].reason.contains("pixel type"),
        "the reason names the cause: {}",
        refused[0].reason
    );
}

#[test]
fn synthetic_out_of_range() {
    let reader = open_synth(&synthetic_czi());
    assert!(matches!(
        reader.plane(0, 0, 1, 0),
        Err(Error::OutOfRange { c: 1, .. })
    ));
    assert!(matches!(
        reader.plane(0, 1, 0, 0),
        Err(Error::OutOfRange { t: 1, .. })
    ));
    assert!(matches!(
        reader.plane(0, 0, 0, 1),
        Err(Error::OutOfRange { z: 1, .. })
    ));
    assert!(matches!(
        reader.plane(1, 0, 0, 0),
        Err(Error::SceneOutOfRange { scene: 1, count: 1 })
    ));
}
