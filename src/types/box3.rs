// SPDX-License-Identifier: Apache-2.0
use crate::error::{Error, Result};

use super::dims::Dims5;

/// A 3-D box of one channel at one time point: `(x, y, z)` is the origin in pixels/planes, `w`/`h`/`d` its size.
/// `x`/`w` address the width axis, `y`/`h` the height axis and `z`/`d` the plane axis, as in [`Dims5`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Box3 {
    pub x: usize,
    pub y: usize,
    pub z: usize,
    pub w: usize,
    pub h: usize,
    pub d: usize,
}

impl Box3 {
    pub fn new(x: usize, y: usize, z: usize, w: usize, h: usize, d: usize) -> Box3 {
        Box3 { x, y, z, w, h, d }
    }

    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0 || self.d == 0
    }
}

/// Validates a box's coordinate, then its box, in the order every reader uses.
/// One helper, because a call wrong in both ways must report the same error everywhere.
pub(crate) fn checked_box3(
    label: &str,
    dims: &Dims5,
    scene: usize,
    t: usize,
    c: usize,
    bx: Box3,
) -> Result<()> {
    if t >= dims.t || c >= dims.c || bx.z >= dims.z {
        return Err(Error::OutOfRange {
            scene,
            t,
            c,
            z: bx.z,
        });
    }
    if bx.is_empty() {
        return Err(Error::Decode(format!(
            "{label} box {{x:{}, y:{}, z:{}, w:{}, h:{}, d:{}}} is empty",
            bx.x, bx.y, bx.z, bx.w, bx.h, bx.d
        )));
    }

    let x_end = end(label, "x", bx.x, bx.w)?;
    let y_end = end(label, "y", bx.y, bx.h)?;
    let z_end = end(label, "z", bx.z, bx.d)?;
    if x_end > dims.x || y_end > dims.y || z_end > dims.z {
        return Err(Error::Decode(format!(
            "{label} box {{x:{}, y:{}, z:{}, w:{}, h:{}, d:{}}} out of range for scene {} dims (x:{}, y:{}, z:{})",
            bx.x, bx.y, bx.z, bx.w, bx.h, bx.d, scene, dims.x, dims.y, dims.z
        )));
    }

    Ok(())
}

fn end(label: &str, axis: &str, start: usize, len: usize) -> Result<usize> {
    start.checked_add(len).ok_or_else(|| {
        Error::Decode(format!(
            "{label} box {axis}+size overflows usize (start={start}, len={len})"
        ))
    })
}

/// The bricks of a scene's x/y/z, clipped at the far edge so every voxel is covered exactly once.
/// A zero `brick` side or a zero `dims` side yields no brick: a zero step would never reach the far edge.
pub fn bricks(dims: Dims5, brick: (u32, u32, u32)) -> impl Iterator<Item = Box3> {
    let (bw, bh, bd) = (brick.0 as usize, brick.1 as usize, brick.2 as usize);
    let count = |len: usize, step: usize| {
        if step == 0 {
            0
        } else {
            len.div_ceil(step)
        }
    };
    let (nx, ny, nz) = (count(dims.x, bw), count(dims.y, bh), count(dims.z, bd));

    (0..nz).flat_map(move |zi| {
        (0..ny).flat_map(move |yi| {
            (0..nx).map(move |xi| {
                Box3::new(
                    xi * bw,
                    yi * bh,
                    zi * bd,
                    bw.min(dims.x - xi * bw),
                    bh.min(dims.y - yi * bh),
                    bd.min(dims.z - zi * bd),
                )
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    fn dims(x: usize, y: usize, z: usize) -> Dims5 {
        Dims5 {
            t: 1,
            c: 1,
            z,
            y,
            x,
        }
    }

    #[test]
    fn a_box_past_the_scene_is_refused_on_every_axis() {
        let d = dims(8, 6, 4);
        for bx in [
            Box3::new(4, 0, 0, 5, 1, 1),
            Box3::new(0, 3, 0, 1, 4, 1),
            Box3::new(0, 0, 2, 1, 1, 3),
        ] {
            assert!(
                matches!(
                    checked_box3("volume", &d, 0, 0, 0, bx),
                    Err(Error::Decode(_))
                ),
                "a box past the scene is refused, got {:?}",
                checked_box3("volume", &d, 0, 0, 0, bx)
            );
        }
    }

    #[test]
    fn a_wrong_coordinate_is_out_of_range_before_the_box_is_measured() {
        let d = dims(8, 6, 4);
        assert!(matches!(
            checked_box3("volume", &d, 0, 0, 0, Box3::new(0, 0, 9, 1, 1, 1)),
            Err(Error::OutOfRange { .. })
        ));
        assert!(matches!(
            checked_box3("volume", &d, 0, 0, 1, Box3::new(0, 0, 99, 0, 0, 0)),
            Err(Error::OutOfRange { .. })
        ));
    }

    #[test]
    fn an_empty_box_is_refused() {
        let d = dims(8, 6, 4);
        for bx in [
            Box3::new(0, 0, 0, 0, 1, 1),
            Box3::new(0, 0, 0, 1, 0, 1),
            Box3::new(0, 0, 0, 1, 1, 0),
        ] {
            assert!(
                matches!(
                    checked_box3("volume", &d, 0, 0, 0, bx),
                    Err(Error::Decode(_))
                ),
                "an empty box is refused, got {:?}",
                checked_box3("volume", &d, 0, 0, 0, bx)
            );
        }
    }

    #[test]
    fn an_overflowing_far_edge_is_refused() {
        let d = dims(8, 6, 4);
        assert!(matches!(
            checked_box3("volume", &d, 0, 0, 0, Box3::new(usize::MAX, 0, 0, 2, 1, 1)),
            Err(Error::Decode(_))
        ));
        assert!(matches!(
            checked_box3("volume", &d, 0, 0, 0, Box3::new(0, 0, 0, 1, 1, usize::MAX)),
            Err(Error::Decode(_))
        ));
    }

    #[test]
    fn bricks_cover_a_scene_that_is_not_a_multiple_of_the_brick() {
        let d = dims(130, 64, 7);
        let boxes: Vec<Box3> = bricks(d, (64, 64, 4)).collect();
        // 3 bricks on x, 1 on y, 2 on z.
        assert_eq!(boxes.len(), 6);

        let mut seen = vec![0u32; 130 * 64 * 7];
        for bx in &boxes {
            for z in bx.z..bx.z + bx.d {
                for y in bx.y..bx.y + bx.h {
                    for x in bx.x..bx.x + bx.w {
                        seen[(z * 64 + y) * 130 + x] += 1;
                    }
                }
            }
        }
        assert!(
            seen.iter().all(|count| *count == 1),
            "every voxel is covered exactly once"
        );
    }

    #[test]
    fn the_last_brick_on_an_axis_is_clipped() {
        let d = dims(130, 64, 7);
        let last = bricks(d, (64, 64, 4)).last().expect("a last brick");
        assert_eq!(last, Box3::new(128, 0, 4, 2, 64, 3));
    }

    #[test]
    fn empty_dims_and_an_empty_brick_give_no_brick() {
        for d in [dims(0, 64, 7), dims(130, 0, 7), dims(130, 64, 0)] {
            assert_eq!(bricks(d, (64, 64, 4)).count(), 0, "no brick of {d:?}");
        }
        let d = dims(130, 64, 7);
        assert_eq!(bricks(d, (0, 64, 4)).count(), 0);
        assert_eq!(bricks(d, (64, 64, 0)).count(), 0);
    }
}
