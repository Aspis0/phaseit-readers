// SPDX-License-Identifier: Apache-2.0
//! Reading one ICS plane: a sample sits at `Σ index · stride`, so a plane whose x and y are the two fastest axes is
//! one contiguous block, and every other axis order is gathered sample by sample.
//! A deflated plane is inflated from the start of the stream: a gzip stream has no index, so a plane read costs the
//! bytes before it.

use std::io::Read;

use flate2::read::GzDecoder;

use crate::error::{Error, Result};
use crate::types::{Plane, PlaneBuf};

use super::layout::Compression;
use super::IcsReader;

impl IcsReader {
    pub(super) fn read_plane(&self, scene: usize, t: usize, c: usize, z: usize) -> Result<Plane> {
        if scene >= 1 {
            return Err(Error::SceneOutOfRange { scene, count: 1 });
        }
        let dims = self.layout.dims;
        if t >= dims.t || c >= dims.c || z >= dims.z {
            return Err(Error::OutOfRange { scene, t, c, z });
        }
        let samples = self.plane_samples(t, c, z)?;
        let raw = match self.layout.compression() {
            Compression::None => self.mapped(samples)?,
            Compression::Gzip => self.inflated(samples)?,
        };
        let data = PlaneBuf::from_bytes(self.layout.dtype, self.layout.endian, &raw)?;
        Plane::new(data, [dims.y, dims.x])
    }

    /// The plane as bytes in the file's own order: one block when its rows are contiguous, gathered when they are not.
    fn mapped(&self, samples: usize) -> Result<Vec<u8>> {
        let bps = self.layout.dtype.size_bytes();
        let origin = samples
            .checked_mul(bps)
            .and_then(|at| at.checked_add(self.layout.data_offset()))
            .ok_or_else(|| Error::Decode("ICS plane offset overflows usize".to_string()))?;
        let span = self
            .layout
            .plane_span_samples()
            .and_then(|n| n.checked_mul(bps))
            .ok_or_else(|| Error::Decode("ICS plane span overflows usize".to_string()))?;
        let block = self
            .mmap
            .get(origin..origin + span)
            .ok_or_else(|| Error::Decode(self.past_the_end(origin, span)))?;
        if self.layout.contiguous() {
            return Ok(block.to_vec());
        }
        self.gather(block)
    }

    /// The plane's samples inside `block`, which starts at the plane's own first sample: each is copied from
    /// `y · stride_y + x · stride_x` to its place in a row-major plane.
    fn gather(&self, block: &[u8]) -> Result<Vec<u8>> {
        let dims = self.layout.dims;
        let bps = self.layout.dtype.size_bytes();
        let mut out =
            vec![
                0u8;
                dims.y
                    .checked_mul(dims.x)
                    .and_then(|px| px.checked_mul(bps))
                    .ok_or_else(|| Error::Decode("ICS plane size overflows usize".to_string()))?
            ];
        let stride_y = self.layout.stride("y").unwrap_or(dims.x);
        let stride_x = self.layout.stride("x").unwrap_or(1);
        for y in 0..dims.y {
            for x in 0..dims.x {
                let from = (y * stride_y + x * stride_x)
                    .checked_mul(bps)
                    .ok_or_else(|| {
                        Error::Decode("ICS sample offset overflows usize".to_string())
                    })?;
                let source = block
                    .get(from..from + bps)
                    .ok_or_else(|| Error::Decode(self.past_the_end(from, bps)))?;
                let at = (y * dims.x + x) * bps;
                out[at..at + bps].copy_from_slice(source);
            }
        }
        Ok(out)
    }

    /// The plane's bytes, inflated from the start of the stream: the bytes before the plane are decoded and dropped.
    fn inflated(&self, samples: usize) -> Result<Vec<u8>> {
        let bytes = self.layout.plane_bytes()?;
        let skip = samples
            .checked_mul(self.layout.dtype.size_bytes())
            .ok_or_else(|| Error::Decode("ICS plane offset overflows usize".to_string()))?;
        let data = self.mmap.get(self.layout.data_offset()..).ok_or_else(|| {
            Error::Decode("ICS gzip data starts past the end of the file".to_string())
        })?;
        let mut decoder = GzDecoder::new(data);
        let skipped = std::io::copy(
            &mut decoder.by_ref().take(skip as u64),
            &mut std::io::sink(),
        )
        .map_err(|e| Error::Decode(format!("ICS gzip stream is broken before this plane: {e}")))?;
        if skipped < skip as u64 {
            return Err(Error::Decode(format!(
                "ICS gzip stream ends after {skipped} byte(s), before the {skip} byte(s) this plane starts at"
            )));
        }
        let mut out = vec![0u8; bytes];
        decoder.read_exact(&mut out).map_err(|e| {
            Error::Decode(format!(
                "ICS gzip stream ends inside the plane that needs {bytes} byte(s): {e}"
            ))
        })?;
        Ok(out)
    }

    fn plane_samples(&self, t: usize, c: usize, z: usize) -> Result<usize> {
        self.layout
            .plane_samples(t, c, z)
            .ok_or_else(|| Error::Decode("ICS plane offset overflows usize".to_string()))
    }

    fn past_the_end(&self, at: usize, bytes: usize) -> String {
        format!(
            "ICS plane spans bytes {at}..{}, past the end of the {}-byte file",
            at.saturating_add(bytes),
            self.mmap.len()
        )
    }
}
