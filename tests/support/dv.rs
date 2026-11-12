// SPDX-License-Identifier: Apache-2.0
//! Writes the small DeltaVision files the reader tests read back: a 1024-byte header, an optional extended header, then the
//! sections in file order. Samples follow `section * 1000 + y * 10 + x`, so a wrong offset, stride or byte order shows.

use std::path::Path;

const HEADER_BYTES: usize = 1024;
const NDVID_LE: i16 = -16224;

pub struct DvFile {
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    pub mode: i32,
    pub dx: f32,
    pub dy: f32,
    pub dz: f32,
    /// Bytes of extended header after the 1024-byte header.
    pub next: usize,
    /// Extended-header override written at word 23; `None` writes `next`.
    pub next_word: Option<i32>,
    pub n_dvid: i16,
    /// Per-section extended-header word counts at bytes 128 and 130.
    pub num_integers: i16,
    pub num_floats: i16,
    pub num_times: i16,
    pub img_sequence: i16,
    pub num_waves: i16,
    pub big_endian: bool,
    /// Raw sections in file order, each `nx * ny * sample size` bytes.
    pub sections: Vec<Vec<u8>>,
}

impl DvFile {
    pub fn u8(nx: usize, ny: usize, nz: usize, w: usize, t: usize, seq: i16) -> DvFile {
        let mut file = DvFile::empty(nx, ny, nz, 0, w, t, seq);
        file.sections = (0..nz)
            .map(|s| {
                samples_i16(nx, ny, s)
                    .iter()
                    .map(|v| (*v as u8 as i16) as u8)
                    .collect()
            })
            .collect();
        file
    }

    pub fn i16(nx: usize, ny: usize, nz: usize, w: usize, t: usize, seq: i16) -> DvFile {
        let mut file = DvFile::empty(nx, ny, nz, 1, w, t, seq);
        file.sections = (0..nz)
            .map(|s| {
                samples_i16(nx, ny, s)
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect()
            })
            .collect();
        file
    }

    pub fn u16(nx: usize, ny: usize, nz: usize, w: usize, t: usize, seq: i16) -> DvFile {
        let mut file = DvFile::empty(nx, ny, nz, 6, w, t, seq);
        file.sections = (0..nz)
            .map(|s| {
                samples_i16(nx, ny, s)
                    .iter()
                    .flat_map(|v| (*v as u16).to_le_bytes())
                    .collect()
            })
            .collect();
        file
    }

    pub fn f32(nx: usize, ny: usize, nz: usize, w: usize, t: usize, seq: i16) -> DvFile {
        let mut file = DvFile::empty(nx, ny, nz, 2, w, t, seq);
        file.sections = (0..nz)
            .map(|s| {
                samples_i16(nx, ny, s)
                    .iter()
                    .flat_map(|v| (*v as f32 + 0.5).to_le_bytes())
                    .collect()
            })
            .collect();
        file
    }

    fn empty(nx: usize, ny: usize, nz: usize, mode: i32, w: usize, t: usize, seq: i16) -> DvFile {
        DvFile {
            nx,
            ny,
            nz,
            mode,
            dx: 0.1,
            dy: 0.1,
            dz: 0.2,
            next: 0,
            next_word: None,
            n_dvid: NDVID_LE,
            num_integers: 0,
            num_floats: 0,
            num_times: t as i16,
            img_sequence: seq,
            num_waves: w as i16,
            big_endian: false,
            sections: Vec::new(),
        }
    }

    pub fn sample_size(&self) -> usize {
        match self.mode {
            0 => 1,
            1 | 6 => 2,
            2 => 4,
            _ => 8,
        }
    }

    pub fn section_bytes(&self) -> usize {
        self.nx * self.ny * self.sample_size()
    }

    pub fn bytes(&self) -> Vec<u8> {
        let mut out = vec![0u8; HEADER_BYTES];
        let little = !self.big_endian;
        self.put(&mut out, 0, self.nx as i32);
        self.put(&mut out, 1, self.ny as i32);
        self.put(&mut out, 2, self.nz as i32);
        self.put(&mut out, 3, self.mode);
        for (axis, val) in [self.dx, self.dy, self.dz].iter().enumerate() {
            let bits = if little {
                val.to_le_bytes()
            } else {
                val.to_be_bytes()
            };
            out[40 + axis * 4..44 + axis * 4].copy_from_slice(&bits);
        }
        self.put(&mut out, 23, self.next_word.unwrap_or(self.next as i32));
        self.short(&mut out, 96, self.n_dvid);
        self.short(&mut out, 128, self.num_integers);
        self.short(&mut out, 130, self.num_floats);
        self.short(&mut out, 180, self.num_times);
        self.short(&mut out, 182, self.img_sequence);
        self.short(&mut out, 196, self.num_waves);

        out.extend(std::iter::repeat_n(0xCD, self.next));
        for section in &self.sections {
            if !self.big_endian {
                out.extend_from_slice(section);
                continue;
            }
            let mut swapped = section.clone();
            for sample in swapped.chunks_exact_mut(self.sample_size()) {
                sample.reverse();
            }
            out.extend_from_slice(&swapped);
        }
        out
    }

    pub fn write(&self, path: &Path) {
        std::fs::write(path, self.bytes()).expect("write the synthetic DeltaVision");
    }

    fn put(&self, out: &mut [u8], index: usize, value: i32) {
        let at = index * 4;
        let raw = if self.big_endian {
            value.to_be_bytes()
        } else {
            value.to_le_bytes()
        };
        out[at..at + 4].copy_from_slice(&raw);
    }

    /// i16 at byte `offset`.
    fn short(&self, out: &mut [u8], offset: usize, value: i16) {
        let raw = if self.big_endian {
            value.to_be_bytes()
        } else {
            value.to_le_bytes()
        };
        out[offset..offset + 2].copy_from_slice(&raw);
    }
}

fn samples_i16(nx: usize, ny: usize, section: usize) -> Vec<i16> {
    (0..ny)
        .flat_map(|y| (0..nx).map(move |x| (section * 1000 + y * 10 + x) as i16))
        .collect()
}

/// The sample the builders write for section `section` at `(y, x)`; x is the fastest axis.
pub fn sample_at(section: usize, y: usize, x: usize) -> i16 {
    (section * 1000 + y * 10 + x) as i16
}
