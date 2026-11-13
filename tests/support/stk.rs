// SPDX-License-Identifier: Apache-2.0
//! Writes the small MetaMorph STK files the reader tests read back: a classic TIFF whose first IFD carries the UIC
//! tags, then the planes with no gap. Samples follow `plane * 1000 + y * 10 + x`, so a wrong plane offset, stride or
//! byte order shows.

use std::path::Path;

const UIC1: u16 = 33628;
const UIC2: u16 = 33629;

const TYPE_SHORT: u16 = 3;
const TYPE_LONG: u16 = 4;
const TYPE_RATIONAL: u16 = 5;

const TAG_IMAGE_WIDTH: u16 = 256;
const TAG_IMAGE_LENGTH: u16 = 257;
const TAG_BITS_PER_SAMPLE: u16 = 258;
const TAG_COMPRESSION: u16 = 259;
const TAG_PHOTOMETRIC: u16 = 262;
const TAG_STRIP_OFFSETS: u16 = 273;
const TAG_SAMPLES_PER_PIXEL: u16 = 277;
const TAG_ROWS_PER_STRIP: u16 = 278;
const TAG_STRIP_BYTE_COUNTS: u16 = 279;
const TAG_SAMPLE_FORMAT: u16 = 339;

/// Bytes of one UIC2 plane record.
const RECORD_BYTES: usize = 24;

/// How UIC2 declares its plane table: real files use both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Uic2Type {
    Long,
    Rational,
}

/// One UIC1 value: a rational the reader converts, an ASCII unit name, or an entry the file does not hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Uic1Value {
    Rational(u32, u32),
    Text(String),
    /// The entry's value offset points past the end of the file.
    PastEnd,
}

pub struct StkFile {
    pub width: u32,
    pub height: u32,
    pub planes: usize,
    pub bits: u16,
    pub big_endian: bool,
    /// Rows per strip of plane 0; `None` writes one strip for the whole plane.
    pub rows_per_strip: Option<u32>,
    /// Bytes written between two strips of plane 0; 0 writes them contiguous.
    pub strip_gap: u32,
    /// Bytes the last strip declares fewer than it holds; 0 writes the true count.
    pub strip_bytes_short: u32,
    pub uic2_type: Uic2Type,
    /// UIC2 count override; `None` writes `planes`.
    pub uic2_count: Option<u32>,
    /// UIC2 value offset override; `None` writes the real one.
    pub uic2_at: Option<u32>,
    /// The UIC1 entries in file order; `None` writes no UIC1 at all.
    pub uic1: Option<Vec<(u32, Uic1Value)>>,
    /// How many times the UIC1 list is written, every copy pointing at the same values.
    pub uic1_repeat: usize,
    /// Planes whose bytes are written; `None` writes them all.
    pub planes_written: Option<usize>,
    pub compression: u16,
    pub samples_per_pixel: u16,
    pub sample_format: u16,
    /// BitsPerSample override; `None` writes `bits`.
    pub bits_tag: Option<u16>,
}

/// Where each value that does not fit in its entry sits in the file.
struct Places {
    uic2: u32,
    uic1_pairs: u32,
    uic1_values: Vec<u32>,
    strip_table: u32,
    plane_data: u32,
}

impl StkFile {
    /// An 8-bit, little-endian stack of `planes` `width` x `height` planes, calibrated in µm.
    pub fn u8(width: u32, height: u32, planes: usize) -> StkFile {
        StkFile::new(width, height, planes, 8)
    }

    /// A 16-bit, little-endian stack of `planes` `width` x `height` planes, calibrated in µm.
    pub fn u16(width: u32, height: u32, planes: usize) -> StkFile {
        StkFile::new(width, height, planes, 16)
    }

    fn new(width: u32, height: u32, planes: usize, bits: u16) -> StkFile {
        StkFile {
            width,
            height,
            planes,
            bits,
            big_endian: false,
            rows_per_strip: None,
            strip_gap: 0,
            strip_bytes_short: 0,
            uic2_type: Uic2Type::Long,
            uic2_count: None,
            uic2_at: None,
            uic1: Some(vec![
                (4, Uic1Value::Rational(1, 2)),
                (5, Uic1Value::Rational(1, 4)),
                (6, Uic1Value::Text("um".to_string())),
            ]),
            uic1_repeat: 1,
            planes_written: None,
            compression: 1,
            samples_per_pixel: 1,
            sample_format: 1,
            bits_tag: None,
        }
    }

