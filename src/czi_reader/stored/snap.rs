//! The read a rect names: the scene's part of it, on the level's lattice, in whole level pixels.

use crate::error::{Error, Result};
use crate::types::PixelRect;

use super::grid::div_ceil;

/// The read a rect names: the scene's part of it, on the level's lattice, in whole level pixels.
pub(super) struct Snap {
    /// The read's first level pixel's corner, in the scene's source pixels.
    pub x: i64,
    pub y: i64,
    /// The read's size, in level pixels.
    pub cols: usize,
    pub rows: usize,
    /// The scene's rect inside the read, counted from the read's own corner.
    pub scene: PixelRect,
}

impl Snap {
    /// The whole read, in source pixels counted from its own corner.
    pub(super) fn frame(&self, factor: usize) -> PixelRect {
        PixelRect::new(0, 0, self.cols * factor, self.rows * factor)
    }

    /// A rect of the read frame, back in the scene's source pixels.
    ///
    /// WHY the cast holds: the rect lies inside [`Snap::scene`], whose corner is at or after the
    /// scene's own.
    pub(super) fn to_scene(&self, rect: PixelRect) -> PixelRect {
        PixelRect::new(
            (rect.x as i64 + self.x) as usize,
            (rect.y as i64 + self.y) as usize,
            rect.w,
            rect.h,
        )
    }
}

/// The read a rect names, or the refusal of one the scene cannot serve.
///
/// WHY outward: a level pixel is whole or absent — a partial one would shift the level against
/// the caller's grid.
pub(super) fn snap_rect(
    origin: [i64; 2],
    factor: usize,
    rect: PixelRect,
    scene: (usize, usize),
) -> Result<Snap> {
    if rect.w == 0 || rect.h == 0 {
        return Err(Error::Decode(format!(
            "CZI stored level x{factor}: the rect {{x:{}, y:{}, w:{}, h:{}}} is empty",
            rect.x, rect.y, rect.w, rect.h
        )));
    }
    let (x_end, y_end) = match (rect.x.checked_add(rect.w), rect.y.checked_add(rect.h)) {
        (Some(x_end), Some(y_end)) => (x_end, y_end),
        _ => {
            return Err(Error::Decode(format!(
                "CZI stored level x{factor}: the rect {{x:{}, y:{}, w:{}, h:{}}} overflows",
                rect.x, rect.y, rect.w, rect.h
            )))
        }
    };
    let (x0, x1) = (rect.x.min(scene.0), x_end.min(scene.0));
    let (y0, y1) = (rect.y.min(scene.1), y_end.min(scene.1));
    if x0 >= x1 || y0 >= y1 {
        return Err(Error::Decode(format!(
            "CZI stored level x{factor}: the rect {{x:{}, y:{}, w:{}, h:{}}} does not meet the scene {}x{}",
            rect.x, rect.y, rect.w, rect.h, scene.0, scene.1
        )));
    }
    let step = factor as i64;
    let left = origin[0] + (x0 as i64 - origin[0]).div_euclid(step) * step;
    let top = origin[1] + (y0 as i64 - origin[1]).div_euclid(step) * step;
    let right = origin[0] + div_ceil(x1 as i64 - origin[0], step) * step;
    let bottom = origin[1] + div_ceil(y1 as i64 - origin[1], step) * step;
    // WHY the scene's own edges and not the rect's: a cell the rect cuts is still a whole cell
    // of the read, and its value must not depend on the window that asked for it.
    let (scene_x0, scene_x1) = ((-left).max(0), scene.0 as i64 - left);
    let (scene_y0, scene_y1) = ((-top).max(0), scene.1 as i64 - top);
    let (frame_w, frame_h) = (right - left, bottom - top);
    let (scene_x1, scene_y1) = (scene_x1.min(frame_w), scene_y1.min(frame_h));
    Ok(Snap {
        x: left,
        y: top,
        cols: (frame_w / step) as usize,
        rows: (frame_h / step) as usize,
        scene: PixelRect::new(
            scene_x0 as usize,
            scene_y0 as usize,
            (scene_x1 - scene_x0) as usize,
            (scene_y1 - scene_y0) as usize,
        ),
    })
}
