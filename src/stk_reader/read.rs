// SPDX-License-Identifier: Apache-2.0

use crate::error::{Error, Result};
use crate::types::{Dims5, Plane, PlaneBuf};

use super::StkReader;

impl StkReader {
    pub(super) fn read_plane(&self, scene: usize, t: usize, c: usize, z: usize) -> Result<Plane> {
        if scene >= 1 {
            return Err(Error::SceneOutOfRange { scene, count: 1 });
        }
        let dims = self.layout.dims;
        if t >= dims.t || c >= dims.c || z >= dims.z {
            return Err(Error::OutOfRange { scene, t, c, z });
        }
        let plane_dims = Dims5 {
            t: 1,
            c: 1,
            z: 1,
            y: dims.y,
            x: dims.x,
        };
        let plane_bytes = plane_dims.checked_plane_bytes(self.dtype)?;
        let offset = self.layout.plane_offset(z)?;
        let end = offset
            .checked_add(plane_bytes)
            .ok_or_else(|| Error::Decode(ADDRESS_OVERFLOW.to_string()))?;
        let block = self.mmap.get(offset..end).ok_or_else(|| {
            Error::Decode(format!(
                "MetaMorph STK plane {z} spans bytes {offset}..{end}, past the end of the {}-byte file",
                self.mmap.len()
            ))
        })?;
        let data = PlaneBuf::from_bytes(self.dtype, self.endian, block)?;
        Plane::new(data, [dims.y, dims.x])
    }
}

const ADDRESS_OVERFLOW: &str = "MetaMorph STK: a plane end overflows the address space";