    pub fn bytes(&self) -> Vec<u8> {
        let entries = self.entry_count();
        let places = self.places(entries);

        let mut out = if self.big_endian {
            b"MM\0*".to_vec()
        } else {
            b"II*\0".to_vec()
        };
        self.put(&mut out, 8, 4);
        self.put(&mut out, entries as u32, 2);
        for (tag, kind, count, value) in self.ifd(&places) {
            self.put(&mut out, u32::from(tag), 2);
            self.put(&mut out, u32::from(kind), 2);
            self.put(&mut out, count, 4);
            // TIFF puts a value narrower than the field in its first bytes, then pads: a SHORT is two bytes, not four.
            if kind == TYPE_SHORT {
                self.put(&mut out, value, 2);
                self.put(&mut out, 0, 2);
            } else {
                self.put(&mut out, value, 4);
            }
        }
        self.put(&mut out, 0, 4);

        self.push_uic2(&mut out);
        self.push_uic1(&mut out, &places);
        self.push_strip_tables(&mut out, &places);
        out.resize(places.plane_data as usize, 0);
        self.push_planes(&mut out);
        out
    }

    pub fn write(&self, path: &Path) {
        std::fs::write(path, self.bytes()).expect("write the synthetic STK");
    }

    /// The IFD entries, sorted by tag as TIFF requires: `(tag, kind, count, value field)`.
    fn ifd(&self, places: &Places) -> Vec<(u16, u16, u32, u32)> {
        let short = |tag: u16, value: u16| (tag, TYPE_SHORT, 1, u32::from(value));
        let long = |tag: u16, count: u32, value: u32| (tag, TYPE_LONG, count, value);
        // A one-strip table is a single LONG, which TIFF holds in the entry's own four bytes.
        let strip = |tag: u16, values: Vec<u32>, at: u32| {
            if values.len() == 1 {
                (tag, TYPE_LONG, 1, values[0])
            } else {
                (tag, TYPE_LONG, values.len() as u32, at)
            }
        };
        let (offsets, counts) = self.strip_table(places.plane_data);
        // The byte-count table follows the offset table: both hold one LONG per strip.
        let counts_at = places.strip_table + 4 * self.strips() as u32;
        let mut entries = vec![
            long(TAG_IMAGE_WIDTH, 1, self.width),
            long(TAG_IMAGE_LENGTH, 1, self.height),
            short(TAG_BITS_PER_SAMPLE, self.bits_tag.unwrap_or(self.bits)),
            short(TAG_COMPRESSION, self.compression),
            short(TAG_PHOTOMETRIC, 1),
            strip(TAG_STRIP_OFFSETS, offsets, places.strip_table),
            short(TAG_SAMPLES_PER_PIXEL, self.samples_per_pixel),
            short(TAG_ROWS_PER_STRIP, self.rows() as u16),
            strip(TAG_STRIP_BYTE_COUNTS, counts, counts_at),
            short(TAG_SAMPLE_FORMAT, self.sample_format),
        ];
        if self.uic1.is_some() {
            entries.push((UIC1, TYPE_LONG, self.uic1_pairs() as u32, places.uic1_pairs));
        }
        let uic2_kind = match self.uic2_type {
            Uic2Type::Long => TYPE_LONG,
            Uic2Type::Rational => TYPE_RATIONAL,
        };
        entries.push((
            UIC2,
            uic2_kind,
            self.uic2_records() as u32,
            self.uic2_at.unwrap_or(places.uic2),
        ));
        entries.sort_by_key(|entry| entry.0);
        entries
    }

    /// The strip table of plane 0: where each strip starts and how many bytes it holds.
    fn strip_table(&self, plane_data: u32) -> (Vec<u32>, Vec<u32>) {
        let rows = self.rows() as usize;
        let row_bytes = self.width as usize * (self.bits as usize / 8);
        let stride = rows * row_bytes + self.strip_gap as usize;
        let mut offsets = Vec::with_capacity(self.strips());
        let mut counts = Vec::with_capacity(self.strips());
        for strip in 0..self.strips() {
            offsets.push(plane_data + (strip * stride) as u32);
            let first = strip * rows;
            let last = (first + rows).min(self.height as usize);
            // A short run keeps its offsets: only the last strip declares fewer bytes than it holds.
            let short = u32::from(strip + 1 == self.strips()) * self.strip_bytes_short;
            counts.push(((last - first) * row_bytes) as u32 - short);
        }
        (offsets, counts)
    }

    fn places(&self, entries: usize) -> Places {
        let strips = self.strips();
        let row_bytes = self.width as usize * (self.bits as usize / 8);
        let plane_bytes = row_bytes * self.height as usize;
        let mut at = 8 + 2 + 12 * entries + 4;
        // A strip table of one value is held in its entry, so it needs no bytes here.
        let table_bytes = if strips == 1 { 0 } else { 4 * strips };

        at = align2(at);
        let uic2 = at as u32;
        at += RECORD_BYTES * self.uic2_records();

        at = align2(at);
        let uic1_pairs = at as u32;
        at += 8 * self.uic1_pairs();
        let mut uic1_values = Vec::new();
        if let Some(pairs) = &self.uic1 {
            for (_, value) in pairs {
                at = align2(at);
                uic1_values.push(at as u32);
                at += match value {
                    Uic1Value::Rational(..) => 8,
                    Uic1Value::Text(text) => 4 + text.len(),
                    Uic1Value::PastEnd => 0,
                };
            }
        }

        at = align2(at);
        let strip_table = at as u32;
        at += 2 * table_bytes;

        let plane_data = align2(at) as u32;
        let _ = plane_bytes;
        Places {
            uic2,
            uic1_pairs,
            uic1_values,
            strip_table,
            plane_data,
        }
    }

