use crate::error::{Error, Result};
use crate::tiff_pages;

use super::PlanePlan;

/// Whether every published scene is served natively. `supports_region()` cannot say which scene, so the decision
/// is per file: one page that is not served makes the whole file not served.
pub(super) fn scenes_serve_regions(
    pages: &[tiff_pages::PageEntry],
    plans: &[PlanePlan],
    real_pages: &[usize],
) -> bool {
    for plan in plans {
        let ifds: &[usize] = match plan {
            PlanePlan::Pages(_) => real_pages,
            PlanePlan::Ome(plan) => &plan.ifds,
        };
        for ifd in ifds {
            if pages.get(*ifd).and_then(|page| page.service.as_ref()).is_none() {
                return false;
            }
        }
    }
    true
}

/// Fails CLOSED on an unreadable tag: collapsing it to `None` would fall back to plain-TIFF geometry and mis-parse OME.
/// A large OME-XML block is handled by raising the decoder's `Limits` (see `tiff_limits`), not by swallowing the error.
pub(super) fn description_from(
    result: tiff::TiffResult<Option<tiff::decoder::ifd::Value>>,
) -> Result<Option<String>> {
    match result {
        Ok(None) => Ok(None),
        Ok(Some(v)) => v.into_string().map(Some).map_err(|e| Error::Decode(e.to_string())),
        Err(e) => Err(Error::Decode(e.to_string())),
    }
}

pub(super) fn looks_like_ome(d: &str) -> bool {
    d.contains("<OME") || d.contains("openmicroscopy.org")
}
