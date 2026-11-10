use crate::types::Dims5;

use super::{check_span, check_strides, Axes, ParsedChannel, ParsedDim};

fn dim(bytes_inc: usize) -> ParsedDim {
    ParsedDim { id: 1, n: 4, bytes_inc, length: 1.0 }
}

fn channel(bytes_inc: usize) -> ParsedChannel {
    ParsedChannel { data_type: 0, resolution: 8, bytes_inc, lut_name: String::new() }
}

fn xy_axes() -> Axes {
    Axes { x: dim(1), y: dim(4), z: None, t: None }
}

fn two_channels() -> Dims5 {
    Dims5 { t: 1, c: 2, z: 1, y: 4, x: 4 }
}

#[test]
fn channels_stepping_by_less_than_a_frame_alias_in_memory() {
    let chans = [channel(0), channel(10)];
    assert_eq!(
        check_strides(&xy_axes(), &chans, two_channels(), 16, 0, 0),
        Err("the channels of the image overlap in memory".to_string())
    );
}

#[test]
fn channels_one_frame_apart_do_not_alias() {
    let chans = [channel(0), channel(16)];
    assert_eq!(check_strides(&xy_axes(), &chans, two_channels(), 16, 0, 0), Ok(()));
}

#[test]
fn a_span_beyond_its_memory_block_is_refused_by_name() {
    assert_eq!(
        check_span("MB", 100, 50, 1000, 60),
        Err("the computed data span exceeds its memory block \"MB\"".to_string())
    );
}

#[test]
fn a_span_inside_its_memory_block_is_accepted() {
    assert_eq!(check_span("MB", 100, 50, 1000, 49), Ok(150));
}
