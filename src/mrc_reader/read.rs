// SPDX-License-Identifier: Apache-2.0
//! One plane of an MRC file: a contiguous section, converted to the host's byte order.

use crate::error::{Error, Result};
use crate::types::{Dims5, Plane, PlaneBuf};

use super::MrcReader;

impl MrcReader {
    pub(super) fn read_plane(&self, scene: usize, t: usize, c: usize, z: usize) -> Result<Plane> {
        if scene >= 1 {
            return Err(Error::SceneOutOfRange { scene, count: 1 });
        }
        let dims = self.layout.dims;
        if t >= dims.t || c >= dims.c || z >= dims.z {
            return Err(Error::OutOfRange { scene, t, c, z });
        }
        // The output is sized (and refused by the decode ceiling) before a byte is copied.
        let plane_dims = Dims5 {
            t: 1,
            c: 1,
            z: 1,
            y: dims.y,
            x: dims.x,
        };
        let plane_bytes = plane_dims.checked_plane_bytes(self.dtype)?;
        let offset = self.section_offset(t, z)?;
        let end = offset
            .checked_add(plane_bytes)
            .ok_or_else(|| Error::Decode("MRC section end overflows usize".to_string()))?;
        let block = self.mmap.get(offset..end).ok_or_else(|| {
            Error::Decode(format!(
                "MRC section (t={t}, z={z}) spans bytes {offset}..{end}, past the end of the {}-byte file",
                self.mmap.len()
            ))
        })?;
        let data = PlaneBuf::from_bytes(self.dtype, self.endian, block)?;
        Plane::new(data, [dims.y, dims.x])
    }

    /// Sections are stored in order, `z` fastest then `t`, each `section_bytes` long.
    fn section_offset(&self, t: usize, z: usize) -> Result<usize> {
        let index = t
            .checked_mul(self.layout.dims.z)
            .and_then(|planes| planes.checked_add(z))
            .ok_or_else(|| Error::Decode("MRC section index overflows usize".to_string()))?;
        index
            .checked_mul(self.layout.section_bytes)
            .and_then(|at| at.checked_add(self.layout.data_offset))
            .ok_or_else(|| Error::Decode("MRC section offset overflows usize".to_string()))
    }
}
