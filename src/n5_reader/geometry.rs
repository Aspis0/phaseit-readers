// SPDX-License-Identifier: Apache-2.0
//! The geometry of one N5 scene: its `Dims5`, the block grid a plane is tiled by, and the assembly of one plane.
//! Dimension 0 of the file is the fastest axis; v1 reads (x, y) as one plane and (x, y, z) as one volume.
//! Each block's big-endian samples are copied into the plane row by row; the plane is converted once, at the end.

use crate::error::{Error, Result};
use crate::types::{DType, Dims5, Endian, Plane, PlaneBuf};

use super::attributes::Attributes;
use super::block::Block;

#[derive(Debug)]
pub(super) struct Layout {
    /// The dataset's block size, dimension 0 fastest; a block on disk may be smaller along an edge, never larger.
    block: Vec<usize>,
    /// Blocks along x and y of every plane.
    grid: [usize; 2],
    /// The block row of each plane: `z / block side along z`.
    rows: Vec<usize>,
    dims: Dims5,
    dtype: DType,
}

pub(super) fn layout(attributes: &Attributes) -> Result<Layout> {
    let dims = scene(&attributes.dimensions)?;
    let side = |axis: usize| extent(&attributes.block_size, axis, dims);
    Ok(Layout {
        grid: [dims.x.div_ceil(side(0)?), dims.y.div_ceil(side(1)?)],
        rows: (0..dims.z)
            .map(|z| Ok(z / side(2)?))
            .collect::<Result<Vec<usize>>>()?,
        block: attributes.block_size.clone(),
        dims,
        dtype: attributes.dtype,
    })
}

/// The one scene: 2 dimensions are (x, y), 3 dimensions are (x, y, z); v1 maps no other rank.
fn scene(dimensions: &[usize]) -> Result<Dims5> {
    let (y, x, z) = match dimensions {
        [x, y] => (*y, *x, 1),
        [x, y, z] => (*y, *x, *z),
        _ => {
            return Err(Error::Unsupported(format!(
                "N5 v1 reads a dataset of 2 dimensions (x, y) or 3 dimensions (x, y, z); this one has {}: channels \\
                 and time points are not mapped yet",
                dimensions.len()
            )))
        }
    };
    Ok(Dims5 {
        t: 1,
        c: 1,
        z,
        y,
        x,
    })
}

/// A block side along one axis; a 2-D dataset states none for axis 2, where a block covers the whole extent.
fn extent(block: &[usize], axis: usize, dims: Dims5) -> Result<usize> {
    if axis == 2 && block.len() < 3 {
        return Ok(dims.z);
    }
    block
        .get(axis)
        .copied()
        .filter(|side| *side > 0)
        .ok_or_else(|| {
            Error::Parse(format!(
                "N5 block size has no positive side along axis {axis}"
            ))
        })
}

impl Layout {
    /// The dataset's extent per axis, dimension 0 fastest: what every block header is checked against.
    pub fn edge(&self) -> Vec<usize> {
        match self.block.len() < 3 {
            true => vec![self.dims.x, self.dims.y],
            false => vec![self.dims.x, self.dims.y, self.dims.z],
        }
    }

    /// The dataset's block size, dimension 0 fastest.
    pub fn block(&self) -> &[usize] {
        &self.block
    }

    /// The one scene's extent; the decode ceiling applies where a plane of it is materialised.
    pub fn dims(&self) -> Dims5 {
        self.dims
    }

    /// The dataset's sample type.
    pub fn dtype(&self) -> DType {
        self.dtype
    }

    /// One plane assembled block by block: each held block is decoded once and its `(y, x)` samples of this plane
    /// are copied where they belong. A block the dataset does not hold stays zero (N5's missing-block fill value).
    pub fn plane(
        &self,
        z: usize,
        read: impl Fn(&[usize]) -> Result<Option<Block>>,
    ) -> Result<Plane> {
        let plane = Dims5 { z: 1, ..self.dims };
        // The ceiling before the first byte is reserved, so an over-ceiling plane never allocates.
        let mut out = vec![0u8; plane.checked_plane_bytes(self.dtype)?];
        let row = self.rows[z];
        let mut at = vec![0usize; self.block.len()];
        for g_y in 0..self.grid[1] {
            for g_x in 0..self.grid[0] {
                at[0] = g_x;
                at[1] = g_y;
                if at.len() > 2 {
                    at[2] = row;
                }
                let Some(block) = read(&at)? else {
                    continue;
                };
                let shape = block.shape();
                if shape[0] > self.block[0] || shape[1] > self.block[1] {
                    return Err(Error::Decode(format!(
                        "N5 block {}x{} (x x y) is larger than the dataset's block size {}x{}",
                        shape[0], shape[1], self.block[0], self.block[1]
                    )));
                }
                // A block row of several planes holds one plane per z: the one this plane reads is its own.
                let within = z - row * self.block.get(2).copied().unwrap_or(1);
                copy(
                    &block,
                    (g_x * self.block[0], g_y * self.block[1]),
                    within,
                    plane,
                    &mut out,
                )?;
            }
        }
        Plane::new(
            PlaneBuf::from_bytes(self.dtype, Endian::Big, &out)?,
            [plane.y, plane.x],
        )
    }
}

