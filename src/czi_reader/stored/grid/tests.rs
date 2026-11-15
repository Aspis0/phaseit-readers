//! The overlap check on synthetic grids: a cell that holds many tiles, and two planes that sit at
//! the same place.

use super::*;

use crate::czi_reader::subblock::SubBlock;
use crate::types::DType;

/// A tile at `(x, y)` of the given size, in the given plane.
fn tile(x: i64, y: i64, size: i64, plane: (i32, i32, i32)) -> StoredTile {
    StoredTile {
        x,
        y,
        source_w: size,
        source_h: size,
        w: size as usize,
        h: size as usize,
        plane,
        block: SubBlock {
            pixel_off: 0,
            data_size: 0,
            compression: 0,
            dtype: DType::U8,
            samples: 1,
            m: 0,
            start_x: x as i32,
            start_y: y as i32,
            stored_x: size as usize,
            stored_y: size as usize,
        },
    }
}

/// `count` tiles of `size` pixels, side by side in one row.
fn row_of(count: usize, size: i64) -> Vec<StoredTile> {
    (0..count)
        .map(|index| tile(index as i64 * size, 0, size, (0, 0, 0)))
        .collect()
}

/// A cell of many tiles that do not overlap: the check must say so, whatever the count is.
#[test]
fn a_cell_of_sixty_five_tiles_that_do_not_overlap_is_clear() {
    // 65 tiles of 8 pixels in a cell of 512: they share the cell and stand side by side.
    let tiles = row_of(65, 8);
    let cell = [512usize, 8];
    assert_eq!(overlap_of(&tiles, &cell, &[0, 0]), Overlap::Clear);
}

/// The tiles of two planes may sit at the same place: they are two images of the same scene.
#[test]
fn the_tiles_of_two_planes_at_the_same_place_do_not_overlap() {
    let tiles = vec![
        tile(0, 0, 64, (0, 0, 0)),
        tile(0, 0, 64, (1, 0, 0)),
        tile(64, 0, 64, (0, 0, 0)),
        tile(64, 0, 64, (1, 0, 0)),
    ];
    assert_eq!(overlap_of(&tiles, &[64, 64], &[0, 0]), Overlap::Clear);
}

/// Two tiles of one plane at the same place do overlap.
#[test]
fn two_tiles_of_one_plane_at_the_same_place_overlap() {
    let tiles = vec![tile(0, 0, 64, (0, 0, 0)), tile(0, 0, 64, (0, 0, 0))];
    assert_eq!(overlap_of(&tiles, &[64, 64], &[0, 0]), Overlap::Overlapping);
}

/// Two tiles of one plane that touch at an edge do not overlap.
#[test]
fn two_tiles_of_one_plane_that_touch_at_an_edge_do_not_overlap() {
    let tiles = vec![tile(0, 0, 64, (0, 0, 0)), tile(64, 0, 64, (0, 0, 0))];
    assert_eq!(overlap_of(&tiles, &[64, 64], &[0, 0]), Overlap::Clear);
}

/// A cell that costs more than the check may spend is refused by its count, not called an overlap.
#[test]
fn a_cell_that_costs_more_than_the_check_may_spend_names_its_count() {
    // 1415 tiles cost 1_000_405 comparisons, one more than the check may spend.
    let tiles = row_of(1415, 1);
    let cell = [1415usize, 1];
    assert_eq!(
        overlap_of(&tiles, &cell, &[0, 0]),
        Overlap::TooManyTiles { tiles: 1415 }
    );
}

/// A cell that costs what the check may spend is checked, and found clear.
#[test]
fn a_cell_that_costs_what_the_check_may_spend_is_checked() {
    // 1414 tiles cost 998_991 comparisons, within the budget.
    let tiles = row_of(1414, 1);
    let cell = [1414usize, 1];
    assert_eq!(overlap_of(&tiles, &cell, &[0, 0]), Overlap::Clear);
}

/// The budget is spent over the whole level: cells that are cheap on their own add up.
#[test]
fn the_budget_is_spent_over_the_whole_level() {
    // 30 cells of 300 tiles: each costs 44_850 comparisons, and the level costs more than the budget.
    let mut tiles = Vec::new();
    for cell in 0..30i64 {
        for index in 0..300i64 {
            tiles.push(tile(cell * 300 + index, 0, 1, (0, 0, 0)));
        }
    }
    assert_eq!(
        overlap_of(&tiles, &[300, 1], &[0, 0]),
        Overlap::TooManyTiles { tiles: 300 }
    );
}
