// SPDX-License-Identifier: Apache-2.0
//! Writes the small ICS files the reader tests read back: a text header, then (for version 2.0) the samples in the
//! file's own axis order. A sample is `plane * 1000 + y * 10 + x`, so a wrong stride, offset or byte order shows.

use std::io::Write;
use std::path::Path;

use flate2::write::GzEncoder;
use flate2::Compression;

pub struct IcsFile {
    pub version_text: String,
    pub axes: Vec<&'static str>,
    pub sizes: Vec<usize>,
    pub bits: u32,
    pub format: String,
    pub sign: String,
    /// `1 2` little-endian, `2 1` big-endian, `None` leaves the line out.
    pub byte_order: Option<String>,
    pub compression: String,
    pub scale: Vec<f32>,
    pub units: Vec<String>,
    /// `layout parameters`; `None` writes the count the entries imply.
    pub parameters: Option<usize>,
    pub field: u8,
    pub crlf: bool,
    /// Extra header lines, written before `end`.
    pub extra: Vec<String>,
    /// Header lines the builder writes that this file leaves out, by category and subcategory.
    pub drop: Vec<(&'static str, &'static str)>,
}

/// The sample the builders write at plane `plane`, `(y, x)`.
pub fn sample_at(plane: usize, y: usize, x: usize) -> i32 {
    (plane * 1000 + y * 10 + x) as i32
}

/// The crate's own plane order: `((t * C + c) * Z + z)`.
pub fn plane_of(t: usize, c: usize, z: usize, channels: usize, sections: usize) -> usize {
    (t * channels + c) * sections + z
}

/// The byte-order line the builders write for `bits` bits per sample.
fn byte_order_for(bits: u32) -> String {
    match bits {
        8 => "1".to_string(),
        16 => "1 2".to_string(),
        _ => "1 2 3 4".to_string(),
    }
}

impl IcsFile {
    /// A 2.0 file of unsigned 16-bit samples in the axes and sizes given.
    pub fn new(axes: &[&'static str], sizes: &[usize], bits: u32) -> IcsFile {
        IcsFile {
            version_text: "2.0".to_string(),
            axes: axes.to_vec(),
            sizes: sizes.to_vec(),
            bits,
            format: if bits == 32 { "real" } else { "integer" }.to_string(),
            sign: "unsigned".to_string(),
            byte_order: Some(byte_order_for(bits)),
            compression: "uncompressed".to_string(),
            scale: vec![0.5; axes.len()],
            units: vec!["micrometers".to_string(); axes.len()],
            parameters: None,
            field: b'\t',
            crlf: false,
            extra: Vec::new(),
            drop: Vec::new(),
        }
    }

    /// A 2.0 file of `x` by `y` by `z` planes, x fastest, with `c` channels and `t` time points.
    pub fn stack(x: usize, y: usize, z: usize, c: usize, t: usize) -> IcsFile {
        IcsFile::new(&["x", "y", "z", "c", "t"], &[x, y, z, c, t], 16)
    }

    /// The header alone: what a 1.0 `.ics` holds.
    pub fn header(&self) -> Vec<u8> {
        let mut out = vec![self.field, b'\n'];
        self.line(
            &mut out,
            &["ics_version".to_string(), self.version_text.clone()],
        );
        self.line(
            &mut out,
            &["filename".to_string(), "synthetic.ids".to_string()],
        );
        if !self.dropped("layout", "parameters") {
            let n = self.parameters.unwrap_or(self.axes.len() + 1);
            self.line(
                &mut out,
                &[
                    "layout".to_string(),
                    "parameters".to_string(),
                    n.to_string(),
                ],
            );
        }
        if !self.dropped("layout", "order") {
            let mut fields = vec![
                "layout".to_string(),
                "order".to_string(),
                "bits".to_string(),
            ];
            fields.extend(self.axes.iter().map(|a| a.to_string()));
            self.line(&mut out, &fields);
        }
        if !self.dropped("layout", "sizes") {
            let mut fields = vec![
                "layout".to_string(),
                "sizes".to_string(),
                self.bits.to_string(),
            ];
            fields.extend(self.sizes.iter().map(|s| s.to_string()));
            self.line(&mut out, &fields);
        }
        if !self.dropped("representation", "format") {
            self.line(
                &mut out,
                &[
                    "representation".to_string(),
                    "format".to_string(),
                    self.format.clone(),
                ],
            );
        }
        if !self.dropped("representation", "sign") {
            self.line(
                &mut out,
                &[
                    "representation".to_string(),
                    "sign".to_string(),
                    self.sign.clone(),
                ],
            );
        }
        if !self.dropped("representation", "compression") {
            self.line(
                &mut out,
                &[
                    "representation".to_string(),
                    "compression".to_string(),
                    self.compression.clone(),
                ],
            );
        }
        if let Some(order) = &self.byte_order {
            if !self.dropped("representation", "byte_order") {
                let mut fields = vec!["representation".to_string(), "byte_order".to_string()];
                fields.extend(order.split(' ').map(|b| b.to_string()));
                self.line(&mut out, &fields);
            }
        }
        if !self.dropped("parameter", "scale") {
            let mut fields = vec!["parameter".to_string(), "scale".to_string()];
            fields.push(self.axes.len().to_string());
            fields.extend(self.scale.iter().map(|s| s.to_string()));
            self.line(&mut out, &fields);
        }
        if !self.dropped("parameter", "units") {
            let mut fields = vec!["parameter".to_string(), "units".to_string()];
            fields.push(self.axes.len().to_string());
            fields.extend(self.units.iter().map(|u| u.to_string()));
            self.line(&mut out, &fields);
        }
        for extra in &self.extra {
            self.line(&mut out, std::slice::from_ref(extra));
        }
        self.line(&mut out, &["end".to_string()]);
        out
    }

    /// Header plus pixels: a 2.0 file in full.
    pub fn bytes(&self) -> Vec<u8> {
        let mut out = self.header();
        out.extend(self.pixel_bytes());
        out
    }

    /// The samples in the file's axis order and byte order, deflated for a gzip file.
    pub fn pixel_bytes(&self) -> Vec<u8> {
        let raw = self.raw_samples();
        if self.compression != "gzip" {
            return raw;
        }
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&raw).expect("deflate the synthetic ICS");
        encoder.finish().expect("finish the gzip stream")
    }

