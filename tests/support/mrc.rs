// SPDX-License-Identifier: Apache-2.0
//! Writes the small MRC2014 files the reader tests read back: a 1024-byte header, an optional extended header, then the
//! sections in file order. Samples follow `section * 1000 + y * 10 + x`, so a wrong offset, stride or byte order shows.

use std::path::Path;

const HEADER_BYTES: usize = 1024;
const MAP: [u8; 4] = *b"MAP ";
const LITTLE_ENDIAN_STAMP: [u8; 2] = [0x44, 0x44];
const BIG_ENDIAN_STAMP: [u8; 2] = [0x11, 0x11];

pub struct MrcFile {
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    pub mode: i32,
    pub mx: i32,
    pub my: i32,
    pub mz: i32,
    pub cella: [f32; 3],
    pub mapc: i32,
    pub mapr: i32,
    pub maps: i32,
    pub ispg: i32,
    /// Extended-header bytes between the header and the data.
    pub extended: usize,
    /// NSYMBT override; `None` writes `extended`.
    pub nsymbt: Option<i32>,
    pub big_endian: bool,
    /// MACHST override; `None` writes the stamp that matches `big_endian`.
    pub machst: Option<[u8; 2]>,
    /// MAP override; `None` writes `MAP `.
    pub map: Option<[u8; 4]>,
    /// Raw sections in file order, each `nx * ny * sample size` bytes.
    pub sections: Vec<Vec<u8>>,
}

impl MrcFile {
    /// A MODE 1 (i16) file of `nz` sections, 10 Å cells, ISPG 0.
    pub fn i16(nx: usize, ny: usize, nz: usize) -> MrcFile {
        let mut file = MrcFile::empty(nx, ny, nz, 1);
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

    /// A MODE 6 (u16) file of `nz` sections.
    pub fn u16(nx: usize, ny: usize, nz: usize) -> MrcFile {
        let mut file = MrcFile::empty(nx, ny, nz, 6);
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

    /// A MODE 2 (f32) file of `nz` sections; the values are exact halves, so no rounding hides an offset.
    pub fn f32(nx: usize, ny: usize, nz: usize) -> MrcFile {
        let mut file = MrcFile::empty(nx, ny, nz, 2);
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

    fn empty(nx: usize, ny: usize, nz: usize, mode: i32) -> MrcFile {
        MrcFile {
            nx,
            ny,
            nz,
            mode,
            mx: 1,
            my: 1,
            mz: 1,
            cella: [10.0, 10.0, 10.0],
            mapc: 1,
            mapr: 2,
            maps: 3,
            ispg: 0,
            extended: 0,
            nsymbt: None,
            big_endian: false,
            machst: None,
            map: None,
            sections: Vec::new(),
        }
    }

    pub fn sample_size(&self) -> usize {
        match self.mode {
            0 | 101 => 1,
            1 | 6 => 2,
            2 | 12 => 4,
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
        self.put(&mut out, 7, self.mx);
        self.put(&mut out, 8, self.my);
        self.put(&mut out, 9, self.mz);
        for (axis, cella) in self.cella.iter().enumerate() {
            let bits = if little {
                cella.to_le_bytes()
            } else {
                cella.to_be_bytes()
            };
            out[40 + axis * 4..44 + axis * 4].copy_from_slice(&bits);
        }
        self.put(&mut out, 16, self.mapc);
        self.put(&mut out, 17, self.mapr);
        self.put(&mut out, 18, self.maps);
        self.put(&mut out, 22, self.ispg);
        self.put(&mut out, 23, self.nsymbt.unwrap_or(self.extended as i32));
        out[208..212].copy_from_slice(&self.map.unwrap_or(MAP));
        out[212..214].copy_from_slice(&self.machst.unwrap_or(if little {
            LITTLE_ENDIAN_STAMP
        } else {
            BIG_ENDIAN_STAMP
        }));

        out.extend(std::iter::repeat(0xAB).take(self.extended));
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
        std::fs::write(path, self.bytes()).expect("write the synthetic MRC");
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
