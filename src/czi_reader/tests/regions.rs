use crate::czi_reader::CziReader;
use crate::error::Error;
use crate::types::{DType, Dims5, ImageReader, PixelRect, PlaneBuf};

use super::support::*;

// Tiles at X.Start = i32::MIN and 0: the i64 `Start - origin` offset is past i32::MAX on both paths.
// open() succeeds, plane() refuses via the cap, and a bounded region() over the far tile decodes.
#[test]
fn crafted_mosaic_extreme_start_opens_plane_refuses_region_ok() {
    let tiles = vec![
        TileSpec { pixel_type: 0, xs: i32::MIN, xsz: 2, ys: 0, ysz: 2, c: 0, z: 0, m: 0, pixels: vec![1, 2, 3, 4] },
        TileSpec { pixel_type: 0, xs: 0, xsz: 2, ys: 0, ysz: 2, c: 0, z: 0, m: 1, pixels: vec![5, 6, 7, 8] },
    ];
    let reader = open_synth(&build_czi(&tiles));
    let scene = reader.meta().scene(0).unwrap();
    let far_x = (i32::MIN as i64).unsigned_abs() as usize;
    assert_eq!(scene.dims.x, far_x + 2);
    assert_eq!(scene.dims.y, 2);
    // The composed plane is past the 4 GiB cap: plane() refuses it cleanly, never by an overflow panic.
    assert!(matches!(
        reader.plane(0, 0, 0, 0),
        Err(Error::OversizedAllocation { .. })
    ));
    // Tile B sits at scene-x 2^31; the bounded window decodes it through the i64 path, skipping tile A.
    let b = reader.region(0, 0, 0, 0, PixelRect::new(far_x, 0, 2, 2)).unwrap();
    assert_eq!(b.shape, [2, 2]);
    match b.data {
        PlaneBuf::U8(v) => assert_eq!(v, vec![5, 6, 7, 8]),
        other => panic!("expected U8, got {:?}", other.dtype()),
    }
}

#[test]
fn supports_region_is_true_for_czi() {
    // supports_region() must be true: pyramid builders then stream per tile instead of materializing the plane.
    let reader = open_synth(&build_czi(&mosaic_tiles(0, g8, [1, 21, 41])));
    assert!(reader.supports_region());
}

#[test]
fn region_matches_crop_gray8_mosaic() {
    let reader = open_synth(&build_czi(&mosaic_tiles(0, g8, [1, 21, 41])));
    let scene = reader.meta().scene(0).unwrap();
    assert_eq!(scene.dims, Dims5 { t: 1, c: 1, z: 1, y: 4, x: 4 });
    assert_eq!(scene.samples, 1);
    assert_region_eq_crop(&reader, &mosaic_rects_4x4());
}

#[test]
fn region_matches_crop_gray16_mosaic() {
    let reader = open_synth(&build_czi(&mosaic_tiles(1, g16, [100, 200, 40])));
    let scene = reader.meta().scene(0).unwrap();
    assert_eq!(scene.dims, Dims5 { t: 1, c: 1, z: 1, y: 4, x: 4 });
    assert_eq!(scene.dtype, DType::U16);
    assert_eq!(scene.samples, 1);
    assert_region_eq_crop(&reader, &mosaic_rects_4x4());
}

#[test]
fn region_matches_crop_bgr24_mosaic() {
    let reader = open_synth(&build_czi(&mosaic_tiles(3, bgr, [1, 60, 120])));
    let scene = reader.meta().scene(0).unwrap();
    assert_eq!(scene.dims, Dims5 { t: 1, c: 1, z: 1, y: 4, x: 4 });
    assert_eq!(scene.dtype, DType::U8);
    assert_eq!(scene.samples, 3);
    assert_region_eq_crop(&reader, &mosaic_rects_4x4());
}