    /// Every sample of the layout, the fastest axis turning first.
    fn raw_samples(&self) -> Vec<u8> {
        let channels = self.size_of(&["ch", "c"]);
        let sections = self.size_of(&["z"]);
        let mut index = vec![0usize; self.axes.len()];
        let mut out = Vec::new();
        for _ in 0..self.sizes.iter().product::<usize>() {
            let at = |names: &[&str]| -> usize {
                self.axes
                    .iter()
                    .position(|a| names.contains(a))
                    .and_then(|slot| index.get(slot).copied())
                    .unwrap_or(0)
            };
            let plane = plane_of(at(&["t"]), at(&["ch", "c"]), at(&["z"]), channels, sections);
            let value = sample_at(plane, at(&["y"]), at(&["x"]));
            out.extend(encode(value, self.bits, self.byte_order.as_deref()));
            for (slot, value) in index.iter_mut().enumerate() {
                *value += 1;
                if *value < self.sizes[slot] {
                    break;
                }
                *value = 0;
            }
        }
        out
    }

    pub fn write(&self, path: &Path) {
        std::fs::write(path, self.bytes()).expect("write the synthetic ICS");
    }

    /// The `.ics` header and its `.ids` pixel file: what a 1.0 file is two of.
    pub fn write_pair(&self, ics: &Path, ids: &Path) {
        std::fs::write(ics, self.header()).expect("write the synthetic .ics");
        std::fs::write(ids, self.pixel_bytes()).expect("write the synthetic .ids");
    }

    fn size_of(&self, names: &[&str]) -> usize {
        self.axes
            .iter()
            .zip(&self.sizes)
            .find(|(axis, _)| names.contains(axis))
            .map(|(_, size)| *size)
            .unwrap_or(1)
    }

    fn dropped(&self, category: &str, subcategory: &str) -> bool {
        self.drop
            .iter()
            .any(|(c, s)| *c == category && *s == subcategory)
    }

    fn line(&self, out: &mut Vec<u8>, fields: &[String]) {
        let separator = (self.field as char).to_string();
        out.extend_from_slice(fields.join(&separator).as_bytes());
        out.push(b'\n');
        if self.crlf {
            let at = out.len() - 1;
            out.insert(at, b'\r');
        }
    }
}

/// One sample in the file's byte order: `1 2` is little-endian, `2 1` big-endian.
fn encode(value: i32, bits: u32, byte_order: Option<&str>) -> Vec<u8> {
    let raw = match bits {
        8 => vec![value as u8],
        16 => (value as i16).to_le_bytes().to_vec(),
        32 => ((value as f32) + 0.5).to_le_bytes().to_vec(),
        _ => panic!("the builders write 8, 16 or 32 bits"),
    };
    match byte_order {
        Some("2 1") | Some("4 3 2 1") => raw.iter().rev().copied().collect(),
        _ => raw,
    }
}
