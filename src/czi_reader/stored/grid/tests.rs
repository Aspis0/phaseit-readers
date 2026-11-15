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

/// The tiles that share the cell at the origin of the lattice: the cell a tile sits in is its
/// place in the lattice.
fn tiles_of_the_first_cell(tiles: &[StoredTile], cell: &[usize; 2], origin: &[i64; 2]) -> usize {
    tiles
        .iter()
        .filter(|tile| {
            (tile.x - origin[0]).div_euclid(cell[0] as i64) == 0
                && (tile.y - origin[1]).div_euclid(cell[1] as i64) == 0
        })
        .count()
}

/// A cell of many tiles that do not overlap: the check must say so, whatever the count is.
#[test]
fn a_cell_of_sixty_five_tiles_that_do_not_overlap_is_clear() {
    // 65 tiles of 8 pixels in a cell of 520: they share the cell and stand side by side.
    let tiles = row_of(65, 8);
    let cell = [520usize, 8];
    assert_eq!(
        tiles_of_the_first_cell(&tiles, &cell, &[0, 0]),
        65,
        "the cell holds all 65 tiles"
    );
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

/// A cell that holds more tiles than the check may compare is refused by its count, not called an
/// overlap.
#[test]
fn a_cell_that_holds_more_tiles_than_the_check_may_compare_names_its_count() {
    // 129 tiles cost 8256 comparisons, more than the 8128 one cell may cost.
    let tiles = row_of(129, 1);
    let cell = [129usize, 1];
    assert_eq!(
        overlap_of(&tiles, &cell, &[0, 0]),
        Overlap::TooManyTiles {
            tiles: 129,
            comparisons: 8_256
        }
    );
}

/// A cell of the largest count the check may compare is checked, and found clear.
#[test]
fn a_cell_of_the_largest_count_the_check_may_compare_is_checked() {
    // 128 tiles cost 8128 comparisons, what one cell may cost.
    let tiles = row_of(128, 1);
    let cell = [128usize, 1];
    assert_eq!(overlap_of(&tiles, &cell, &[0, 0]), Overlap::Clear);
}

/// A level of many cells is checked whatever the number of its cells: the cost of the check grows
/// with the level, and a level the reader read before is still read.
#[test]
fn a_level_of_many_cells_is_checked() {
    // 500 cells of 64 tiles: the level costs 1 008 000 comparisons, and every cell is checked.
    let mut tiles = Vec::new();
    for cell in 0..500i64 {
        for index in 0..64i64 {
            tiles.push(tile(cell * 64 + index, 0, 1, (0, 0, 0)));
        }
    }
    assert_eq!(overlap_of(&tiles, &[64, 1], &[0, 0]), Overlap::Clear);
}
