//! The window of one tile of a stored level inside a read, and the copy of its pixels.

use crate::error::{Error, Result};

use super::grid::div_round;
use super::levels::StoredTile;

/// The part of one tile that falls inside a read: where it lands in the read, and which of its
/// own pixels it supplies.
pub(super) struct Window {
    out_y: usize,
    out_x: usize,
    src_y: usize,
    src_x: usize,
    rows: usize,
    cols: usize,
}

/// The window of a tile inside a read of `width` x `height` level pixels that starts at the
/// source pixels `(x0, y0)`, or `None` when the tile lies outside the read.
///
/// WHY the shift: a level's pixels sit on the grid of its own origin, which is not the scene's,
/// so a tile's first pixel lands a fraction of a pixel away from it. The shift rounds that away
/// once, the same way for every tile of the level.
pub(super) fn tile_window(
    tile: &StoredTile,
    x0: i64,
    y0: i64,
    factor: usize,
    width: usize,
    height: usize,
) -> Option<Window> {
    let shift_x = div_round(tile.x - x0, factor as i64);
    let shift_y = div_round(tile.y - y0, factor as i64);
    let dx0 = shift_x.max(0);
    let dx1 = (shift_x + tile.w as i64).min(width as i64);
    let dy0 = shift_y.max(0);
    let dy1 = (shift_y + tile.h as i64).min(height as i64);
    if dx0 >= dx1 || dy0 >= dy1 {
        return None;
    }
    Some(Window {
        out_y: dy0 as usize,
        out_x: dx0 as usize,
        src_y: (dy0 - shift_y) as usize,
        src_x: (dx0 - shift_x) as usize,
        rows: (dy1 - dy0) as usize,
        cols: (dx1 - dx0) as usize,
    })
}

/// Copies the window of one decoded tile into the composed plane.
pub(super) fn blit_tile(
    composed: &mut [u8],
    tile: &StoredTile,
    decoded: &[u8],
    window: &Window,
    width: usize,
    pixel_bytes: usize,
) -> Result<()> {
    let tile_stride = tile.w * pixel_bytes;
    let out_stride = width * pixel_bytes;
    let expected = tile_stride * tile.h;
    if decoded.len() != expected {
        return Err(Error::Decode(format!(
            "CZI stored tile decoded to {} bytes, expected {expected}",
            decoded.len()
        )));
    }
    let bytes = window.cols * pixel_bytes;
    for row in 0..window.rows {
        let src = (window.src_y + row) * tile_stride + window.src_x * pixel_bytes;
        let dst = (window.out_y + row) * out_stride + window.out_x * pixel_bytes;
        if src + bytes > decoded.len() || dst + bytes > composed.len() {
            return Err(Error::Decode(
                "CZI stored tile blit runs past its buffer".to_string(),
            ));
        }
        composed[dst..dst + bytes].copy_from_slice(&decoded[src..src + bytes]);
    }
    Ok(())
}
