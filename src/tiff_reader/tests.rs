use crate::types::DType;

use super::*;

#[test]
fn raw_page_bytes_single_channel_matches_plane_bytes() {
    assert_eq!(
        raw_page_bytes(2048, 2048, 1, DType::U16).unwrap(),
        2048 * 2048 * 2
    );
}

#[test]
fn raw_page_bytes_inflated_samples_is_refused() {
    // A crafted SamplesPerPixel must be refused here: the raw decode buffer is sized from it.
    let r = raw_page_bytes(2048, 2048, 1_000_000, DType::U8);
    assert!(matches!(r, Err(Error::OversizedAllocation { .. })));
}

#[test]
fn raw_page_bytes_overflow_is_refused_not_panicking() {
    let r = raw_page_bytes(usize::MAX, 2, 1, DType::U8);
    assert!(matches!(r, Err(Error::OversizedAllocation { .. })));
}

#[test]
fn description_from_ok_none_is_none() {
    let r: tiff::TiffResult<Option<tiff::decoder::ifd::Value>> = Ok(None);
    assert_eq!(description_from(r).unwrap(), None);
}

#[test]
fn description_from_ok_some_extracts_string() {
    let r: tiff::TiffResult<Option<tiff::decoder::ifd::Value>> =
        Ok(Some(tiff::decoder::ifd::Value::Ascii("<OME>hi</OME>".to_string())));
    assert_eq!(description_from(r).unwrap(), Some("<OME>hi</OME>".to_string()));
}

#[test]
fn description_from_err_fails_closed_not_none() {
    let r: tiff::TiffResult<Option<tiff::decoder::ifd::Value>> = Err(tiff::TiffError::LimitsExceeded);
    assert!(matches!(description_from(r), Err(Error::Decode(_))));
}
