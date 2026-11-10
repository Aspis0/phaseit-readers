use crate::error::{Error, Result};

use super::*;

#[test]
fn dtype_sizes() {
    assert_eq!(DType::U8.size_bytes(), 1);
    assert_eq!(DType::U16.size_bytes(), 2);
    assert_eq!(DType::I16.size_bytes(), 2);
    assert_eq!(DType::F32.size_bytes(), 4);
}

#[test]
fn from_bytes_u16_endianness() {
    let be = PlaneBuf::from_bytes(DType::U16, Endian::Big, &[0x12, 0x34]).unwrap();
    assert_eq!(be, PlaneBuf::U16(vec![0x1234]));
    let le = PlaneBuf::from_bytes(DType::U16, Endian::Little, &[0x12, 0x34]).unwrap();
    assert_eq!(le, PlaneBuf::U16(vec![0x3412]));
}

#[test]
fn from_bytes_i16_endianness() {
    let be = PlaneBuf::from_bytes(DType::I16, Endian::Big, &[0xFF, 0xFF]).unwrap();
    assert_eq!(be, PlaneBuf::I16(vec![-1]));
    let min = PlaneBuf::from_bytes(DType::I16, Endian::Big, &[0x80, 0x00]).unwrap();
    assert_eq!(min, PlaneBuf::I16(vec![i16::MIN]));
}

#[test]
fn from_bytes_f32_endianness() {
    let le = PlaneBuf::from_bytes(DType::F32, Endian::Little, &[0x00, 0x00, 0x80, 0x3F]).unwrap();
    assert_eq!(le, PlaneBuf::F32(vec![1.0]));
    let be = PlaneBuf::from_bytes(DType::F32, Endian::Big, &[0x3F, 0x80, 0x00, 0x00]).unwrap();
    assert_eq!(be, PlaneBuf::F32(vec![1.0]));
}

#[test]
fn from_bytes_multi_element() {
    let le =
        PlaneBuf::from_bytes(DType::U16, Endian::Little, &[0x01, 0x00, 0x02, 0x00]).unwrap();
    assert_eq!(le, PlaneBuf::U16(vec![1, 2]));
    assert_eq!(le.len(), 2);
    assert!(!le.is_empty());
}

#[test]
fn from_bytes_length_not_multiple_errors() {
    let err = PlaneBuf::from_bytes(DType::U16, Endian::Little, &[0x00, 0x01, 0x02]);
    assert!(matches!(err, Err(Error::Decode(_))));
    let err = PlaneBuf::from_bytes(DType::F32, Endian::Big, &[0x00, 0x01, 0x02]);
    assert!(matches!(err, Err(Error::Decode(_))));
}

#[test]
fn from_bytes_empty_is_empty() {
    let e = PlaneBuf::from_bytes(DType::U16, Endian::Little, &[]).unwrap();
    assert!(e.is_empty());
    assert_eq!(e.len(), 0);
    assert_eq!(e.dtype(), DType::U16);
}

#[test]
fn dims_num_planes_and_plane_elems() {
    let d = Dims5 { t: 2, c: 3, z: 4, y: 5, x: 6 };
    assert_eq!(d.num_planes(), 24);
    assert_eq!(d.plane_elems(), 30);
}

#[test]
fn dims_checked_plane_bytes() {
    let d = Dims5 { t: 1, c: 1, z: 1, y: 2048, x: 2048 };
    assert_eq!(d.checked_plane_bytes(DType::U16).unwrap(), 2048 * 2048 * 2);

    let big = Dims5 { t: 1, c: 1, z: 1, y: 100_000, x: 100_000 };
    assert!(matches!(
        big.checked_plane_bytes(DType::F32),
        Err(Error::OversizedAllocation { .. })
    ));

    let overflow = Dims5 { t: 1, c: 1, z: 1, y: usize::MAX, x: 2 };
    match overflow.checked_plane_bytes(DType::U8) {
        Err(Error::OversizedAllocation { requested, cap }) => {
            assert_eq!(requested, usize::MAX);
            assert_eq!(cap, crate::guard::MAX_DECODED_PLANE_BYTES);
        }
        other => panic!("expected OversizedAllocation, got {other:?}"),
    }
}

#[test]
fn dims_iter_planes_matches_index_order() {
    let d = Dims5 { t: 2, c: 2, z: 3, y: 1, x: 1 };
    let coords: Vec<_> = d.iter_planes().collect();
    assert_eq!(coords.len(), d.num_planes());
    for (i, &(t, c, z)) in coords.iter().enumerate() {
        assert_eq!(d.plane_index(t, c, z), i);
    }
    assert_eq!(coords[0], (0, 0, 0));
    assert_eq!(coords[1], (0, 0, 1));
    assert_eq!(coords[3], (0, 1, 0));
    assert_eq!(coords.last().copied(), Some((1, 1, 2)));
}

#[test]
fn plane_index_saturates_without_panicking() {
    let d = Dims5 { t: 1, c: usize::MAX, z: usize::MAX, y: 1, x: 1 };
    let idx = d.plane_index(2, 2, 2);
    assert_eq!(idx, usize::MAX);
}

