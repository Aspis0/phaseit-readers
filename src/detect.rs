// SPDX-License-Identifier: Apache-2.0
//! Magic-byte probe (first 16 bytes) plus extension; the pure logic is in [`detect_from`], so tests need no disk.
//! N5 is the exception: a dataset is a directory (or its `attributes.json`), which has no magic, so the file
//! system recognises it before the probe; every other format is probed exactly as before.
//! A MetaMorph `.nd` dataset is a text file: its extension and the `"NDInfoFile"` key it opens with name it
//! together, so a text file renamed `.nd` is not one.

use std::ffi::OsStr;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::error::{Error, Result};
use crate::guard::MAX_ATTRIBUTES_BYTES;
use crate::types::ImageReader;

/// Every format except OIB and OIF has a reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Format {
    /// A classic TIFF: [`detect_format`] and [`open`] call it an STK when its first IFD carries UIC2.
    Tiff,
    /// Routed to the TIFF reader, which confirms OME-ness by scanning the description.
    OmeTiff,
    Lsm,
    Lif,
    Czi,
    Nd2,
    /// MRC2014 (electron microscopy); the `MAP ` magic sits at offset 208, so only the extension detects it.
    Mrc,
    /// DeltaVision (widefield/deconvolution); the nDVID magic sits at offset 96, so only the extension detects it.
    Dv,
    /// ICS/IDS (Image Cytometry Standard); `ics_version` sits at byte 2, so content detects it.
    Ics,
    /// MetaMorph STK: a classic TIFF whose first IFD carries the private UIC2 plane table.
    Stk,
    /// N5 (Saalfeld lab / BigDataViewer): a directory holding `attributes.json`; no magic, the layout detects it.
    N5,
    /// MetaMorph Multi-Dimensional Acquisition: a `.nd` text file listing the acquisition, beside its image files.
    MetaMorphNd,
    Oib,
    /// Olympus OIF (text master file referencing external data).
    Oif,
}

pub const HEADER_PROBE_LEN: usize = 16;

const CFB_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
const ND2_MAGIC: [u8; 4] = [0xDA, 0xCE, 0xBE, 0x0A];
/// The ICS header opens with its two separators, then `ics_version`: bytes 2..13 of the probe.
const ICS_MAGIC: [u8; 11] = *b"ics_version";
/// A MetaMorph `.nd` file opens with its `"NDInfoFile"` key, which the probe reaches.
const ND_MAGIC: &[u8] = b"\"NDInfoFile\"";
/// A UTF-8 byte order mark may precede the first key.
const UTF8_BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];

fn is_tiff_magic(h: &[u8]) -> bool {
    h.starts_with(b"II\x2A\x00")
        || h.starts_with(b"MM\x00\x2A")
        || h.starts_with(b"II\x2B\x00")
        || h.starts_with(b"MM\x00\x2B")
}

/// An N5 dataset is a directory holding `attributes.json` with a `dimensions` key, or that file itself.
fn is_n5(path: &Path) -> Result<bool> {
    let attributes = match path.is_dir() {
        true => path.join("attributes.json"),
        // The name is case-sensitive: another spelling of it is not a dataset's attributes.
        false if path.file_name() == Some(OsStr::new("attributes.json")) => path.to_path_buf(),
        false => return Ok(false),
    };
    has_dimensions(&attributes)
}

/// An `attributes.json` is probed exactly as far as the reader itself reads it: a file past the cap is refused
/// here as `N5Reader::open` refuses it, never mistaken for another format.
fn has_dimensions(attributes: &Path) -> Result<bool> {
    let Ok(file) = File::open(attributes) else {
        return Ok(false);
    };
    let mut head = Vec::new();
    let read = file
        .take(MAX_ATTRIBUTES_BYTES + 1)
        .read_to_end(&mut head)
        .unwrap_or(0);
    if read > MAX_ATTRIBUTES_BYTES as usize {
        return Err(Error::OversizedAllocation {
            requested: read,
            cap: MAX_ATTRIBUTES_BYTES as usize,
        });
    }
    Ok(serde_json::from_slice::<serde_json::Value>(&head)
        .ok()
        .is_some_and(|json| json.get("dimensions").is_some()))
}

/// A name with no `.` yields `""`, so a file named `lif` has no extension.
fn ext_of(name: &str) -> &str {
    name.rsplit_once('.').map(|(_, e)| e).unwrap_or("")
}

/// A MetaMorph `.nd` file opens with its `"NDInfoFile"` key; a byte order mark may precede it.
fn is_nd_magic(header: &[u8]) -> bool {
    let body = header.strip_prefix(&UTF8_BOM[..]).unwrap_or(header);
    body.len() >= ND_MAGIC.len() && body[..ND_MAGIC.len()].eq_ignore_ascii_case(ND_MAGIC)
}