#[test]
fn oversize_scene_opens_plane_refuses_region_works() {
    let reader = open_synth(&oversize_scene());
    let scene = reader.meta().scene(0).unwrap();
    assert_eq!(scene.dims, Dims5 { t: 1, c: 1, z: 1, y: 70002, x: 70002 });
    assert_eq!(scene.dtype, DType::U8);
    assert!(70002usize * 70002 > crate::guard::MAX_DECODED_PLANE_BYTES);

    // plane() still materializes the whole plane, so it MUST refuse.
    assert!(
        matches!(reader.plane(0, 0, 0, 0), Err(Error::OversizedAllocation { .. })),
        "plane() on an oversize scene must be refused by the cap"
    );

    let a = reader.region(0, 0, 0, 0, PixelRect::new(0, 0, 2, 2)).unwrap();
    assert_eq!(a.shape, [2, 2]);
    match a.data {
        PlaneBuf::U8(v) => assert_eq!(v, vec![1, 2, 3, 4]),
        other => panic!("expected U8, got {:?}", other.dtype()),
    }

    let b = reader.region(0, 0, 0, 0, PixelRect::new(70000, 70000, 2, 2)).unwrap();
    assert_eq!(b.shape, [2, 2]);
    match b.data {
        PlaneBuf::U8(v) => assert_eq!(v, vec![5, 6, 7, 8]),
        other => panic!("expected U8, got {:?}", other.dtype()),
    }

    let gap = reader.region(0, 0, 0, 0, PixelRect::new(10, 10, 2, 2)).unwrap();
    assert_eq!(gap.shape, [2, 2]);
    match gap.data {
        PlaneBuf::U8(v) => assert_eq!(v, vec![0, 0, 0, 0]),
        other => panic!("expected U8, got {:?}", other.dtype()),
    }
}

#[test]
fn region_own_size_exceeding_cap_is_refused() {
    // A rect whose own size passes the cap is refused before allocation, though its scene opened.
    let reader = open_synth(&oversize_scene());
    let huge = PixelRect::new(0, 0, 70000, 70000);
    assert!(70000usize * 70000 > crate::guard::MAX_DECODED_PLANE_BYTES);
    assert!(
        matches!(
            reader.region(0, 0, 0, 0, huge),
            Err(Error::OversizedAllocation { .. })
        ),
        "a rect larger than the cap must be refused"
    );
}

#[test]
fn region_out_of_bounds_and_overflow_are_clean_errors() {
    let reader = open_synth(&build_czi(&mosaic_tiles(0, g8, [1, 21, 41])));
    assert!(matches!(
        reader.region(0, 0, 0, 0, PixelRect::new(3, 0, 2, 1)),
        Err(Error::Decode(_))
    ));
    assert!(matches!(
        reader.region(0, 0, 0, 0, PixelRect::new(0, 3, 1, 2)),
        Err(Error::Decode(_))
    ));
    assert!(matches!(
        reader.region(0, 0, 0, 0, PixelRect::new(usize::MAX, 0, 2, 1)),
        Err(Error::Decode(_))
    ));
    assert!(matches!(
        reader.region(0, 0, 0, 0, PixelRect::new(0, usize::MAX, 1, 2)),
        Err(Error::Decode(_))
    ));
    assert!(matches!(
        reader.region(1, 0, 0, 0, PixelRect::new(0, 0, 1, 1)),
        Err(Error::SceneOutOfRange { scene: 1, .. })
    ));
    assert!(matches!(
        reader.region(0, 1, 0, 0, PixelRect::new(0, 0, 1, 1)),
        Err(Error::OutOfRange { t: 1, .. })
    ));
}

// Real-file parity: region() is byte-exact against plane().crop() on a real Gray16 mosaic CZI.
// Read from PHASIT_REAL_DATA_DIR; skips when absent, and fails when PHASIT_REAL_DATA_REQUIRED=1.
#[test]
fn region_matches_crop_real_mosaic_czi() {
    const SAMPLE: &str = "czi/mosaic.czi";
    let Some(path) = crate::real_data::real_file(SAMPLE) else {
        return;
    };
    let reader = CziReader::open(&path).expect("open real mosaic CZI");
    let dims = reader.meta().scene(0).unwrap().dims;
    let z_mid = dims.z / 2;
    let rects = [
        PixelRect::new(0, 0, dims.x, dims.y),
        PixelRect::new(dims.x / 4, dims.y / 4, dims.x / 2, dims.y / 2),
        PixelRect::new(dims.x.saturating_sub(37), dims.y.saturating_sub(29), 37, 29),
    ];
    let full = reader.plane(0, 0, 0, z_mid).expect("compose real plane");
    for &rect in &rects {
        let via_crop = full.crop(rect).expect("crop real plane");
        let via_region = reader
            .region(0, 0, 0, z_mid, rect)
            .expect("out-of-core region on real file");
        assert_eq!(via_region.shape, [rect.h, rect.w], "real shape for {rect:?}");
        assert_eq!(
            via_region, via_crop,
            "real region() != plane().crop() for rect {rect:?}"
        );
    }
    eprintln!("real-file region parity PASSED for {SAMPLE}");
}