#[test]
fn plane_new_enforces_length() {
    let buf = PlaneBuf::U8(vec![0; 6]);
    let p = Plane::new(buf, [2, 3]).unwrap();
    assert_eq!(p.height(), 2);
    assert_eq!(p.width(), 3);
    assert_eq!(p.dtype(), DType::U8);

    let bad = Plane::new(PlaneBuf::U8(vec![0; 5]), [2, 3]);
    assert!(matches!(bad, Err(Error::Decode(_))));

    assert_eq!(p.samples, 1);
}

#[test]
fn plane_new_samples_enforces_length_and_interleave() {
    let rgb = PlaneBuf::U8(vec![0; 12]);
    let p = Plane::new_samples(rgb, [2, 2], 3).unwrap();
    assert_eq!(p.shape, [2, 2]);
    assert_eq!(p.samples(), 3);
    assert_eq!(p.data.len(), 12);

    let bad = Plane::new_samples(PlaneBuf::U8(vec![0; 4]), [2, 2], 3);
    assert!(matches!(bad, Err(Error::Decode(_))));

    let zero = Plane::new_samples(PlaneBuf::U8(vec![]), [2, 2], 0);
    assert!(matches!(zero, Err(Error::Decode(_))));
}

#[test]
fn checked_plane_bytes_accounts_for_samples() {
    let d = Dims5 { t: 1, c: 1, z: 1, y: 174, x: 275 };
    assert_eq!(d.checked_plane_bytes(DType::U8).unwrap(), 174 * 275);
    assert_eq!(
        d.checked_plane_bytes_samples(DType::U8, 3).unwrap(),
        174 * 275 * 3
    );
    assert_eq!(
        d.checked_plane_bytes_samples(DType::U16, 3).unwrap(),
        174 * 275 * 6
    );
}

#[cfg(feature = "ndarray")]
#[test]
fn plane_to_ndarray_roundtrip() {
    let p = Plane::new(PlaneBuf::U16(vec![1, 2, 3, 4, 5, 6]), [2, 3]).unwrap();
    match p.to_ndarray().unwrap() {
        PlaneArray::U16(a) => {
            assert_eq!(a.dim(), (2, 3));
            assert_eq!(a[[0, 0]], 1);
            assert_eq!(a[[1, 2]], 6);
        }
        _ => panic!("wrong ndarray variant"),
    }
}

struct FakeReader {
    meta: ImageMeta,
}

impl FakeReader {
    /// Fixed to `U8`: dtype conversion is covered by the `PlaneBuf` tests.
    fn new(y: usize, x: usize, samples: u32) -> FakeReader {
        let dims = Dims5 { t: 1, c: 1, z: 1, y, x };
        let scene = SceneMeta {
            dims,
            dtype: DType::U8,
            samples,
            px_size_um: None,
            notes: Vec::new(),
            channels: vec![ChannelMeta::default()],
            palette: None,
            name: "fake".to_string(),
        };
        FakeReader {
            meta: ImageMeta { name: "fake".to_string(), scenes: vec![scene] },
        }
    }

    fn synthetic(x: usize, y: usize, s: usize) -> u8 {
        ((x.wrapping_mul(7) ^ y.wrapping_mul(13)).wrapping_add(s * 31) % 251) as u8
    }
}

impl ImageReader for FakeReader {
    fn open(_path: &std::path::Path) -> Result<Self> {
        unimplemented!("FakeReader is test-only and never constructed via open()")
    }

    fn meta(&self) -> &ImageMeta {
        &self.meta
    }

    fn plane(&self, scene: usize, t: usize, c: usize, z: usize) -> Result<Plane> {
        let sm = self.scene_meta(scene)?;
        if t >= sm.dims.t || c >= sm.dims.c || z >= sm.dims.z {
            return Err(Error::OutOfRange { scene, t, c, z });
        }
        let (w, h, samples) = (sm.dims.x, sm.dims.y, sm.samples as usize);
        let mut data = vec![0u8; w * h * samples];
        for yy in 0..h {
            for xx in 0..w {
                for s in 0..samples {
                    data[(yy * w + xx) * samples + s] = FakeReader::synthetic(xx, yy, s);
                }
            }
        }
        Plane::new_samples(PlaneBuf::U8(data), [h, w], sm.samples)
    }
}

/// Independent ground truth: a manual crop that does not reuse `Plane::crop`.
fn manual_crop(_full_w: usize, samples: usize, rect: PixelRect) -> Vec<u8> {
    let mut out = Vec::with_capacity(rect.h * rect.w * samples);
    for row in 0..rect.h {
        let yy = rect.y + row;
        for col in 0..rect.w {
            let xx = rect.x + col;
            for s in 0..samples {
                out.push(FakeReader::synthetic(xx, yy, s));
            }
        }
    }
    out
}

