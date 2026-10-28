// SPDX-License-Identifier: Apache-2.0
//! The decision a reader makes about a scene whose plane is over the decode
//! ceiling.
//!
//! The ceiling ([`MAX_DECODED_PLANE_BYTES`]) guards ONE materialisation, not a
//! scene's existence. A scene whose plane is too big to hold in RAM is still
//! worth publishing when the reader can serve it window by window: `plane()`
//! refuses the materialisation and `region()` serves the windows. That is what
//! the CZI reader has always done for a whole-slide scene, and what the LIF
//! and ND2 readers do too.
//!
//! The decision is made in ONE place, [`Dims5::plane_bytes_no_ceiling`], which
//! sizes a plane and refuses it only when the byte arithmetic OVERFLOWS. This
//! test pins that decision directly: an over-ceiling plane must be SIZED (so
//! the scene is published) while the same plane is refused where it is
//! materialised.

use phasit_imread::types::Dims5;
use phasit_imread::{DType, Error, MAX_DECODED_PLANE_BYTES};

/// A 70000 x 70000 U8 plane: 4.9 GB, past the 4 GiB decode ceiling.
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
    // The decision the LIF and ND2 readers make at open: the plane's size is
    // computed, so the scene is PUBLISHED rather than refused.
    let dims = over_ceiling_plane();
    assert_eq!(
        dims.plane_bytes_no_ceiling(DType::U8).expect("the size fits a usize"),
        70_000 * 70_000,
        "an over-ceiling plane is still sized"
    );
}

#[test]
fn an_over_ceiling_plane_is_refused_where_it_is_materialised() {
    // The same plane, refused where the whole plane is materialised: this is
    // what `plane()` returns for such a scene.
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
    // A size that does not fit a `usize` is refused on every path: there is no
    // way to lay out such a plane, so it is not a scene any reader can serve.
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
