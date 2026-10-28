// SPDX-License-Identifier: Apache-2.0
//! The file's pages, read in ONE pass over the IFD chain.
//!
//! A TIFF file's pages are its IFDs, and everything the reader needs to know
//! about them can be read in one walk: the page's own facts (its size, its
//! sample type, its samples per pixel, its photometric interpretation),
//! whether the page is a real focal plane or a reduced sub-file, and how the
//! page is served by a window read.
//!
//! WHY one walk: the window service is decided at open, and deciding it from
//! a SECOND pass over the chain would read every page's tags twice — the
//! cost the audit measured over the corpus.

use tiff::tags::Tag;

use crate::error::{Error, Result};
use crate::tiff_page::{read_page_facts, PageFacts};
use crate::tiff_region::{page_service, Service, TiffDec};

/// What the file states about one page.
pub(crate) struct PageEntry {
    /// The page's own facts, when they could be read. A page this reader
    /// cannot model (a planar multi-sample page, say) has none.
    pub facts: Option<PageFacts>,
    /// Whether the page is a reduced-resolution / thumbnail / mask sub-file,
    /// i.e. NOT a real focal plane.
    pub reduced: bool,
    /// How the page is served by a window read, if it is.
    pub service: Option<Service>,
}

impl PageEntry {
    /// The page's facts, or an error naming the page that has none.
    pub(crate) fn facts(&self, ifd: usize) -> Result<PageFacts> {
        self.facts
            .ok_or_else(|| Error::Decode(format!("IFD {ifd} states no facts this reader can read")))
    }
}

/// Walk the file's IFD chain once and collect what each page states.
///
/// The walk leaves the decoder on the last IFD; a caller that reads the
/// first page seeks back to it.
pub(crate) fn walk_pages(dec: &mut TiffDec<'_>, mmap: &[u8]) -> Result<Vec<PageEntry>> {
    dec.seek_to_image(0)
        .map_err(|e| Error::Decode(format!("seek to IFD 0: {e}")))?;
    let mut pages = Vec::new();
    let mut index = 0usize;
    loop {
        let reduced = is_reduced(new_subfile_type(dec, index)?);
        // A page this reader cannot model has no facts and no window service;
        // it is not published, so it does not fail the file.
        let facts = read_page_facts(dec).ok();
        let service = match &facts {
            Some(facts) => page_service(dec, mmap, facts),
            None => None,
        };
        pages.push(PageEntry {
            facts,
            reduced,
            service,
        });
        if !dec.more_images() {
            break;
        }
        dec.next_image()
            .map_err(|e| Error::Decode(format!("walk IFD chain: {e}")))?;
        index += 1;
    }
    Ok(pages)
}

/// The `NewSubfileType` (254) of the page the decoder is on: 0 when the file
/// states none.
fn new_subfile_type(dec: &mut TiffDec<'_>, index: usize) -> Result<u32> {
    dec.find_tag_unsigned::<u32>(Tag::NewSubfileType)
        .map_err(|e| Error::Decode(format!("read NewSubfileType (IFD {index}): {e}")))
        .map(|value| value.unwrap_or(0))
}

/// True when an IFD's `NewSubfileType` (tag 254) marks it as a
/// reduced-resolution (bit 0, value 1) or mask (bit 2, value 4) sub-file —
/// i.e. NOT a real focal plane. Bit 1 (value 2, "single page of a multi-page
/// image") is deliberately ignored: those pages ARE focal planes.
fn is_reduced(newsubfiletype: u32) -> bool {
    newsubfiletype & 0b101 != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_reduced_flags_thumbnails_and_masks() {
        // Absent tag is decoded as 0 → real focal plane.
        assert!(!is_reduced(0));
        // Bit 0 (reduced-resolution / thumbnail) and bit 2 (mask) are NOT planes.
        assert!(is_reduced(1));
        assert!(is_reduced(4));
        assert!(is_reduced(5));
        assert!(is_reduced(0b101));
        assert!(is_reduced(3)); // reduced + multi-page bit
                                // Bit 1 ("single page of a multi-page image") IS a real focal plane.
        assert!(!is_reduced(2));
    }
}
