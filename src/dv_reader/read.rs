// SPDX-License-Identifier: Apache-2.0

use crate::error::{Error, Result};
use crate::types::{Dims5, Plane, PlaneBuf};

use super::layout::SectionOrder;
use super::DvReader;

impl DvReader {
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
        let offset = self.section_offset(t, c, z)?;
        let end = offset
            .checked_add(plane_bytes)
            .ok_or_else(|| Error::Decode("DeltaVision section end overflows usize".to_string()))?;
        let block = self.mmap.get(offset..end).ok_or_else(|| {
            Error::Decode(format!(
                "DeltaVision section (t={t}, c={c}, z={z}) spans bytes {offset}..{end}, past the end of the {}-byte file",
                self.mmap.len()
            ))
        })?;
        let data = PlaneBuf::from_bytes(self.dtype, self.endian, block)?;
        Plane::new(data, [dims.y, dims.x])
    }

    fn section_index(&self, t: usize, c: usize, z: usize) -> Result<usize> {
        let dims = self.layout.dims;
        let idx = match self.layout.section_order {
            SectionOrder::Ztw => (c * dims.t + t) * dims.z + z,
            SectionOrder::Wzt => (t * dims.z + z) * dims.c + c,
            SectionOrder::Zwt => (t * dims.c + c) * dims.z + z,
        };
        if idx >= dims.t * dims.c * dims.z {
            return Err(Error::Decode(
                "DeltaVision section index out of range".to_string(),
            ));
        }
        Ok(idx)
    }

    fn section_offset(&self, t: usize, c: usize, z: usize) -> Result<usize> {
        let index = self.section_index(t, c, z)?;
        index
            .checked_mul(self.layout.section_bytes)
            .and_then(|at| at.checked_add(self.layout.data_offset))
            .ok_or_else(|| Error::Decode("DeltaVision section offset overflows usize".to_string()))
    }
}
