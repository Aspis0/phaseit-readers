use crate::error::{Error, Result};

/// Reads past the end return `Error::Parse`, never a panic.
pub(super) struct Cursor<'a> {
    pub(super) buf: &'a [u8],
    pub(super) pos: usize,
}

impl<'a> Cursor<'a> {
    pub(super) fn new(buf: &'a [u8]) -> Self {
        Cursor { buf, pos: 0 }
    }

    pub(super) fn tell(&self) -> usize {
        self.pos
    }

    pub(super) fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    pub(super) fn read_bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.buf.len())
            .ok_or_else(|| clx_bounds(self.pos, n, self.buf.len()))?;
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    pub(super) fn rest(&self) -> &'a [u8] {
        &self.buf[self.pos.min(self.buf.len())..]
    }

    pub(super) fn skip(&mut self, n: usize) -> Result<()> {
        self.read_bytes(n).map(|_| ())
    }

    pub(super) fn read_u8(&mut self) -> Result<u8> {
        Ok(self.read_bytes(1)?[0])
    }

    pub(super) fn read_u16(&mut self) -> Result<u16> {
        let s = self.read_bytes(2)?;
        Ok(u16::from_le_bytes([s[0], s[1]]))
    }

    pub(super) fn read_i32(&mut self) -> Result<i32> {
        let s = self.read_bytes(4)?;
        Ok(i32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    pub(super) fn read_u32(&mut self) -> Result<u32> {
        let s = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    pub(super) fn read_i64(&mut self) -> Result<i64> {
        let s = self.read_bytes(8)?;
        Ok(i64::from_le_bytes(s.try_into().unwrap()))
    }

    pub(super) fn read_u64(&mut self) -> Result<u64> {
        let s = self.read_bytes(8)?;
        Ok(u64::from_le_bytes(s.try_into().unwrap()))
    }

    pub(super) fn read_f64(&mut self) -> Result<f64> {
        Ok(f64::from_bits(self.read_u64()?))
    }
}

fn clx_bounds(off: usize, need: usize, len: usize) -> Error {
    Error::Parse(format!(
        "ND2 read out of bounds: need {need} bytes at offset {off}, buffer is {len} bytes"
    ))
}
