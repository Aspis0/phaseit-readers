//! The grid the tiles of a stored level make: its cell size, its lattice, and whether the tiles
//! fit it without overlapping.

use std::collections::HashMap;

use crate::czi_reader::scene::CziScene;

use super::levels::StoredTile;

/// The most tiles one grid cell may hold: the overlap check is quadratic inside a cell.
pub(super) const MAX_TILES_PER_CELL: usize = 64;

/// The grid's cell size: the size of a whole tile of the level.
pub(super) fn cell_of(tiles: &[StoredTile]) -> [usize; 2] {
    let mut cell = [0usize; 2];
    for tile in tiles {
        cell[0] = cell[0].max(tile.source_w as usize);
        cell[1] = cell[1].max(tile.source_h as usize);
    }
    cell
}

/// The axis a grid line is measured on.
#[derive(Clone, Copy)]
enum Axis {
    X,
    Y,
}

/// The grid's origin: the last grid line at or before the scene's origin.
///
/// WHY: the file's pyramid is the pyramid of the whole acquisition and the scene is a crop
/// of it, so the grid starts before the scene does. The origin is the line that keeps the
/// grid over the scene.
pub(super) fn lattice_origin(tiles: &[StoredTile], cell: &[usize; 2]) -> Option<[i64; 2]> {
    if cell[0] == 0 || cell[1] == 0 {
        return None;
    }
    let residue_x = residue_of(tiles, cell, Axis::X)?;
    let residue_y = residue_of(tiles, cell, Axis::Y)?;
    // The tiles sit in the scene's frame, so the scene's origin is the origin of that frame.
    Some([
        grid_line(0, residue_x, cell[0] as i64),
        grid_line(0, residue_y, cell[1] as i64),
    ])
}

/// The grid line at or before `position` that carries the grid's residue.
fn grid_line(position: i64, residue: i64, cell: i64) -> i64 {
    position - (position - residue).rem_euclid(cell)
}

/// The grid's residue along one axis: where the whole tiles' starts sit modulo the cell.
fn residue_of(tiles: &[StoredTile], cell: &[usize; 2], axis: Axis) -> Option<i64> {
    let whole: Vec<i64> = tiles
        .iter()
        .filter(|tile| whole_tile(tile, cell, axis))
        .map(|tile| match axis {
            Axis::X => tile.x,
            Axis::Y => tile.y,
        })
        .collect();
    let residue = whole.first()?.rem_euclid(cell[axis_index(axis)] as i64);
    // Every whole tile must sit on the same grid line, or the grid is not a grid.
    whole
        .iter()
        .all(|&start| start.rem_euclid(cell[axis_index(axis)] as i64) == residue)
        .then_some(residue)
}

/// The index of an axis in a pair.
fn axis_index(axis: Axis) -> usize {
    match axis {
        Axis::X => 0,
        Axis::Y => 1,
    }
}

/// Whether a tile fills its cell along one axis: its source extent is the whole cell.
fn whole_tile(tile: &StoredTile, cell: &[usize; 2], axis: Axis) -> bool {
    match axis {
        Axis::X => tile.source_w as usize == cell[0],
        Axis::Y => tile.source_h as usize == cell[1],
    }
}

/// Whether every tile sits inside the cell of the grid it belongs to.
pub(super) fn tiles_fit_their_cells(
    tiles: &[StoredTile],
    cell: &[usize; 2],
    origin: &[i64; 2],
) -> bool {
    tiles.iter().all(|tile| {
        let x0 = tile.x - origin[0];
        let y0 = tile.y - origin[1];
        let cell_x = x0.div_euclid(cell[0] as i64) * cell[0] as i64;
        let cell_y = y0.div_euclid(cell[1] as i64) * cell[1] as i64;
        x0 >= cell_x
            && y0 >= cell_y
            && x0 + tile.source_w <= cell_x + cell[0] as i64
            && y0 + tile.source_h <= cell_y + cell[1] as i64
    })
}

/// Whether two tiles of the level do not cover the same pixels.
pub(super) fn tiles_do_not_overlap(
    tiles: &[StoredTile],
    cell: &[usize; 2],
    origin: &[i64; 2],
) -> bool {
    let mut cells: HashMap<(i64, i64), Vec<&StoredTile>> = HashMap::new();
    for tile in tiles {
        let key = (
            (tile.x - origin[0]).div_euclid(cell[0] as i64),
            (tile.y - origin[1]).div_euclid(cell[1] as i64),
        );
        cells.entry(key).or_default().push(tile);
    }
    for group in cells.values() {
        if group.len() > MAX_TILES_PER_CELL {
            return false;
        }
        for (index, first) in group.iter().enumerate() {
            for second in &group[index + 1..] {
                let overlaps = first.x < second.x + second.source_w
                    && second.x < first.x + first.source_w
                    && first.y < second.y + second.source_h
                    && second.y < first.y + first.source_h;
                if overlaps {
                    return false;
                }
            }
        }
    }
    true
}

/// The level's extent: the box its tiles cover, in source pixels.
pub(super) fn extent_of(tiles: &[StoredTile]) -> [usize; 2] {
    let mut min_x = i64::MAX;
    let mut min_y = i64::MAX;
    let mut max_x = i64::MIN;
    let mut max_y = i64::MIN;
    for tile in tiles {
        min_x = min_x.min(tile.x);
        min_y = min_y.min(tile.y);
        max_x = max_x.max(tile.x + tile.source_w);
        max_y = max_y.max(tile.y + tile.source_h);
    }
    [(max_x - min_x) as usize, (max_y - min_y) as usize]
}

/// Whether the level's tiles reach the scene's edges.
///
/// WHY: the scene is a crop of the image the pyramid was made of, so the pyramid's grid
/// starts before the scene and ends after it. A level that stops short of the scene by more
/// than a cell belongs to another image, and its pixels would be shifted.
pub(super) fn covers_the_scene(
    tiles: &[StoredTile],
    extent: &[usize; 2],
    cell: &[usize; 2],
    scene: &CziScene,
) -> bool {
    let min_x = tiles.iter().map(|tile| tile.x).min().unwrap_or(0);
    let min_y = tiles.iter().map(|tile| tile.y).min().unwrap_or(0);
    let width = scene.dims.x as i64;
    let height = scene.dims.y as i64;
    let margins = [
        min_x.max(0),
        min_y.max(0),
        (width - min_x - extent[0] as i64).max(0),
        (height - min_y - extent[1] as i64).max(0),
    ];
    let cells = [
        cell[0] as i64,
        cell[1] as i64,
        cell[0] as i64,
        cell[1] as i64,
    ];
    margins
        .iter()
        .zip(cells.iter())
        .all(|(margin, cell)| *margin < *cell)
}

/// Divides and rounds to the nearest, halves up.
pub(super) fn div_round(a: i64, b: i64) -> i64 {
    let quotient = a.div_euclid(b);
    let remainder = a.rem_euclid(b);
    if remainder * 2 >= b {
        quotient + 1
    } else {
        quotient
    }
}

/// Divides and rounds up.
pub(super) fn div_ceil(a: i64, b: i64) -> i64 {
    let quotient = a.div_euclid(b);
    if a.rem_euclid(b) > 0 {
        quotient + 1
    } else {
        quotient
    }
}