/// A real LIF header: LE `0x70` test value, a block length, then `0x2A` at offset 8.
/// Strong enough to detect without the extension: a text file renamed `notes.lif` is rejected.
fn is_lif_magic(h: &[u8]) -> bool {
    h.len() >= 9 && h[0] == 0x70 && h[1..4] == [0, 0, 0] && h[8] == 0x2A
}

/// Content magic wins; the extension is the fallback. OME-TIFF is recognized by its compound
/// `.ome.tif[f]` suffix, which one trailing extension cannot express.
/// The 16-byte probe cannot see the first IFD, so [`Format::Tiff`] here is refined to [`Format::Stk`] by
/// [`detect_format`] and [`open`], the two that can read it.
pub fn detect_from(header: &[u8], name: Option<&str>) -> Result<Format> {
    let name_l = name.map(|n| n.to_ascii_lowercase());
    let name_ref = name_l.as_deref();
    let ext = name_ref.map(ext_of).unwrap_or("");
    let is_ome = name_ref.is_some_and(|n| n.ends_with(".ome.tif") || n.ends_with(".ome.tiff"));

    if header.starts_with(b"ZISRAW") {
        return Ok(Format::Czi);
    }
    // CFB magic is shared by .doc/.xls/.msi: Olympus OIB only when the extension confirms it.
    if header.starts_with(&CFB_MAGIC) && ext == "oib" {
        return Ok(Format::Oib);
    }
    if header.starts_with(&ND2_MAGIC) {
        return Ok(Format::Nd2);
    }
    if is_lif_magic(header) {
        return Ok(Format::Lif);
    }
    if header.get(2..13) == Some(&ICS_MAGIC) {
        return Ok(Format::Ics);
    }
    // A MetaMorph dataset is a `.nd` text file: the extension and the `"NDInfoFile"` key name it together.
    if ext == "nd" && is_nd_magic(header) {
        return Ok(Format::MetaMorphNd);
    }
    if is_tiff_magic(header) {
        if ext == "lsm" {
            return Ok(Format::Lsm);
        }
        // The `.stk` name is enough: the reader refuses a TIFF without UIC2 with the STK sentence.
        if ext == "stk" {
            return Ok(Format::Stk);
        }
        if is_ome {
            return Ok(Format::OmeTiff);
        }
        return Ok(Format::Tiff);
    }

    match ext {
        "lsm" => Ok(Format::Lsm),
        "nd2" => Ok(Format::Nd2),
        "oib" => Ok(Format::Oib),
        "oif" => Ok(Format::Oif),
        "czi" => Ok(Format::Czi),
        "mrc" | "mrcs" | "rec" | "st" | "map" | "ali" => Ok(Format::Mrc),
        "dv" | "r3d" => Ok(Format::Dv),
        "ics" | "ids" => Ok(Format::Ics),
        "tif" | "tiff" if is_ome => Ok(Format::OmeTiff),
        "tif" | "tiff" => Ok(Format::Tiff),
        other => Err(Error::Unsupported(format!(
            "unrecognized image format (extension {other:?}, no known magic)"
        ))),
    }
}

/// The format of one file: [`detect_from`] plus the one fact the probe cannot reach. This and [`open`] decide the
/// format, so a caller that names a reader from `detect_format` never reads an STK as a one-plane TIFF.
pub fn detect_format(path: &Path) -> Result<Format> {
    if is_n5(path)? {
        return Ok(Format::N5);
    }
    let mut file = File::open(path)?;
    let mut buf = [0u8; HEADER_PROBE_LEN];
    let n = file.read(&mut buf)?;
    let name = path.file_name().and_then(|s| s.to_str());
    match detect_from(&buf[..n], name)? {
        Format::Tiff if crate::stk_reader::carries_uic2(path)? => Ok(Format::Stk),
        format => Ok(format),
    }
}

