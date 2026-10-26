// SPDX-License-Identifier: Apache-2.0
//! Format detection and the `open()` dispatch skeleton.
//!
//! Detection combines a small magic-byte probe (first ~16 bytes) with the file
//! extension. The pure matching logic lives in [`detect_from`] so it is unit
//! testable without touching disk.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::error::{Error, Result};
use crate::types::ImageReader;

/// Recognized container formats. Detection maps a file to one of these; every
/// format except OIB and OIF has a reader behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Format {
    /// Plain (or unrecognized-flavour) TIFF; also the routing target for LSM.
    Tiff,
    /// OME-TIFF (TIFF whose `ImageDescription` carries OME-XML). Routed to the
    /// TIFF reader, which confirms OME-ness by scanning the description.
    OmeTiff,
    /// Zeiss LSM (a TIFF variant with private tags).
    Lsm,
    /// Leica LIF / LIFEXT.
    Lif,
    /// Zeiss CZI.
    Czi,
    /// Nikon ND2.
    Nd2,
    /// Olympus OIB (MS-CFB container).
    Oib,
    /// Olympus OIF (text master file referencing external data).
    Oif,
}

/// Number of leading bytes read for the magic probe.
pub const HEADER_PROBE_LEN: usize = 16;

/// MS Compound File Binary (CFB / OLE2) magic — used by Olympus OIB.
const CFB_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
/// Modern ND2 chunk magic.
const ND2_MAGIC: [u8; 4] = [0xDA, 0xCE, 0xBE, 0x0A];

/// True if `h` begins with any TIFF / BigTIFF magic (LE or BE).
fn is_tiff_magic(h: &[u8]) -> bool {
    h.starts_with(b"II\x2A\x00")   // classic TIFF, little-endian
        || h.starts_with(b"MM\x00\x2A") // classic TIFF, big-endian
        || h.starts_with(b"II\x2B\x00") // BigTIFF, little-endian
        || h.starts_with(b"MM\x00\x2B") // BigTIFF, big-endian
}

/// Final extension (after the last `.`), lowercased-in advance by the caller.
///
/// A name with no `.` has no extension and yields `""` (so a file literally
/// named `lif`/`czi`/`tif` is not mistaken for having that extension).
fn ext_of(name: &str) -> &str {
    name.rsplit_once('.').map(|(_, e)| e).unwrap_or("")
}

/// True if `h` looks like a real Leica LIF header: a 4-byte LE test value
/// `0x70`, then a 4-byte block length, then the `0x2A` (`*`) marker at offset 8.
/// Strong enough to detect header-only, so a text file renamed `notes.lif` is
/// rejected while a valid `.lif` with no name is accepted.
fn is_lif_magic(h: &[u8]) -> bool {
    h.len() >= 9 && h[0] == 0x70 && h[1..4] == [0, 0, 0] && h[8] == 0x2A
}

/// Decide a [`Format`] from a header probe and an (optional) file name.
///
/// `name` is the file name (not the full path); it may be `None` when only a
/// byte stream is available. Detection prefers content magic, falling back to
/// the extension. OME-TIFF is recognized via the compound `.ome.tif[f]` suffix
/// (which a single trailing extension cannot express).
pub fn detect_from(header: &[u8], name: Option<&str>) -> Result<Format> {
    let name_l = name.map(|n| n.to_ascii_lowercase());
    let name_ref = name_l.as_deref();
    let ext = name_ref.map(ext_of).unwrap_or("");
    let is_ome = name_ref
        .is_some_and(|n| n.ends_with(".ome.tif") || n.ends_with(".ome.tiff"));

    // --- content magic (most reliable) ---
    if header.starts_with(b"ZISRAW") {
        return Ok(Format::Czi);
    }
    // CFB (OLE2) magic is shared by .doc/.xls/.msi/Thumbs.db; only treat it as
    // Olympus OIB when the extension confirms it, otherwise fall through.
    if header.starts_with(&CFB_MAGIC) && ext == "oib" {
        return Ok(Format::Oib);
    }
    if header.starts_with(&ND2_MAGIC) {
        return Ok(Format::Nd2);
    }
    // LIF is detectable header-only, independent of extension (a text file
    // renamed `notes.lif` is rejected; a valid `.lif` with no name is accepted).
    if is_lif_magic(header) {
        return Ok(Format::Lif);
    }
    if is_tiff_magic(header) {
        if ext == "lsm" {
            return Ok(Format::Lsm);
        }
        if is_ome {
            return Ok(Format::OmeTiff);
        }
        return Ok(Format::Tiff);
    }

    // --- extension-only fallbacks (no decisive magic) ---
    match ext {
        "lsm" => Ok(Format::Lsm),
        "nd2" => Ok(Format::Nd2),
        "oib" => Ok(Format::Oib),
        "oif" => Ok(Format::Oif),
        "czi" => Ok(Format::Czi),
        "tif" | "tiff" if is_ome => Ok(Format::OmeTiff),
        "tif" | "tiff" => Ok(Format::Tiff),
        other => Err(Error::Unsupported(format!(
            "unrecognized image format (extension {other:?}, no known magic)"
        ))),
    }
}

