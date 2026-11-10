// SPDX-License-Identifier: Apache-2.0
//! A scene over the decode ceiling is still published: the ceiling guards a materialisation, not the scene's existence.
//! `Dims5::plane_bytes_no_ceiling` refuses only on overflow; `plane()` refuses, `region()` serves windows.

use phasit_imread::types::Dims5;
use phasit_imread::{DType, Error, MAX_DECODED_PLANE_BYTES};

fn over_ceiling_plane() -> Dims5 {
    Dims5 { t: 1, c: 1, z: 1, y: 70_000, x: 70_000 }
}

#[test]
fn the_fixture_really_is_over_the_ceiling() {
    let dims = over_ceiling_plane();
    let bytes = dims.plane_bytes_no_ceiling(DType::U8).expect("the size fits a usize");
    assert_eq!(bytes, 70_000 * 70_000);
    assert!(
        bytes > MAX_DECODED_PLANE_BYTES,
        "the fixture is over the ceiling: {bytes} > {MAX_DECODED_PLANE_BYTES}"
    );
}

#[test]
fn an_over_ceiling_plane_is_sized_so_the_scene_can_be_published() {
    // The decision LIF and ND2 make at open: the size is computed, so the scene is published.
    let dims = over_ceiling_plane();
    assert_eq!(
        dims.plane_bytes_no_ceiling(DType::U8).expect("the size fits a usize"),
        70_000 * 70_000,
        "an over-ceiling plane is still sized"
    );
}

#[test]
fn an_over_ceiling_plane_is_refused_where_it_is_materialised() {
    // Refused where the whole plane is materialised: what `plane()` returns for such a scene.
    let dims = over_ceiling_plane();
    match dims.checked_plane_bytes(DType::U8) {
        Err(Error::OversizedAllocation { requested, cap }) => {
            assert_eq!(requested, 70_000 * 70_000);
            assert_eq!(cap, MAX_DECODED_PLANE_BYTES);
        }
        Err(other) => panic!("the plane must be refused as oversized, got {other}"),
        Ok(bytes) => panic!("the plane must not be materialised, got {bytes} bytes"),
    }
}

#[test]
fn a_plane_within_the_ceiling_is_sized_and_materialisable() {
    // The ordinary case: both answers agree, so the two paths cannot drift.
    let dims = Dims5 { t: 1, c: 1, z: 1, y: 512, x: 512 };
    let bytes = dims.plane_bytes_no_ceiling(DType::U16).expect("the size fits a usize");
    assert_eq!(bytes, 512 * 512 * 2);
    assert_eq!(
        dims.checked_plane_bytes(DType::U16).expect("within the ceiling"),
        bytes
    );
}

#[test]
fn a_plane_whose_size_overflows_is_refused_everywhere() {
    // A size that overflows `usize` is refused on every path: no reader can lay such a plane out.
    let dims = Dims5 { t: 1, c: 1, z: 1, y: usize::MAX, x: usize::MAX };
    assert!(
        matches!(
            dims.plane_bytes_no_ceiling(DType::U8),
            Err(Error::OversizedAllocation { .. })
        ),
        "an overflowing plane is refused by the sizing path"
    );
    assert!(
        matches!(
            dims.checked_plane_bytes(DType::U8),
            Err(Error::OversizedAllocation { .. })
        ),
        "an overflowing plane is refused by the materialising path"
    );
}