    fn entry_count(&self) -> usize {
        10 + usize::from(self.uic1.is_some()) + 1
    }

    /// How many `(id, value)` pairs the UIC1 table holds.
    fn uic1_pairs(&self) -> usize {
        self.uic1
            .as_ref()
            .map_or(0, |pairs| pairs.len() * self.uic1_repeat)
    }

    fn uic2_records(&self) -> usize {
        self.uic2_count.unwrap_or(self.planes as u32) as usize
    }

    /// Rows of one strip of plane 0; a strip count of one means the whole plane.
    fn rows(&self) -> u32 {
        self.rows_per_strip
            .unwrap_or(self.height)
            .clamp(1, self.height.max(1))
    }

    fn strips(&self) -> usize {
        (self.height as usize).div_ceil(self.rows() as usize)
    }

    /// UIC2's records: the z distance as a rational, then the creation and modification date and time.
    fn push_uic2(&self, out: &mut Vec<u8>) {
        for _ in 0..self.uic2_records() {
            for value in [1u32, 1000, 0, 0, 0, 0] {
                self.put(out, value, 4);
            }
        }
    }

    fn push_uic1(&self, out: &mut Vec<u8>, places: &Places) {
        let Some(pairs) = &self.uic1 else {
            return;
        };
        for _ in 0..self.uic1_repeat {
            for (index, (id, value)) in pairs.iter().enumerate() {
                let at = match value {
                    // Past the end of any file these tests write.
                    Uic1Value::PastEnd => u32::MAX,
                    _ => places.uic1_values[index],
                };
                self.put(out, *id, 4);
                self.put(out, at, 4);
            }
        }
        for (_, value) in pairs {
            match value {
                Uic1Value::Rational(numerator, denominator) => {
                    self.put(out, *numerator, 4);
                    self.put(out, *denominator, 4);
                }
                Uic1Value::Text(text) => {
                    self.put(out, text.len() as u32, 4);
                    out.extend_from_slice(text.as_bytes());
                }
                Uic1Value::PastEnd => {}
            }
            // TIFF values begin on a word boundary: the next one is placed at the even byte after this one.
            if out.len() % 2 == 1 {
                out.push(0);
            }
        }
    }

    fn push_strip_tables(&self, out: &mut Vec<u8>, places: &Places) {
        if self.strips() == 1 {
            return;
        }
        let (offsets, counts) = self.strip_table(places.plane_data);
        for value in offsets.into_iter().chain(counts) {
            self.put(out, value, 4);
        }
    }

    fn push_planes(&self, out: &mut Vec<u8>) {
        let rows = self.rows() as usize;
        let written = self.planes_written.unwrap_or(self.planes);
        for plane in 0..written {
            for strip in 0..self.strips() {
                let first = strip * rows;
                let last = (first + rows).min(self.height as usize);
                for y in first..last {
                    for x in 0..self.width {
                        self.push_sample(out, plane, y as u32, x);
                    }
                }
                if strip + 1 < self.strips() {
                    out.extend(std::iter::repeat_n(0u8, self.strip_gap as usize));
                }
            }
        }
    }

    /// One sample, in the file's byte order when it is wider than a byte.
    fn push_sample(&self, out: &mut Vec<u8>, plane: usize, y: u32, x: u32) {
        let value = sample_at(plane, y, x);
        if self.bits == 8 {
            out.push(value as u8);
            return;
        }
        let raw = if self.big_endian {
            value.to_be_bytes()
        } else {
            value.to_le_bytes()
        };
        out.extend_from_slice(&raw);
    }

    /// One unsigned value of `bytes` bytes, in the file's byte order.
    fn put(&self, out: &mut Vec<u8>, value: u32, bytes: usize) {
        let mut raw = value.to_le_bytes();
        if self.big_endian {
            raw[..bytes].reverse();
        }
        out.extend_from_slice(&raw[..bytes]);
    }
}

/// The sample the builder writes for plane `plane` at `(y, x)`; x is the fastest axis.
pub fn sample_at(plane: usize, y: u32, x: u32) -> u16 {
    (plane * 1000 + y as usize * 10 + x as usize) as u16
}

fn align2(at: usize) -> usize {
    if at % 2 == 1 {
        at + 1
    } else {
        at
    }
}