/// Detect the [`Format`] of a file on disk (reads the first [`HEADER_PROBE_LEN`]
/// bytes and inspects the file name).
pub fn detect_format(path: &Path) -> Result<Format> {
    let mut file = File::open(path)?;
    let mut buf = [0u8; HEADER_PROBE_LEN];
    let n = file.read(&mut buf)?;
    let name = path.file_name().and_then(|s| s.to_str());
    detect_from(&buf[..n], name)
}

/// Open a microscopy image file, dispatching to the appropriate reader.
///
/// TIFF, OME-TIFF, LSM, LIF, CZI and ND2 each have a reader; OIB and OIF are
/// detected but have none yet, and return [`Error::NotImplemented`].
pub fn open(path: &Path) -> Result<Box<dyn ImageReader>> {
    let fmt = detect_format(path)?;
    match fmt {
        // The TIFF reader handles plain TIFF and OME-TIFF; LSM has its own reader.
        Format::Tiff => Ok(Box::new(crate::tiff_reader::TiffReader::open(path)?)),
        Format::OmeTiff => Ok(Box::new(crate::tiff_reader::TiffReader::open(path)?)),
        Format::Lsm => Ok(Box::new(crate::lsm_reader::LsmReader::open(path)?)),
        // Leica LIF / LIFEXT.
        Format::Lif => Ok(Box::new(crate::lif_reader::LifReader::open(path)?)),
        // Zeiss CZI / ZISRAW.
        Format::Czi => Ok(Box::new(crate::czi_reader::CziReader::open(path)?)),
        // Nikon ND2.
        Format::Nd2 => Ok(Box::new(crate::nd2_reader::Nd2Reader::open(path)?)),
        // Olympus OIB and OIF: detected, but no reader is implemented yet.
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
        assert_eq!(detect_from(b"II\x2B\x00", Some("big.tif")).unwrap(), Format::Tiff);
        assert_eq!(detect_from(b"MM\x00\x2B", Some("big.tif")).unwrap(), Format::Tiff);
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
        assert_eq!(detect_from(b"II\x2A\x00", Some("scan.lsm")).unwrap(), Format::Lsm);
        // Extension-only fallback (no magic) still routes to Lsm.
        assert_eq!(detect_from(b"", Some("scan.lsm")).unwrap(), Format::Lsm);
    }

    #[test]
    fn lif_detected_by_header_magic_only() {
        // Valid LIF header: 0x70 LE test value, 4-byte block length, 0x2A marker.
        let hdr = [0x70, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x2A];
        assert_eq!(detect_from(&hdr, Some("x.lif")).unwrap(), Format::Lif);
        // Magic is sufficient even with no name at all.
        assert_eq!(detect_from(&hdr, None).unwrap(), Format::Lif);
    }

    #[test]
    fn lif_extension_alone_is_not_enough() {
        // A text file renamed `notes.lif` (no LIF magic) must NOT be detected.
        let e = detect_from(b"just some notes\n", Some("notes.lif"));
        assert!(matches!(e, Err(Error::Unsupported(_))));
        // The old single-byte 0x70 probe must no longer suffice.
        let e = detect_from(&[0x70, 0x00, 0x00, 0x00], Some("x.lif"));
        assert!(matches!(e, Err(Error::Unsupported(_))));
    }

    #[test]
    fn czi_by_magic() {
        assert_eq!(detect_from(b"ZISRAWFILE", Some("a.czi")).unwrap(), Format::Czi);
        // Magic wins even with no/other extension.
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
        // CFB magic is shared by .doc/.xls/.msi/Thumbs.db: without the .oib
        // extension it must NOT be detected as OIB.
        let e = detect_from(&CFB_MAGIC, Some("report.doc"));
        assert!(matches!(e, Err(Error::Unsupported(_))));
        let e = detect_from(&CFB_MAGIC, None);
        assert!(matches!(e, Err(Error::Unsupported(_))));
    }

    #[test]
    fn ext_of_handles_dotless_names() {
        // A dotless name has no extension.
        assert_eq!(ext_of("lif"), "");
        assert_eq!(ext_of("czi"), "");
        assert_eq!(ext_of("archive.tar.gz"), "gz");
        assert_eq!(ext_of("a.tif"), "tif");
        // A file literally named `lif` must not misdetect as LIF.
        let e = detect_from(b"garbage!!", Some("lif"));
        assert!(matches!(e, Err(Error::Unsupported(_))));
    }

    #[test]
    fn oif_by_extension() {
        assert_eq!(detect_from(b"[Version Info]", Some("a.oif")).unwrap(), Format::Oif);
    }

    #[test]
    fn unrecognized_errors() {
        let e = detect_from(b"garbage!!", Some("a.png"));
        assert!(matches!(e, Err(Error::Unsupported(_))));
        let e = detect_from(b"garbage!!", None);
        assert!(matches!(e, Err(Error::Unsupported(_))));
    }
}
