use crate::error::{Error, Result};

use super::blit_tile;

fn blit_at(col_off: i64, row_off: i64) -> Result<()> {
    let mut composed = vec![0u8; 16];
    blit_tile(&mut composed, 4, 4, 4, 1, &[0u8; 4], 2, 2, col_off, row_off)
}

#[test]
fn a_tile_inside_the_scene_is_blitted() {
    assert!(blit_at(0, 0).is_ok());
}

#[test]
fn a_tile_left_of_the_scene_origin_is_a_decode_error() {
    match blit_at(-1, 0) {
        Err(Error::Decode(msg)) => assert_eq!(msg, "CZI tile placed before scene origin"),
        _ => panic!("a tile left of the origin must be a Decode error"),
    }
}

#[test]
fn a_tile_above_the_scene_origin_is_a_decode_error() {
    match blit_at(0, -1) {
        Err(Error::Decode(msg)) => assert_eq!(msg, "CZI tile placed before scene origin"),
        _ => panic!("a tile above the origin must be a Decode error"),
    }
}