/// OIB and OIF are detected but have no reader: they return [`Error::NotImplemented`].
pub fn open(path: &Path) -> Result<Box<dyn ImageReader>> {
    let fmt = detect_format(path)?;
    match fmt {
        Format::Tiff => Ok(Box::new(crate::tiff_reader::TiffReader::open(path)?)),
        Format::Stk => Ok(Box::new(crate::stk_reader::StkReader::open(path)?)),
        Format::OmeTiff => Ok(Box::new(crate::tiff_reader::TiffReader::open(path)?)),
        Format::Lsm => Ok(Box::new(crate::lsm_reader::LsmReader::open(path)?)),
        Format::Lif => Ok(Box::new(crate::lif_reader::LifReader::open(path)?)),
        Format::Czi => Ok(Box::new(crate::czi_reader::CziReader::open(path)?)),
        Format::Nd2 => Ok(Box::new(crate::nd2_reader::Nd2Reader::open(path)?)),
        Format::Mrc => Ok(Box::new(crate::mrc_reader::MrcReader::open(path)?)),
        Format::Dv => Ok(Box::new(crate::dv_reader::DvReader::open(path)?)),
        Format::Ics => Ok(Box::new(crate::ics_reader::IcsReader::open(path)?)),
        Format::N5 => Ok(Box::new(crate::n5_reader::N5Reader::open(path)?)),
        Format::MetaMorphNd => Ok(Box::new(crate::nd_reader::NdReader::open(path)?)),
        Format::Oib => Err(Error::NotImplemented(fmt)),
        Format::Oif => Err(Error::NotImplemented(fmt)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiff_little_and_big_endian() {
        assert_eq!(
            detect_from(b"II\x2A\x00\x08\x00\x00\x00", Some("a.tif")).unwrap(),
            Format::Tiff
        );
        assert_eq!(
            detect_from(b"MM\x00\x2A\x00\x00\x00\x08", Some("a.tiff")).unwrap(),
            Format::Tiff
        );
    }

    #[test]
    fn bigtiff_little_and_big_endian() {
        assert_eq!(
            detect_from(b"II\x2B\x00", Some("big.tif")).unwrap(),
            Format::Tiff
        );
        assert_eq!(
            detect_from(b"MM\x00\x2B", Some("big.tif")).unwrap(),
            Format::Tiff
        );
    }

    #[test]
    fn ome_tiff_by_compound_extension() {
        assert_eq!(
            detect_from(b"II\x2A\x00", Some("stack.ome.tif")).unwrap(),
            Format::OmeTiff
        );
        assert_eq!(
            detect_from(b"II\x2A\x00", Some("stack.OME.TIFF")).unwrap(),
            Format::OmeTiff
        );
    }

    #[test]
    fn lsm_is_tiff_magic_plus_extension() {
        // LSM files carry TIFF magic; the .lsm extension routes them to Lsm.
        assert_eq!(
            detect_from(b"II\x2A\x00", Some("scan.lsm")).unwrap(),
            Format::Lsm
        );
        assert_eq!(detect_from(b"", Some("scan.lsm")).unwrap(), Format::Lsm);
    }

    #[test]
    fn lif_detected_by_header_magic_only() {
        let hdr = [0x70, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x2A];
        assert_eq!(detect_from(&hdr, Some("x.lif")).unwrap(), Format::Lif);
        assert_eq!(detect_from(&hdr, None).unwrap(), Format::Lif);
    }

    #[test]
    fn lif_extension_alone_is_not_enough() {
        let e = detect_from(b"just some notes\n", Some("notes.lif"));
        assert!(matches!(e, Err(Error::Unsupported(_))));
        let e = detect_from(&[0x70, 0x00, 0x00, 0x00], Some("x.lif"));
        assert!(matches!(e, Err(Error::Unsupported(_))));
    }

    #[test]
    fn czi_by_magic() {
        assert_eq!(
            detect_from(b"ZISRAWFILE", Some("a.czi")).unwrap(),
            Format::Czi
        );
        assert_eq!(detect_from(b"ZISRAW\x00\x00", None).unwrap(), Format::Czi);
    }

    #[test]
    fn nd2_by_magic_and_extension() {
        assert_eq!(detect_from(&ND2_MAGIC, Some("a.nd2")).unwrap(), Format::Nd2);
        assert_eq!(detect_from(&[0x00], Some("a.nd2")).unwrap(), Format::Nd2);
    }

    #[test]
    fn oib_by_cfb_magic_requires_extension() {
        assert_eq!(detect_from(&CFB_MAGIC, Some("a.oib")).unwrap(), Format::Oib);
        let e = detect_from(&CFB_MAGIC, Some("report.doc"));
        assert!(matches!(e, Err(Error::Unsupported(_))));
        let e = detect_from(&CFB_MAGIC, None);
        assert!(matches!(e, Err(Error::Unsupported(_))));
    }

    #[test]
    fn ics_by_content_and_by_extension() {
        // The two separators come first, so `ics_version` sits at bytes 2..13 of the probe.
        let header = b"\t\nics_version\t2.0\n";
        assert_eq!(detect_from(header, Some("a.dat")).unwrap(), Format::Ics);
        assert_eq!(detect_from(header, None).unwrap(), Format::Ics);
        assert_eq!(detect_from(b"", Some("a.ics")).unwrap(), Format::Ics);
        assert_eq!(detect_from(b"", Some("a.ICS")).unwrap(), Format::Ics);
        assert_eq!(detect_from(b"", Some("a.ids")).unwrap(), Format::Ics);
        assert_eq!(detect_from(b"", Some("a.IDS")).unwrap(), Format::Ics);
        // A text file whose byte 2 is not `ics_version` is not detected by content.
        assert!(matches!(
            detect_from(b"\t\nics_versio\t2.0\n", Some("a.dat")),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn stk_by_extension_and_tiff_magic() {
        // An STK is a classic TIFF: the name alone, with no TIFF magic, is not one.
        assert!(matches!(
            detect_from(b"", Some("stack.stk")),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            detect_from(b"", Some("stack.STK")),
            Err(Error::Unsupported(_))
        ));
        assert_eq!(
            detect_from(b"II\x2A\x00", Some("stack.stk")).unwrap(),
            Format::Stk
        );
        // The UIC2 that names an STK is in the first IFD, which the 16-byte probe does not reach: a .tif stays a TIFF
        // here and is routed to the STK reader by `open`.
        assert_eq!(
            detect_from(b"II\x2A\x00", Some("stack.tif")).unwrap(),
            Format::Tiff
        );
    }

    #[test]
    fn ext_of_handles_dotless_names() {
        assert_eq!(ext_of("lif"), "");
        assert_eq!(ext_of("czi"), "");
        assert_eq!(ext_of("archive.tar.gz"), "gz");
        assert_eq!(ext_of("a.tif"), "tif");
        let e = detect_from(b"garbage!!", Some("lif"));
        assert!(matches!(e, Err(Error::Unsupported(_))));
    }

    #[test]
    fn oif_by_extension() {
        assert_eq!(
            detect_from(b"[Version Info]", Some("a.oif")).unwrap(),
            Format::Oif
        );
    }

    #[test]
    fn a_directory_of_attributes_is_an_n5_dataset_and_nothing_else_is() {
        // A dataset is recognised by the `dimensions` member of its `attributes.json`, so nothing else is one.
        let scratch = std::env::temp_dir().join(format!("phasit_n5_detect_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        let cases = [
            (
                "attributes.json",
                r#"{"dimensions":[4,3],"blockSize":[2,2],"dataType":"uint8"}"#,
                true,
            ),
            (
                "attributes.json",
                "\n{ \"dimensions\": [4, 3], \"blockSize\": [2, 2] }",
                true,
            ),
            ("Attributes.json", r#"{"dimensions":[4,3]}"#, false),
            ("attributes.json.bak", r#"{"dimensions":[4,3]}"#, false),
            (
                "attributes.json",
                r#"{"pixelResolution":{"dimensions":[1,1]}}"#,
                false,
            ),
            ("attributes.json", "", false),
        ];
        for (index, (name, text, is_dataset)) in cases.iter().enumerate() {
            // One folder per case: a case-insensitive file system would open the previous case's file.
            let folder = scratch.join(index.to_string());
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join(name), text).unwrap();
            let path = folder.join(name);
            assert_eq!(
                is_n5(&path).unwrap(),
                *is_dataset,
                "{}: expected {is_dataset}",
                path.display()
            );
            if *name == "attributes.json" {
                // A case-insensitive file system opens another spelling too; the directory probe is asked only of
                // the name the format states.
                assert_eq!(
                    is_n5(&folder).unwrap(),
                    *is_dataset,
                    "{}: by directory",
                    path.display()
                );
            }
            assert_eq!(
                detect_format(&path).ok().is_some_and(|f| f == Format::N5),
                *is_dataset,
                "{}",
                path.display()
            );
            if *is_dataset {
                assert!(
                    detect_format(&folder).ok().is_some_and(|f| f == Format::N5),
                    "{}",
                    folder.display()
                );
            }
        }
        let empty = scratch.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(
            !is_n5(&empty).unwrap(),
            "a directory without attributes.json is no dataset"
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn a_metamorph_nd_dataset_is_detected_by_its_extension_and_its_first_key() {
        let header = b"\"NDInfoFile\", Version 1.0\r\n";
        assert_eq!(
            detect_from(header, Some("acq.nd")).unwrap(),
            Format::MetaMorphNd
        );
        assert_eq!(
            detect_from(header, Some("acq.ND")).unwrap(),
            Format::MetaMorphNd
        );
        // A byte order mark may precede the first key.
        let mut with_bom = UTF8_BOM.to_vec();
        with_bom.extend_from_slice(header);
        assert_eq!(
            detect_from(&with_bom, Some("acq.nd")).unwrap(),
            Format::MetaMorphNd
        );
        // The extension alone is not enough: a text file renamed `.nd` is not a dataset.
        assert!(matches!(
            detect_from(b"just some notes\n", Some("acq.nd")),
            Err(Error::Unsupported(_))
        ));
        // And the key alone does not name it: the extension is part of the format's name.
        assert!(matches!(
            detect_from(header, Some("acq.txt")),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn unrecognized_errors() {
        let e = detect_from(b"garbage!!", Some("a.png"));
        assert!(matches!(e, Err(Error::Unsupported(_))));
        let e = detect_from(b"garbage!!", None);
        assert!(matches!(e, Err(Error::Unsupported(_))));
    }
}
