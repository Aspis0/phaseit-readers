// SPDX-License-Identifier: Apache-2.0
//! Reading one N5 plane: the blocks that intersect it are read one at a time, each decoded before the next, and a
//! block the dataset does not hold is N5's zero block.

use crate::error::{Error, Result};
use crate::types::Plane;

use super::block::{self, Block};
use super::N5Reader;

impl N5Reader {
    pub(super) fn read_plane(&self, scene: usize, t: usize, c: usize, z: usize) -> Result<Plane> {
        let dims = self.layout.dims();
        if scene >= 1 {
            return Err(Error::SceneOutOfRange { scene, count: 1 });
        }
        if t >= dims.t || c >= dims.c || z >= dims.z {
            return Err(Error::OutOfRange { scene, t, c, z });
        }
        let read = |at: &[usize]| self.read_block(at);
        self.layout.plane(z, read)
    }

    /// The block at grid position `at` (its third component is the block row of the plane), or `None` when the
    /// dataset holds no file there (N5's zero block).
    fn read_block(&self, at: &[usize]) -> Result<Option<Block>> {
        let Some(path) = self.blocks.file(at) else {
            return Ok(None);
        };
        block::read(
            &path,
            self.layout.block(),
            &self.layout.edge(),
            self.layout.dtype(),
            self.compression,
        )
        .map(Some)
    }
}
