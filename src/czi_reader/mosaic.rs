use std::collections::BTreeSet;

use crate::error::{Error, Result};

use super::directory::Coord;
use super::subblock::SubBlock;

/// Blits `tile` into the composed plane at `(col_off, row_off)`. `pixel_bytes` is one whole pixel,
/// so the row stride and the column offsets are samples-aware for multi-sample tiles.
#[allow(clippy::too_many_arguments)]
pub(super) fn blit_tile(
    composed: &mut [u8],
    scene_x: usize,
    scene_y: usize,
    row_stride: usize,
    pixel_bytes: usize,
    tile: &[u8],
    stored_x: usize,
    stored_y: usize,
    col_off_i: i64,
    row_off_i: i64,
) -> Result<()> {
    // Validated defensively: a stray coordinate must never write out of bounds.
    if col_off_i < 0 || row_off_i < 0 {
        return Err(Error::Decode("CZI tile placed before scene origin".to_string()));
    }
    let col_off = usize::try_from(col_off_i)
        .map_err(|_| Error::Decode("CZI tile column offset out of range".to_string()))?;
    let row_off = usize::try_from(row_off_i)
        .map_err(|_| Error::Decode("CZI tile row offset out of range".to_string()))?;
    let right = col_off.checked_add(stored_x);
    let bottom = row_off.checked_add(stored_y);
    match (right, bottom) {
        (Some(r), Some(b)) if r <= scene_x && b <= scene_y => {}
        _ => {
            return Err(Error::Decode(
                "CZI tile extends past the composed scene bounds".to_string(),
            ))
        }
    }

    let tile_row_bytes = stored_x
        .checked_mul(pixel_bytes)
        .ok_or_else(|| Error::Decode("CZI tile row stride overflow".to_string()))?;
    if tile.len() != tile_row_bytes.saturating_mul(stored_y) {
        return Err(Error::Decode(format!(
            "CZI tile has {} bytes, expected {}",
            tile.len(),
            tile_row_bytes.saturating_mul(stored_y)
        )));
    }
    let col_off_bytes = col_off
        .checked_mul(pixel_bytes)
        .ok_or_else(|| Error::Decode("CZI tile column offset overflow".to_string()))?;

    for ty in 0..stored_y {
        let dst_start = (row_off + ty)
            .checked_mul(row_stride)
            .and_then(|v| v.checked_add(col_off_bytes))
            .ok_or_else(|| Error::Decode("CZI blit destination overflow".to_string()))?;
        let src_start = ty * tile_row_bytes;
        composed[dst_start..dst_start + tile_row_bytes]
            .copy_from_slice(&tile[src_start..src_start + tile_row_bytes]);
    }
    Ok(())
}

/// Blits only the `inter_w x inter_h` overlap of a decoded `tile` into a rect-sized destination (`region`).
/// Every offset and stride is checked against the slice lengths, so a crafted tile or rect fails with `Decode`.
#[allow(clippy::too_many_arguments)]
pub(super) fn blit_tile_clipped(
    composed: &mut [u8],
    dst_row_stride: usize,
    pixel_bytes: usize,
    tile: &[u8],
    stored_x: usize,
    stored_y: usize,
    src_row_off: usize,
    src_col_off: usize,
    dst_row_off: usize,
    dst_col_off: usize,
    inter_w: usize,
    inter_h: usize,
) -> Result<()> {
    // Same size invariant as blit_tile: a short or long tile is rejected before any copy indexes it.
    let src_row_stride = stored_x
        .checked_mul(pixel_bytes)
        .ok_or_else(|| Error::Decode("CZI tile row stride overflow".to_string()))?;
    let expected_tile = src_row_stride
        .checked_mul(stored_y)
        .ok_or_else(|| Error::Decode("CZI tile size overflow".to_string()))?;
    if tile.len() != expected_tile {
        return Err(Error::Decode(format!(
            "CZI tile has {} bytes, expected {expected_tile}",
            tile.len()
        )));
    }

    let copy_bytes = inter_w
        .checked_mul(pixel_bytes)
        .ok_or_else(|| Error::Decode("CZI region copy width overflow".to_string()))?;
    let src_col_bytes = src_col_off
        .checked_mul(pixel_bytes)
        .ok_or_else(|| Error::Decode("CZI region source column overflow".to_string()))?;
    let dst_col_bytes = dst_col_off
        .checked_mul(pixel_bytes)
        .ok_or_else(|| Error::Decode("CZI region dest column overflow".to_string()))?;

    for row in 0..inter_h {
        let src_start = (src_row_off + row)
            .checked_mul(src_row_stride)
            .and_then(|v| v.checked_add(src_col_bytes))
            .ok_or_else(|| Error::Decode("CZI region source offset overflow".to_string()))?;
        let src_end = src_start
            .checked_add(copy_bytes)
            .ok_or_else(|| Error::Decode("CZI region source span overflow".to_string()))?;
        let dst_start = (dst_row_off + row)
            .checked_mul(dst_row_stride)
            .and_then(|v| v.checked_add(dst_col_bytes))
            .ok_or_else(|| Error::Decode("CZI region dest offset overflow".to_string()))?;
        let dst_end = dst_start
            .checked_add(copy_bytes)
            .ok_or_else(|| Error::Decode("CZI region dest span overflow".to_string()))?;
        // Defensive: the intersection math keeps these in range, but no slice index may panic.
        if src_end > tile.len() || dst_end > composed.len() {
            return Err(Error::Decode(
                "CZI region blit span out of bounds".to_string(),
            ));
        }
        composed[dst_start..dst_end].copy_from_slice(&tile[src_start..src_end]);
    }
    Ok(())
}

pub(super) fn scene_bbox(members: &[&Coord], subblocks: &[SubBlock]) -> Result<(i32, i32, usize, usize)> {
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i64::MIN;
    let mut max_y = i64::MIN;
    for co in members {
        let sb = &subblocks[co.subblock];
        min_x = min_x.min(sb.start_x);
        min_y = min_y.min(sb.start_y);
        max_x = max_x.max(sb.start_x as i64 + sb.stored_x as i64);
        max_y = max_y.max(sb.start_y as i64 + sb.stored_y as i64);
    }
    // The extent is not bounded by MAX_SCENE_DIM here: a whole-slide or crafted scene must still open for region().
    // The dimension bound belongs to consumers that scale with the extent, not to open(), which allocates nothing
    // plane-sized.
    let width = (max_x - min_x as i64)
        .try_into()
        .map_err(|_| Error::Decode("CZI scene width is invalid".to_string()))?;
    let height = (max_y - min_y as i64)
        .try_into()
        .map_err(|_| Error::Decode("CZI scene height is invalid".to_string()))?;
    Ok((min_x, min_y, width, height))
}

pub(super) fn sorted_distinct<I: Iterator<Item = i32>>(iter: I) -> Vec<i32> {
    iter.collect::<BTreeSet<_>>().into_iter().collect()
}

#[cfg(test)]
mod tests;
