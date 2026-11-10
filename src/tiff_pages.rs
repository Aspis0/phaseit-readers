// SPDX-License-Identifier: Apache-2.0
//! One walk over the IFD chain, which the window service is decided from at open: a second pass would read every
//! page's tags twice (the cost the audit measured over the corpus).

use tiff::tags::Tag;

use crate::error::{Error, Result};
use crate::tiff_page::{read_page_facts, PageFacts};
use crate::tiff_region::{page_service, Service, TiffDec};

pub(crate) struct PageEntry {
    /// None for a page this reader cannot model, such as a planar multi-sample page.
    pub facts: Option<PageFacts>,
    pub reduced: bool,
    pub service: Option<Service>,
}

impl PageEntry {
    pub(crate) fn facts(&self, ifd: usize) -> Result<PageFacts> {
        self.facts
            .ok_or_else(|| Error::Decode(format!("IFD {ifd} states no facts this reader can read")))
    }
}

/// Leaves the decoder on the last IFD; a caller that reads the first page seeks back.
pub(crate) fn walk_pages(dec: &mut TiffDec<'_>, mmap: &[u8]) -> Result<Vec<PageEntry>> {
    dec.seek_to_image(0)
        .map_err(|e| Error::Decode(format!("seek to IFD 0: {e}")))?;
    let mut pages = Vec::new();
    let mut index = 0usize;
    loop {
        let reduced = is_reduced(new_subfile_type(dec, index)?);
        // A page the reader cannot model is not published, so it does not fail the file.
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

fn new_subfile_type(dec: &mut TiffDec<'_>, index: usize) -> Result<u32> {
    dec.find_tag_unsigned::<u32>(Tag::NewSubfileType)
        .map_err(|e| Error::Decode(format!("read NewSubfileType (IFD {index}): {e}")))
        .map(|value| value.unwrap_or(0))
}

/// Bits 0 (reduced) and 2 (mask) mark a sub-file; bit 1 (multi-page image) is ignored: those pages ARE focal planes.
fn is_reduced(newsubfiletype: u32) -> bool {
    newsubfiletype & 0b101 != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_reduced_flags_thumbnails_and_masks() {
        assert!(!is_reduced(0));
        assert!(is_reduced(1));
        assert!(is_reduced(4));
        assert!(is_reduced(5));
        assert!(is_reduced(0b101));
        assert!(is_reduced(3));
        assert!(!is_reduced(2));
    }
}