fn assert_region_matches(reader: &FakeReader, w: usize, samples: u32, rect: PixelRect) {
    let region = reader.region(0, 0, 0, 0, rect).unwrap();
    assert_eq!(region.shape, [rect.h, rect.w]);
    assert_eq!(region.samples, samples);
    assert_eq!(region.dtype(), DType::U8);
    let expected = manual_crop(w, samples as usize, rect);
    match region.data {
        PlaneBuf::U8(ref v) => assert_eq!(*v, expected),
        other => panic!("expected U8 plane data, got {other:?}"),
    }
}

#[test]
fn supports_region_defaults_to_false() {
    // The default `region()` must report `false`: that routes pyramid builders to the plane-once path.
    let reader = FakeReader::new(4, 4, 1);
    assert!(!reader.supports_region());
}

#[test]
fn region_grayscale_full_plane_matches_manual_crop() {
    let (h, w) = (10, 8);
    let reader = FakeReader::new(h, w, 1);
    assert_region_matches(&reader, w, 1, PixelRect::new(0, 0, w, h));
}

#[test]
fn region_grayscale_single_pixel() {
    let (h, w) = (10, 8);
    let reader = FakeReader::new(h, w, 1);
    assert_region_matches(&reader, w, 1, PixelRect::new(3, 4, 1, 1));
}

#[test]
fn region_grayscale_top_left_corner() {
    let (h, w) = (10, 8);
    let reader = FakeReader::new(h, w, 1);
    assert_region_matches(&reader, w, 1, PixelRect::new(0, 0, 3, 2));
}

#[test]
fn region_grayscale_bottom_right_corner() {
    let (h, w) = (10, 8);
    let reader = FakeReader::new(h, w, 1);
    assert_region_matches(&reader, w, 1, PixelRect::new(w - 2, h - 3, 2, 3));
}

#[test]
fn region_grayscale_right_edge_column() {
    let (h, w) = (10, 8);
    let reader = FakeReader::new(h, w, 1);
    assert_region_matches(&reader, w, 1, PixelRect::new(w - 1, 0, 1, h));
}

#[test]
fn region_grayscale_bottom_edge_row() {
    let (h, w) = (10, 8);
    let reader = FakeReader::new(h, w, 1);
    assert_region_matches(&reader, w, 1, PixelRect::new(0, h - 1, w, 1));
}

#[test]
fn region_rgb_samples_aware_crop() {
    let (h, w) = (12, 9);
    let reader = FakeReader::new(h, w, 3);
    assert_region_matches(&reader, w, 3, PixelRect::new(2, 3, 4, 5));
    assert_region_matches(&reader, w, 3, PixelRect::new(w - 1, h - 1, 1, 1));
}

#[test]
fn region_out_of_bounds_x_errors() {
    let reader = FakeReader::new(10, 8, 1);
    let err = reader.region(0, 0, 0, 0, PixelRect::new(6, 0, 3, 1));
    assert!(matches!(err, Err(Error::Decode(_))));
}

#[test]
fn region_out_of_bounds_y_errors() {
    let reader = FakeReader::new(10, 8, 1);
    let err = reader.region(0, 0, 0, 0, PixelRect::new(0, 9, 1, 3));
    assert!(matches!(err, Err(Error::Decode(_))));
}

#[test]
fn region_checked_add_overflow_errors_cleanly() {
    let reader = FakeReader::new(10, 8, 1);
    let err = reader.region(0, 0, 0, 0, PixelRect::new(usize::MAX, 0, 2, 1));
    assert!(matches!(err, Err(Error::Decode(_))));
    let err = reader.region(0, 0, 0, 0, PixelRect::new(0, usize::MAX, 1, 2));
    assert!(matches!(err, Err(Error::Decode(_))));
}

#[test]
fn region_scene_out_of_range_errors() {
    let reader = FakeReader::new(10, 8, 1);
    let err = reader.region(1, 0, 0, 0, PixelRect::new(0, 0, 1, 1));
    assert!(matches!(err, Err(Error::SceneOutOfRange { .. })));
}

#[test]
fn pixel_rect_is_empty() {
    assert!(PixelRect::new(0, 0, 0, 5).is_empty());
    assert!(PixelRect::new(0, 0, 5, 0).is_empty());
    assert!(!PixelRect::new(0, 0, 1, 1).is_empty());
}

#[test]
fn plane_crop_matches_region_default() {
    // Plane::crop directly: an overriding backend would call it on an over-fetched plane.
    let reader = FakeReader::new(6, 5, 1);
    let full = reader.plane(0, 0, 0, 0).unwrap();
    let rect = PixelRect::new(1, 2, 3, 3);
    let cropped = full.crop(rect).unwrap();
    let via_region = reader.region(0, 0, 0, 0, rect).unwrap();
    assert_eq!(cropped, via_region);
}

#[test]
fn plane_crop_out_of_bounds_errors() {
    let full = Plane::new(PlaneBuf::U8(vec![0; 6]), [2, 3]).unwrap();
    let err = full.crop(PixelRect::new(2, 0, 2, 1));
    assert!(matches!(err, Err(Error::Decode(_))));
    let err = full.crop(PixelRect::new(0, 1, 1, 2));
    assert!(matches!(err, Err(Error::Decode(_))));
}
