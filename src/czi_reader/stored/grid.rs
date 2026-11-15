//! The grid the tiles of a stored level make: its cell size, its lattice, and whether the tiles
//! fit it without overlapping.

use std::collections::HashMap;

use crate::czi_reader::scene::CziScene;

use super::levels::StoredTile;

/// The most comparisons the overlap check may spend on one level.
///
/// WHY: the check compares every pair of the tiles that share a plane and a cell, so a cell of k
/// tiles costs k*(k-1)/2 of them. A million comparisons is a millisecond of work, so a crafted file
/// cannot make the check slow; a level that would cost more is refused by name and not called an
/// overlap.
pub(super) const MAX_OVERLAP_COMPARISONS: u64 = 1_000_000;

/// The verdict of the overlap check on the tiles of a level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Overlap {
    /// No two tiles of a plane cover the same pixels.
    Clear,
    /// Two tiles of a plane cover the same pixels.
    Overlapping,
    /// A cell holds more tiles than the check may compare; the count names them.
    TooManyTiles {
        /// The tiles the cell holds.
        tiles: usize,
    },
}

/// The cell of the grid a tile sits in: the plane it belongs to, and its lattice position.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct CellKey {
    plane: (i32, i32, i32),
    x: i64,
    y: i64,
}

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

/// The overlap of the level's tiles: whether two tiles of a plane cover the same pixels.
///
/// WHY the plane is part of the cell: the tiles of different planes are different images of the
/// same scene, so two of them may sit at the same place without overlapping.
pub(super) fn overlap_of(
    tiles: &[StoredTile],
    cell: &[usize; 2],
    origin: &[i64; 2],
) -> Overlap {
    let mut cells: HashMap<CellKey, Vec<&StoredTile>> = HashMap::new();
    for tile in tiles {
        let key = CellKey {
            plane: tile.plane,
            x: (tile.x - origin[0]).div_euclid(cell[0] as i64),
            y: (tile.y - origin[1]).div_euclid(cell[1] as i64),
        };
        cells.entry(key).or_default().push(tile);
    }
    // The cells are walked in a fixed order, so a level that costs too much names the same cell every time.
    let mut cells: Vec<(CellKey, Vec<&StoredTile>)> = cells.into_iter().collect();
    cells.sort_by_key(|(key, _)| *key);

    let mut spent: u64 = 0;
    for (_, group) in cells {
        let cost = pair_count(group.len());
        if spent + cost > MAX_OVERLAP_COMPARISONS {
            return Overlap::TooManyTiles {
                tiles: group.len(),
            };
        }
        spent += cost;
        for (index, first) in group.iter().enumerate() {
            for second in &group[index + 1..] {
                if tiles_overlap(first, second) {
                    return Overlap::Overlapping;
                }
            }
        }
    }
    Overlap::Clear
}

/// The comparisons a cell of `tiles` tiles costs: every pair of them is compared.
fn pair_count(tiles: usize) -> u64 {
    let count = tiles as u64;
    count * count.saturating_sub(1) / 2
}

/// Whether two tiles of a plane cover the same pixels.
fn tiles_overlap(first: &StoredTile, second: &StoredTile) -> bool {
    first.x < second.x + second.source_w
        && second.x < first.x + first.source_w
        && first.y < second.y + second.source_h
        && second.y < first.y + first.source_h
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

#[cfg(test)]
mod tests;
