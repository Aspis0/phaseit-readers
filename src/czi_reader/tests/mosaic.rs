use crate::types::{DType, Dims5, ImageReader, PixelRect, PlaneBuf};

use super::support::*;

#[test]
fn synthetic_mosaic_geometry_and_composition() {
    let reader = open_synth(&synthetic_czi());
    assert_eq!(reader.meta().scene_count(), 1);
    let scene = reader.meta().scene(0).unwrap();
    assert_eq!(scene.dims, Dims5 { t: 1, c: 1, z: 1, y: 2, x: 4 });
    assert_eq!(scene.dtype, DType::U8);

    // Tile A (1..4) at x=0 and tile B (5..8) at x=2 compose to rows [1,2,5,6] and [3,4,7,8].
    let p = reader.plane(0, 0, 0, 0).unwrap();
    assert_eq!(p.shape, [2, 4]);
    match p.data {
        PlaneBuf::U8(v) => assert_eq!(v, vec![1, 2, 5, 6, 3, 4, 7, 8]),
        other => panic!("expected U8, got {:?}", other.dtype()),
    }
}

/// Overlapping tiles whose directory order is the REVERSE of their mosaic order: ascending M draws the M=1 tile last,
/// so the overlap column reads 11/13; the directory order would give 20/22 (czifile.py:1258-1259).
#[test]
fn overlapping_tiles_composite_in_mosaic_index_order() {
    let reader = open_synth(&build_czi(&[
        TileSpec {
            pixel_type: 0,
            xs: 0,
            xsz: 2,
            ys: 0,
            ysz: 2,
            c: 0,
            z: 0,
            m: 1,
            pixels: vec![10, 11, 12, 13],
        },
        TileSpec {
            pixel_type: 0,
            xs: 1,
            xsz: 2,
            ys: 0,
            ysz: 2,
            c: 0,
            z: 0,
            m: 0,
            pixels: vec![20, 21, 22, 23],
        },
    ]));

    let scene = reader.meta().scene(0).unwrap();
    assert_eq!(scene.dims, Dims5 { t: 1, c: 1, z: 1, y: 2, x: 3 });

    let p = reader.plane(0, 0, 0, 0).expect("compose full plane");
    assert_eq!(p.shape, [2, 3]);
    match p.data {
        PlaneBuf::U8(v) => assert_eq!(
            v,
            vec![10, 11, 21, 12, 13, 23],
            "the tile with the higher mosaic index must be drawn on top"
        ),
        other => panic!("expected U8, got {:?}", other.dtype()),
    }

    // The out-of-core path composes in the same order.
    let overlap = reader
        .region(0, 0, 0, 0, PixelRect::new(1, 0, 1, 2))
        .expect("read the overlap column");
    assert_eq!(overlap.shape, [2, 1]);
    match overlap.data {
        PlaneBuf::U8(v) => assert_eq!(v, vec![11, 13]),
        other => panic!("expected U8, got {:?}", other.dtype()),
    }
}

/// An absent `M` is index -1 (czifile.py:4598): in a sorted plane it is drawn below any non-negative `M`.
#[test]
fn absent_mosaic_axis_draws_below_the_mosaic_tiles() {
    let reader = open_synth(&build_czi_raw(&[
        mosaic_tile(0, &[(b'M', 0, 1)], vec![10, 11, 12, 13]),
        mosaic_tile(1, &[], vec![20, 21, 22, 23]),
    ]));
    assert_eq!(
        mosaic_plane(&reader, 3),
        vec![10, 11, 21, 12, 13, 23],
        "the tile without a mosaic dimension is drawn first, so the M=0 tile is on top"
    );
}

/// All-negative `M` planes keep directory order: the sort runs only with a non-negative index (czifile.py:1308).
#[test]
fn plane_with_only_negative_mosaic_indices_keeps_directory_order() {
    let reader = open_synth(&build_czi_raw(&[
        mosaic_tile(0, &[(b'M', -3, 1)], vec![10, 11, 12, 13]),
        mosaic_tile(1, &[(b'M', -5, 1)], vec![20, 21, 22, 23]),
    ]));
    assert_eq!(
        mosaic_plane(&reader, 3),
        vec![10, 20, 21, 12, 22, 23],
        "a plane with no non-negative mosaic index keeps its directory order"
    );
}

#[test]
fn mixed_plane_with_only_negative_mosaic_indices_keeps_directory_order() {
    let reader = open_synth(&build_czi_raw(&[
        mosaic_tile(0, &[], vec![10, 11, 12, 13]),
        mosaic_tile(1, &[(b'M', -5, 1)], vec![20, 21, 22, 23]),
        mosaic_tile(2, &[], vec![30, 31, 32, 33]),
    ]));
    assert_eq!(
        mosaic_plane(&reader, 4),
        vec![10, 20, 30, 31, 12, 22, 32, 33],
        "a mixed plane with no non-negative mosaic index keeps its directory order"
    );
}

#[test]
fn sorted_mixed_plane_draws_the_negative_mosaic_index_first() {
    let reader = open_synth(&build_czi_raw(&[
        mosaic_tile(0, &[], vec![10, 11, 12, 13]),
        mosaic_tile(1, &[(b'M', -5, 1)], vec![20, 21, 22, 23]),
        mosaic_tile(2, &[(b'M', 0, 1)], vec![30, 31, 32, 33]),
    ]));
    assert_eq!(
        mosaic_plane(&reader, 4),
        vec![10, 11, 30, 31, 12, 13, 32, 33],
        "a sorted mixed plane draws the negative mosaic index first"
    );
}

/// Two tiles with the SAME mosaic index keep their subblock-directory
/// order: the sort is stable.
#[test]
fn equal_mosaic_indices_keep_the_subblock_directory_order() {
    let reader = open_synth(&build_czi_raw(&[
        mosaic_tile(0, &[(b'M', 4, 1)], vec![10, 11, 12, 13]),
        mosaic_tile(1, &[(b'M', 4, 1)], vec![20, 21, 22, 23]),
    ]));
    assert_eq!(
        mosaic_plane(&reader, 3),
        vec![10, 20, 21, 12, 22, 23],
        "tiles that share a mosaic index keep their directory order"
    );
}

/// Several `M` entries: the first in file order wins (czifile.py:4609).
#[test]
fn duplicate_mosaic_dimension_takes_the_first_one() {
    let reader = open_synth(&build_czi_raw(&[
        mosaic_tile(0, &[(b'M', 7, 1), (b'M', 3, 1)], vec![10, 11, 12, 13]),
        mosaic_tile(1, &[(b'M', 5, 1)], vec![20, 21, 22, 23]),
    ]));
    assert_eq!(
        mosaic_plane(&reader, 3),
        vec![10, 11, 21, 12, 13, 23],
        "the FIRST mosaic dimension entry wins"
    );
}

/// A packed `M` (`Size > 1`) is accepted, as czifile accepts it (czifile.py:4616-4617).
#[test]
fn packed_mosaic_dimension_still_opens_the_file() {
    let reader = open_synth(&build_czi_raw(&[
        mosaic_tile(0, &[(b'M', 1, 2)], vec![10, 11, 12, 13]),
        mosaic_tile(1, &[(b'M', 0, 1)], vec![20, 21, 22, 23]),
    ]));
    assert_eq!(
        mosaic_plane(&reader, 3),
        vec![10, 11, 21, 12, 13, 23],
        "a packed mosaic axis uses its Start and does not refuse the file"
    );
}
