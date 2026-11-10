// SPDX-License-Identifier: Apache-2.0
//! A decoded plane: its samples in native (host) order, the same bytes `imread dump` writes,
//! so a hash here matches a Bio-Formats dump of the same plane.

use phasit_imread::PlaneBuf;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    pub min: f64,
    pub max: f64,
    pub mean: f64,
}

pub fn to_ne_bytes(buf: &PlaneBuf) -> Vec<u8> {
    match buf {
        PlaneBuf::U8(v) => v.clone(),
        PlaneBuf::U16(v) => v.iter().flat_map(|x| x.to_ne_bytes()).collect(),
        PlaneBuf::I16(v) => v.iter().flat_map(|x| x.to_ne_bytes()).collect(),
        PlaneBuf::F32(v) => v.iter().flat_map(|x| x.to_ne_bytes()).collect(),
    }
}

/// The mean is summed in `f64`, so a 16-bit plane cannot overflow the accumulator.
pub fn statistics(buf: &PlaneBuf) -> Stats {
    match buf {
        PlaneBuf::U8(v) => stats_of(v.iter().map(|s| *s as f64), v.len()),
        PlaneBuf::U16(v) => stats_of(v.iter().map(|s| *s as f64), v.len()),
        PlaneBuf::I16(v) => stats_of(v.iter().map(|s| *s as f64), v.len()),
        PlaneBuf::F32(v) => stats_of(v.iter().map(|s| *s as f64), v.len()),
    }
}

/// `min`/`max` skip a `NaN` sample (`f64::min` keeps the other operand); the mean stays `NaN`, written as `null`.
fn stats_of(values: impl Iterator<Item = f64>, count: usize) -> Stats {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut sum = 0.0f64;
    for value in values {
        min = min.min(value);
        max = max.max(value);
        sum += value;
    }
    Stats {
        min,
        max,
        mean: mean_of(sum, count),
    }
}

/// The mean of a sum over `count` samples; an empty plane has no mean.
fn mean_of(sum: f64, count: usize) -> f64 {
    if count == 0 {
        f64::NAN
    } else {
        sum / count as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u16_bytes_roundtrip() {
        let bytes = to_ne_bytes(&PlaneBuf::U16(vec![1, 0x0102]));
        assert_eq!(bytes.len(), 4);
        assert_eq!(u16::from_ne_bytes([bytes[0], bytes[1]]), 1);
        assert_eq!(u16::from_ne_bytes([bytes[2], bytes[3]]), 0x0102);
    }

    #[test]
    fn u8_statistics() {
        let stats = statistics(&PlaneBuf::U8(vec![0, 11, 5]));
        assert_eq!(stats.min, 0.0);
        assert_eq!(stats.max, 11.0);
        assert_eq!(stats.mean, 16.0 / 3.0);
    }

    #[test]
    fn signed_statistics_keep_the_sign() {
        let stats = statistics(&PlaneBuf::I16(vec![-4, 6]));
        assert_eq!(stats.min, -4.0);
        assert_eq!(stats.max, 6.0);
        assert_eq!(stats.mean, 1.0);
    }

    #[test]
    fn an_empty_plane_has_no_mean() {
        let stats = statistics(&PlaneBuf::U8(Vec::new()));
        assert!(stats.mean.is_nan());
    }
}
