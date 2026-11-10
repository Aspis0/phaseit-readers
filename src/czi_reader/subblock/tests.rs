use crate::czi_reader::segment::SEGMENT_HEADER_LEN;
use crate::error::Error;

use super::{subblock_pixel_span, SUBBLOCK_MIN_FIXED};

const PIXEL_START: usize = SEGMENT_HEADER_LEN + SUBBLOCK_MIN_FIXED;

/// A ZISRAWSUBBLOCK segment at offset 0 holding `data_len` pixel bytes, with no metadata and no dimensions.
fn segment(allocated: i64, data_len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; PIXEL_START + data_len];
    buf[..14].copy_from_slice(b"ZISRAWSUBBLOCK");
    buf[16..24].copy_from_slice(&allocated.to_le_bytes());
    buf[40..48].copy_from_slice(&(data_len as i64).to_le_bytes());
    buf
}

#[test]
fn an_allocated_size_that_covers_the_pixels_is_accepted() {
    let buf = segment(SUBBLOCK_MIN_FIXED as i64 + 8, 8);
    assert_eq!(subblock_pixel_span(&buf, 0).unwrap(), (PIXEL_START, 8));
}

#[test]
fn a_negative_allocated_size_is_a_parse_error() {
    match subblock_pixel_span(&segment(-1, 8), 0) {
        Err(Error::Parse(msg)) => assert_eq!(msg, "CZI subblock AllocatedSize is negative"),
        _ => panic!("a negative AllocatedSize must be a Parse error"),
    }
}

#[test]
fn pixels_past_the_segment_allocated_size_are_a_parse_error() {
    let buf = segment(SUBBLOCK_MIN_FIXED as i64 + 7, 8);
    match subblock_pixel_span(&buf, 0) {
        Err(Error::Parse(msg)) => {
            assert_eq!(msg, "CZI subblock pixel data runs past its segment AllocatedSize")
        }
        _ => panic!("pixels past the segment's AllocatedSize must be a Parse error"),
    }
}

#[test]
fn a_segment_allocated_past_the_end_of_the_file_is_a_parse_error() {
    match subblock_pixel_span(&segment(1 << 40, 8), 0) {
        Err(Error::Parse(msg)) => {
            assert_eq!(msg, "CZI subblock pixel data runs past its segment AllocatedSize")
        }
        _ => panic!("an AllocatedSize past EOF must be a Parse error"),
    }
}
