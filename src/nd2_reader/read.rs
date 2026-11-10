use crate::error::{Error, Result};
use crate::types::{Dims5, Endian, PixelRect, Plane, PlaneBuf};

use super::chunks::chunk_data;
use super::Nd2Reader;

impl Nd2Reader {
    pub(super) fn frame_pixels(&self, seq: usize) -> Result<&[u8]> {
        let name = format!("ImageDataSeq|{seq}!");
        let (off, _) = self.map.get(name.as_bytes()).ok_or_else(|| {
            Error::Decode(format!("ND2 missing frame chunk {name}"))
        })?;
        let data = chunk_data(&self.mmap, *off, None)?;
        data.get(8..)
            .ok_or_else(|| Error::Decode("ND2 frame chunk shorter than its 8-byte prefix".to_string()))
    }

    /// A short frame is a broken file; `plane()` and `region()` must say so the same way.
    pub(super) fn check_frame_length(&self, pixels: &[u8]) -> Result<()> {
        let need = self
            .height
            .checked_mul(self.width_bytes)
            .ok_or_else(|| Error::Decode("ND2 frame size overflow".to_string()))?;
        if pixels.len() < need {
            return Err(Error::Decode(format!(
                "ND2 frame has {} pixel bytes, expected at least {need}",
                pixels.len()
            )));
        }
        Ok(())
    }

    pub(super) fn gather_contiguous_rows(
        &self,
        pixels: &[u8],
        rect: PixelRect,
        raw: &mut [u8],
    ) -> Result<()> {
        let bps = self.dtype.size_bytes();
        let row_bytes = rect
            .w
            .checked_mul(bps)
            .ok_or_else(|| Error::Decode("ND2 region row size overflow".to_string()))?;
        let col_offset = rect
            .x
            .checked_mul(bps)
            .ok_or_else(|| Error::Decode("ND2 region column offset overflow".to_string()))?;
        for row in 0..rect.h {
            let src = (rect.y + row)
                .checked_mul(self.width_bytes)
                .and_then(|v| v.checked_add(col_offset))
                .ok_or_else(|| Error::Decode("ND2 region offset overflow".to_string()))?;
            let end = src
                .checked_add(row_bytes)
                .ok_or_else(|| Error::Decode("ND2 region offset overflow".to_string()))?;
            let span = pixels.get(src..end).ok_or_else(|| {
                Error::Decode("ND2 region row runs past the frame".to_string())
            })?;
            let dst = row * row_bytes;
            raw[dst..dst + row_bytes].copy_from_slice(span);
        }
        Ok(())
    }

    pub(super) fn gather_interleaved_pixels(
        &self,
        pixels: &[u8],
        rect: PixelRect,
        c: usize,
        raw: &mut [u8],
    ) -> Result<()> {
        let bps = self.dtype.size_bytes();
        let row_bytes = rect
            .w
            .checked_mul(bps)
            .ok_or_else(|| Error::Decode("ND2 region row size overflow".to_string()))?;
        let pixel_stride = self
            .comp
            .checked_mul(bps)
            .ok_or_else(|| Error::Decode("ND2 region pixel stride overflow".to_string()))?;
        let comp_offset = c
            .checked_mul(bps)
            .ok_or_else(|| Error::Decode("ND2 region component offset overflow".to_string()))?;
        for row in 0..rect.h {
            let row_start = (rect.y + row)
                .checked_mul(self.width_bytes)
                .ok_or_else(|| Error::Decode("ND2 region offset overflow".to_string()))?;
            for i in 0..rect.w {
                let src = row_start
                    .checked_add(
                        (rect.x + i)
                            .checked_mul(pixel_stride)
                            .ok_or_else(|| Error::Decode("ND2 region offset overflow".to_string()))?,
                    )
                    .and_then(|v| v.checked_add(comp_offset))
                    .ok_or_else(|| Error::Decode("ND2 region offset overflow".to_string()))?;
                let end = src
                    .checked_add(bps)
                    .ok_or_else(|| Error::Decode("ND2 region offset overflow".to_string()))?;
                let span = pixels.get(src..end).ok_or_else(|| {
                    Error::Decode("ND2 region pixel runs past the frame".to_string())
                })?;
                let dst = row * row_bytes + i * bps;
                raw[dst..dst + bps].copy_from_slice(span);
            }
        }
        Ok(())
    }

    pub(super) fn decode_plane(&self, seq: usize, c: usize) -> Result<Plane> {
        // The output size is checked before the frame: an oversize plane is refused whatever the frame holds.
        let plane_dims = Dims5 { t: 1, c: 1, z: 1, y: self.height, x: self.width };
        plane_dims.checked_plane_bytes(self.dtype)?;

        let pixels = self.frame_pixels(seq)?;
        self.check_frame_length(pixels)?;

        let bps = self.dtype.size_bytes();
        let need = self
            .height
            .checked_mul(self.width_bytes)
            .ok_or_else(|| Error::Decode("ND2 frame size overflow".to_string()))?;

        if self.comp == 1 && self.width_bytes == self.width * bps {
            let block = &pixels[..need];
            let data = PlaneBuf::from_bytes(self.dtype, Endian::Little, block)?;
            return Plane::new(data, [self.height, self.width]);
        }

        let out_len = self
            .width
            .checked_mul(self.height)
            .and_then(|px| px.checked_mul(bps))
            .ok_or_else(|| Error::Decode("ND2 gathered plane size overflow".to_string()))?;
        let mut gathered = vec![0u8; out_len];
        for y in 0..self.height {
            let row = y * self.width_bytes;
            let dst_row = y * self.width * bps;
            for x in 0..self.width {
                // In-bounds by the open()-time `width_bytes >= width*comp*bps` check.
                let src = row + (x * self.comp + c) * bps;
                let dst = dst_row + x * bps;
                gathered[dst..dst + bps].copy_from_slice(&pixels[src..src + bps]);
            }
        }
        let data = PlaneBuf::from_bytes(self.dtype, Endian::Little, &gathered)?;
        Plane::new(data, [self.height, self.width])
    }
}