/// A block's bytes, one row at a time, into the plane at row `top`, column `left`; `z` is the plane's own index
/// inside the block (0 for a 2-D dataset, whose blocks hold one plane).
fn copy(
    block: &Block,
    (left, top): (usize, usize),
    z: usize,
    plane: Dims5,
    out: &mut [u8],
) -> Result<()> {
    let (sample, row) = (block.sample_bytes(), block.row_bytes());
    // The flat-buffer check below cannot see an overrun that stays inside the plane: a row whose last samples
    // land on the next row is silent corruption, so the placement itself is checked against the edge.
    if left + block.shape()[0] > plane.x || top + block.shape()[1] > plane.y {
        return Err(Error::Decode(format!(
            "N5 block {}x{} (x x y) at ({left}, {top}) runs past the {}x{} plane's edge",
            block.shape()[0],
            block.shape()[1],
            plane.x,
            plane.y
        )));
    }
    for y in 0..block.shape()[1] {
        let from = block.row(z, y)?;
        let at = (top + y) * plane.x * sample + left * sample;
        let to = out.get_mut(at..at + row).ok_or_else(|| {
            Error::Decode(format!(
                "N5 block row {y} ends past the {}x{} plane",
                plane.y, plane.x
            ))
        })?;
        to.copy_from_slice(from);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calibration::Calibration;
    use crate::n5_reader::attributes::Compression;
    use crate::n5_reader::block::{self, BlockHeader};

    fn attributes(dimensions: Vec<usize>, block: Vec<usize>) -> Attributes {
        Attributes {
            dimensions,
            block_size: block,
            dtype: DType::U16,
            compression: Compression::None,
            calibration: Calibration::default(),
        }
    }

    /// A block of `shape` `[y, x]` whose every sample is `value`.
    fn block(shape: [usize; 2], value: u16) -> Block {
        let header = BlockHeader {
            size: vec![shape[1], shape[0]],
            len: 12,
        };
        let mut bytes = Vec::new();
        for _ in 0..shape[0] * shape[1] {
            bytes.extend(value.to_be_bytes());
        }
        block::decode(&header, &bytes, Compression::None, DType::U16).unwrap()
    }

    #[test]
    fn the_grid_covers_every_block_of_a_plane() {
        let built = layout(&attributes(vec![7, 5, 3], vec![3, 2, 2])).unwrap();
        assert_eq!(built.grid, [3, 3]);
        assert_eq!(built.rows, vec![0, 0, 1]);
        assert_eq!(built.edge(), vec![7, 5, 3]);
        assert_eq!(built.block(), &[3, 2, 2]);
        assert_eq!(built.dtype(), DType::U16);
        let flat = layout(&attributes(vec![7, 5], vec![3, 2])).unwrap();
        assert_eq!(flat.grid, [3, 3]);
        assert_eq!(flat.rows, vec![0]);
        assert_eq!(flat.edge(), vec![7, 5]);
        let dims = flat.dims();
        assert_eq!((dims.z, dims.y, dims.x), (1, 5, 7));
        assert_eq!(
            flat.plane(0, |_| Ok(None)).unwrap().data,
            PlaneBuf::U16(vec![0; 35])
        );
    }

    #[test]
    fn another_rank_than_two_or_three_is_refused() {
        for (dimensions, block) in [
            (&[][..], &vec![2][..]),
            (&[4][..], &vec![2][..]),
            (&[4, 3, 2, 1][..], &vec![2, 2, 2, 2][..]),
            (&[7, 5][..], &vec![0, 2][..]),
        ] {
            let Err(error) = layout(&attributes(dimensions.to_vec(), block.to_vec())) else {
                panic!("a dataset of rank {dimensions:?} opens")
            };
            assert!(
                error.to_string().contains("dimensions") || error.to_string().contains("axis"),
                "{error}"
            );
        }
    }

    #[test]
    fn a_block_lands_at_its_grid_place_and_a_missing_one_stays_zero() {
        // x = 4, y = 3, block (3, 2): block (0,0) covers x 0..3, y 0..2 and (1,1) covers only (3, 2).
        let layout = layout(&attributes(vec![4, 3], vec![3, 2])).unwrap();
        let plane = layout
            .plane(0, |at| {
                Ok(match at {
                    [0, 0, ..] => Some(block([2, 3], 1)),
                    [1, 1, ..] => Some(block([1, 1], 2)),
                    _ => None,
                })
            })
            .unwrap();
        let mut expected = vec![0u16; 12];
        for y in 0..2 {
            for x in 0..3 {
                expected[y * 4 + x] = 1;
            }
        }
        expected[2 * 4 + 3] = 2;
        assert_eq!(plane.shape, [3, 4]);
        assert_eq!(plane.data, PlaneBuf::U16(expected));
    }

    /// A block of `shape` `[y, x]` per plane `z` of the block, every sample `value(z)`.
    fn volume(shape: [usize; 2], planes: usize, value: impl Fn(usize) -> u16) -> Block {
        let header = BlockHeader {
            size: vec![shape[1], shape[0], planes],
            len: 16,
        };
        let mut bytes = Vec::new();
        for z in 0..planes {
            for _ in 0..shape[0] * shape[1] {
                bytes.extend(value(z).to_be_bytes());
            }
        }
        block::decode(&header, &bytes, Compression::None, DType::U16).unwrap()
    }

    #[test]
    fn the_plane_of_a_block_row_is_the_one_the_plane_asks_for() {
        // Block row 1 covers z = 2 and 3 (block side 2 along z): plane 2 must not read plane 3's samples.
        let layout = layout(&attributes(vec![2, 2, 4], vec![2, 2, 2])).unwrap();
        for z in 0..4 {
            let plane = layout
                .plane(z, |at| {
                    Ok(Some(volume([2, 2], 2, |local| {
                        (at[2] * 2 + local + 1) as u16
                    })))
                })
                .unwrap();
            assert_eq!(plane.data, PlaneBuf::U16(vec![z as u16 + 1; 4]), "z = {z}");
        }
    }

    #[test]
    fn a_block_larger_than_the_block_size_is_refused() {
        let layout = layout(&attributes(vec![4, 3], vec![3, 2])).unwrap();
        for shape in [[3, 3], [1, 4]] {
            let error = layout
                .plane(0, |_| Ok(Some(block(shape, 0))))
                .expect_err("too large");
            assert!(error.to_string().contains("block size"), "{error}");
        }
    }

    #[test]
    fn a_block_past_the_edge_of_the_plane_is_refused() {
        let layout = layout(&attributes(vec![4, 3], vec![3, 2])).unwrap();
        let plane = Dims5 {
            z: 1,
            ..layout.dims()
        };
        let out = &mut [0u8; 3 * 4 * 2];
        let mut placed = |top, left| copy(&block([2, 3], 0), (left, top), 0, plane, out);
        assert!(placed(2, 3)
            .expect_err("past the edge")
            .to_string()
            .contains("past"));
        assert!(placed(2, 0)
            .expect_err("past the last row")
            .to_string()
            .contains("past"));
        placed(1, 1).expect("the last full block fits");
    }

    #[test]
    fn an_over_ceiling_plane_is_refused_before_it_is_assembled() {
        let layout = layout(&attributes(vec![70_000, 70_000], vec![1, 1])).unwrap();
        assert!(matches!(
            layout.plane(0, |_| Ok(None)),
            Err(Error::OversizedAllocation { .. })
        ));
    }

    #[test]
    fn a_full_block_at_the_edge_is_refused_not_wrapped_into_the_next_row() {
        // Grid (2, 0) of a 7-wide plane holds only x 6..7, so a block stating its full extent would write its
        // last samples into the next row: refused along one axis as along the other.
        let layout = layout(&attributes(vec![7, 5], vec![3, 2])).unwrap();
        let reads = |at: [usize; 2]| {
            layout.plane(0, |g| {
                Ok((g[0] == at[0] && g[1] == at[1]).then(|| block([2, 3], 238)))
            })
        };
        for at in [[2, 0], [0, 2]] {
            let error = reads(at).err().expect("a block past the plane's edge");
            assert!(error.to_string().contains("edge"), "{at:?}: {error}");
        }
    }
}
