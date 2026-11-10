use std::collections::HashMap;

use crate::czi_reader::scene::CziScene;
use crate::czi_reader::subblock::SubBlock;
use crate::error::Error;
use crate::types::{DType, Dims5, PixelRect};

use super::tile_overlap;

fn tile(start_x: i32, start_y: i32) -> SubBlock {
    SubBlock {
        pixel_off: 0,
        data_size: 0,
        compression: 0,
        dtype: DType::U8,
        samples: 1,
        m: -1,
        start_x,
        start_y,
        stored_x: 4,
        stored_y: 4,
    }
}

fn scene() -> CziScene {
    CziScene {
        dims: Dims5 { t: 1, c: 1, z: 1, y: 16, x: 16 },
        dtype: DType::U8,
        samples: 1,
        c_values: vec![0],
        z_values: vec![0],
        t_values: vec![0],
        origin_x: 100,
        origin_y: 100,
        tiles: HashMap::new(),
    }
}

fn whole_plane() -> PixelRect {
    PixelRect { x: 0, y: 0, w: 16, h: 16 }
}

#[test]
fn a_tile_at_the_scene_origin_overlaps_the_plane() {
    let overlap = tile_overlap(&tile(100, 100), &scene(), whole_plane(), 16, 16);
    assert!(matches!(overlap, Ok(Some(_))));
}

#[test]
fn a_tile_left_of_the_scene_origin_is_a_decode_error() {
    match tile_overlap(&tile(99, 100), &scene(), whole_plane(), 16, 16) {
        Err(Error::Decode(msg)) => assert_eq!(msg, "CZI tile placed before scene origin"),
        _ => panic!("a tile left of the origin must be a Decode error"),
    }
}

#[test]
fn a_tile_above_the_scene_origin_is_a_decode_error() {
    match tile_overlap(&tile(100, 99), &scene(), whole_plane(), 16, 16) {
        Err(Error::Decode(msg)) => assert_eq!(msg, "CZI tile placed before scene origin"),
        _ => panic!("a tile above the origin must be a Decode error"),
    }
}
