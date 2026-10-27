// SPDX-License-Identifier: Apache-2.0
//! Real-file bit-exact parity test for the CZI reader on a real Zeiss Axioscan
//! JPEG-XR CZI.
//!
//! The sample is the public Zenodo record 11234863 ("Human DAB staining
//! Axioscan BF 20x", CC-BY-4.0), linked into the folder named by
//! `PHASIT_REAL_DATA_DIR` as `czi/brightfield-jxr.czi` (the layout is in the
//! crate README). It is a whole-slide brightfield mosaic: 1958 full-resolution
//! tiles of 2056x2464 pixels, JPEG-XR compressed, `PixelType = Bgr24`.
//!
//! The reference is the `czifile` ground truth dumped by
//! `tests/parity/dump_ref_czi_jxr.py`: nine 1024x1024 regions of the composed
//! scene — four single-tile interiors, two seams between side-by-side tiles, a
//! corner where four tiles meet, the bottom-right corner of a tile's interior,
//! and a window at the edge of the scene that no tile covers entirely. The
//! dumps live in `tests/parity/_ref/czi_jxr/` (manifest.json + one `.npy` per
//! region); `czi_jxr_ref` reads them.
//!
//! COMPOSITION ORDER: the reference composes the tiles of a region the way
//! `czifile` does — in ascending mosaic index, so the tile with the higher M
//! ends up on top (`czifile.py:1308`, `czifile.py:1309`, applied at
//! `czifile.py:2810` and `czifile.py:2811`). `phasit-imread` composes them in
//! the same order (`src/czi_reader.rs`, `CziReader::open` — the per-plane
//! mosaic sort), so the reader and the ground
//! truth agree on every pixel. The manifest still records the subblock
//! directory order per region beside the mosaic order, and the seam and corner
//! regions are the ones where the two orders disagree — there the composition
//! order decides the bytes, so those regions are the ones that catch a reader
//! that composites in the wrong order.
//!
//! SAMPLE ORDER: the container stores B,G,R (`PixelType = Bgr24`); `czifile`
//! returns the JPEG-XR tiles as R,G,B (it does not swap BGR->RGB for
//! compression 4, because the decoder already returns R,G,B), and that is the
//! order this reader publishes (`czi_reader.rs` swaps B<->R after the codec
//! returns the container's native order). The `.npy` files therefore hold
//! R,G,B.
//!
//! SKIP GUARD: the test skips cleanly when `PHASIT_REAL_DATA_DIR` is unset,
//! when the sample is absent, or when the reference dumps are absent. The
//! guard is the shared real-data gate: `PHASIT_REAL_DATA_REQUIRED=1` turns
//! every such skip into a failure (`tests/real_data/mod.rs`), so a required
//! run cannot pass without the sample and the dumps.

mod czi_jxr_ref;
mod real_data;

use czi_jxr_ref::{manifest, ref_dir, regions};
#[cfg(all(feature = "ndarray", feature = "jpegxr"))]
use czi_jxr_ref::{selected_regions, Region};
use phasit_imread::czi_reader::CziReader;
use phasit_imread::types::PixelRect;
use phasit_imread::{DType, ImageReader};

use real_data::real_file;

/// The public Axioscan sample, relative to `PHASIT_REAL_DATA_DIR`.
const SAMPLE: &str = "czi/brightfield-jxr.czi";

/// The scene geometry the reference script recorded.
#[test]
fn czi_jxr_real_geometry() {
    let Some(path) = real_file(SAMPLE) else {
        return;
    };
    let Some(manifest) = manifest() else {
        return;
    };

    let reader = CziReader::open(&path).expect("open the Axioscan CZI");
    let meta = reader.meta();
    assert_eq!(meta.scene_count(), 1, "the sample holds one scene");
    assert!(
        reader.refused_scenes().is_empty(),
        "no scene of the sample is refused"
    );

    let scene = meta.scene(0).expect("scene 0");
    let dims = &manifest["scene"]["dims"];
    assert_eq!(scene.dims.t, 1, "T");
    assert_eq!(scene.dims.c, 1, "C");
    assert_eq!(scene.dims.z, 1, "Z");
    assert_eq!(
        scene.dims.y,
        dims["y"].as_u64().expect("recorded Y") as usize,
        "Y equals the recorded scene bounding box"
    );
    assert_eq!(
        scene.dims.x,
        dims["x"].as_u64().expect("recorded X") as usize,
        "X equals the recorded scene bounding box"
    );
    assert_eq!(scene.dtype, DType::U8, "dtype");
    assert_eq!(scene.samples, 3, "samples per pixel");

    // The composed plane is far over the decode cap, so only the out-of-core
    // region read can reach the pixels of this whole-slide scene.
    assert!(reader.supports_region(), "the CZI reader streams regions");
}

/// The manifest must name the composition order the reference was MADE with,
/// and every region must say it was composited in that order.
#[test]
fn czi_jxr_real_composition() {
    let Some(manifest) = manifest() else {
        return;
    };

    // The manifest names the composition order the reference was MADE with and
    // the one the reader uses. Both must be czifile's order — ascending mosaic
    // index — and they must be the SAME rule, or the reference does not
    // certify the reader at all.
    //
    // The manifest records the order the dump was actually made with, so a
    // reference produced with `--order subblock_directory_order` says so here
    // and this test goes RED instead of certifying a reference made the other
    // way round.
    let composition = &manifest["composition"];
    let expected_order = "mosaic_index_ascending";
    assert_eq!(
        composition["reference_order"].as_str(),
        Some(expected_order),
        "the reference was composed in czifile's order (a dump made in another \
         order does not certify the reader)"
    );
    assert_eq!(
        composition["reader_order"].as_str(),
        Some(expected_order),
        "the reader composes in czifile's order too: the tile with the higher \
         mosaic index ends up on top"
    );
    assert_eq!(
        composition["reference_order"].as_str(),
        composition["reader_order"].as_str(),
        "the reference and the reader must use the SAME composition order"
    );
    assert_eq!(
        composition["fill_value"]["czifile"].as_u64(),
        Some(0),
        "czifile fills the pixels no tile covers with zero"
    );
    assert_eq!(
        composition["fill_value"]["phasit_imread"].as_u64(),
        Some(0),
        "the reader fills the pixels no tile covers with zero"
    );
}

/// Every region must say which order it was composited in, and it must be the
/// order the manifest declares.
#[test]
fn czi_jxr_real_region_orders() {
    let Some(manifest) = manifest() else {
        return;
    };

    // The order comes from the manifest, not from a literal here: the manifest
    // names the order the reference was MADE with, and every region has to
    // agree with it. A literal in the test would go stale the moment the
    // manifest changes — and it would not notice a reference made in another
    // order.
    let declared = manifest["composition"]["reference_order"]
        .as_str()
        .expect("the manifest declares the composition order");
    for region in &regions(&manifest) {
        assert_eq!(
            region.composition_order, declared,
            "region {} was composited in another order than the manifest declares",
            region.name
        );
    }
}

/// The reference set must be complete, and it must cover the mosaic.
#[test]
fn czi_jxr_real_reference_set() {
    let Some(manifest) = manifest() else {
        return;
    };

    // The reference set is complete, and it covers the mosaic: an overlap, a
    // region the tiles do not cover entirely, and a region where the two
    // composition orders disagree.
    let all = regions(&manifest);
    eprintln!(
        "the reference set holds {} regions: {}",
        all.len(),
        all.iter()
            .map(|region| region.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    for region in &all {
        assert!(
            ref_dir().join(&region.npy_rel).exists(),
            "partial reference set: {} is missing (broken reference set)",
            region.npy_rel
        );
    }
    assert!(
        all.iter().any(|region| region.tile_count >= 2),
        "the reference covers a region where two tiles overlap"
    );
    assert!(
        all.iter()
            .any(|region| region.covered_pixels < region.w * region.h),
        "the reference covers a region that no tile covers entirely"
    );
    assert!(
        all.iter().any(|region| !region.orders_agree),
        "the reference covers a region where the two composition orders disagree"
    );
}

/// Every reference region must be bit-exact against the `czifile` dump.
///
/// Every region is compared and EVERY difference is reported before the test
/// fails: the first mismatch does not hide the ones after it.
#[cfg(all(feature = "ndarray", feature = "jpegxr"))]
#[test]
fn czi_jxr_real_regions_bit_exact() {
    let Some(path) = real_file(SAMPLE) else {
        return;
    };
    let Some(manifest) = manifest() else {
        return;
    };

    let reader = CziReader::open(&path).expect("open the Axioscan CZI");
    let selected = selected_regions(&manifest);
    assert!(
        !selected.is_empty(),
        "the reference holds regions to compare"
    );

    let mut differences = Vec::new();
    for region in &selected {
        match compare_region(&reader, region) {
            Ok(summary) => eprintln!("region {}: {summary}", region.name),
            Err(detail) => {
                eprintln!("region {}: {detail}", region.name);
                differences.push(format!("{}: {detail}", region.name));
            }
        }
    }

    assert!(
        differences.is_empty(),
        "{} of {} regions are not bit-exact vs the czifile reference:\n  {}",
        differences.len(),
        selected.len(),
        differences.join("\n  ")
    );
}

/// Compare one region with the reference and describe the difference, if any.
///
/// Every failure is a `Result`, never a panic: the caller compares every
/// region and reports them all, so one broken region does not hide the ones
/// after it.
#[cfg(all(feature = "ndarray", feature = "jpegxr"))]
fn compare_region(reader: &CziReader, region: &Region) -> Result<String, String> {
    use ndarray::Array3;
    use phasit_imread::PlaneBuf;

    let expected: Array3<u8> = match ndarray_npy::read_npy(ref_dir().join(&region.npy_rel)) {
        Ok(array) => array,
        Err(e) => return Err(format!("read the reference .npy: {e}")),
    };
    if [
        expected.shape()[0],
        expected.shape()[1],
        expected.shape()[2],
    ] != [region.h, region.w, 3]
    {
        return Err(format!(
            "the reference is {:?} but the region is {}x{}x3",
            expected.shape(),
            region.w,
            region.h
        ));
    }

    let plane = match reader.region(
        0,
        0,
        0,
        0,
        PixelRect::new(region.x, region.y, region.w, region.h),
    ) {
        Ok(plane) => plane,
        Err(e) => return Err(format!("decode the region: {e}")),
    };
    if plane.shape != [region.h, region.w] {
        return Err(format!(
            "the decoded region is {:?} but the reference is {}x{}",
            plane.shape, region.w, region.h
        ));
    }
    if plane.samples != 3 {
        return Err(format!(
            "the decoded region holds {} samples, the reference holds 3",
            plane.samples
        ));
    }
    let ours = match &plane.data {
        PlaneBuf::U8(v) => v,
        other => return Err(format!("expected U8 data, got {:?}", other.dtype())),
    };
    let reference: Vec<u8> = expected.iter().copied().collect();
    if ours.len() != reference.len() {
        return Err(format!(
            "the decoded region holds {} bytes, the reference holds {}",
            ours.len(),
            reference.len()
        ));
    }

    match ours.iter().zip(reference.iter()).position(|(a, b)| a != b) {
        None => Ok(format!(
            "equal ({} bytes, {}x{})",
            ours.len(),
            region.w,
            region.h
        )),
        Some(first) => {
            let differing = ours
                .iter()
                .zip(reference.iter())
                .filter(|(a, b)| a != b)
                .count();
            Err(format!(
                "NOT bit-exact vs the czifile reference: first differing byte at index \
                 {first} (ours {}, reference {}), {differing} of {} bytes differ",
                ours[first],
                reference[first],
                ours.len()
            ))
        }
    }
}

/// Without the `ndarray` feature there is no `.npy` reader, so that build
/// cannot compare the reference bytes. It checks the properties that need no
/// reference: that a JPEG-XR subblock really decodes, that the region has the
/// size the manifest records, and that its pixels are image content and not a
/// constant fill.
#[cfg(all(feature = "jpegxr", not(feature = "ndarray")))]
#[test]
fn czi_jxr_real_region_decodes_without_ndarray() {
    use phasit_imread::PlaneBuf;

    let Some(path) = real_file(SAMPLE) else {
        return;
    };
    let Some(manifest) = manifest() else {
        return;
    };

    let all = regions(&manifest);
    let region = all
        .iter()
        .find(|region| region.tile_count == 1)
        .expect("the reference holds a single-tile region");
    let reader = CziReader::open(&path).expect("open the Axioscan CZI");
    let plane = reader
        .region(
            0,
            0,
            0,
            0,
            PixelRect::new(region.x, region.y, region.w, region.h),
        )
        .unwrap_or_else(|e| panic!("decode region {}: {e}", region.name));
    assert_eq!(
        plane.shape,
        [region.h, region.w],
        "shape of {}",
        region.name
    );
    assert_eq!(plane.samples, 3, "samples of {}", region.name);
    let data = match &plane.data {
        PlaneBuf::U8(v) => v,
        other => panic!(
            "expected U8 data for {}, got {:?}",
            region.name,
            other.dtype()
        ),
    };
    assert_eq!(
        data.len(),
        region.h * region.w * 3,
        "byte length of {}",
        region.name
    );
    // The reader fills the pixels no tile covers with zero, so any non-zero
    // byte is a pixel a JPEG-XR subblock decoded.
    assert!(
        data.iter().any(|byte| *byte != 0),
        "region {} holds no decoded pixel",
        region.name
    );
    let distinct: std::collections::BTreeSet<u8> = data.iter().copied().collect();
    assert!(
        distinct.len() > 1,
        "region {} is a constant fill, not image content",
        region.name
    );
    eprintln!(
        "region {}: {} bytes, {} distinct values, decoded without ndarray",
        region.name,
        data.len(),
        distinct.len()
    );
}

/// Without the `jpegxr` feature the file still opens, but decoding a
/// JPEG-XR subblock is refused with the documented `Unsupported` error.
#[cfg(not(feature = "jpegxr"))]
#[test]
fn czi_jxr_real_unsupported_without_jpegxr() {
    use phasit_imread::Error;

    let Some(path) = real_file(SAMPLE) else {
        return;
    };
    let Some(manifest) = manifest() else {
        return;
    };

    let reader = CziReader::open(&path).expect("the container opens without the codec");
    let region = regions(&manifest)
        .into_iter()
        .next()
        .expect("the manifest holds a region");
    let rect = PixelRect::new(region.x, region.y, region.w, region.h);

    match reader.region(0, 0, 0, 0, rect) {
        Err(Error::Unsupported(message)) => {
            assert!(
                message.contains("jpegxr"),
                "the refusal names the missing feature: {message}"
            );
            eprintln!("without the jpegxr feature: {message}");
        }
        other => panic!("expected Unsupported without the jpegxr feature, got {other:?}"),
    }
    // The whole scene is 99943x148423x3 = 44.5 GB, far over the decoded-plane
    // cap, so the whole-plane read is refused before the codec is even asked.
    match reader.plane(0, 0, 0, 0) {
        Err(Error::OversizedAllocation { requested, cap }) => {
            assert!(
                requested > cap,
                "the refusal names the size that was refused"
            );
            eprintln!("the whole scene is refused: {requested} bytes over a {cap} cap");
        }
        other => panic!("expected the whole plane to be refused, got {other:?}"),
    }
}

/// TIMING PROBE — not a correctness test. It measures how long the reader
/// needs to open the whole-slide CZI and to read the reference regions out of
/// it. Ignored by default; run it explicitly with
/// `cargo test --release --features jpegxr,ndarray --test czi_jxr_real -- --ignored`.
///
/// On MSVC the release build of the vendored jxrlib needs the C flags
/// `-D_byteswap_ulong=phasit_byteswap_ulong`: with /O2 (which implies /Oi)
/// MSVC refuses the vendored fallback definition of `_byteswap_ulong` with
/// C2169, because that name is an intrinsic. The debug build is unaffected.
#[cfg(all(feature = "ndarray", feature = "jpegxr"))]
#[test]
#[ignore = "timing probe, not a correctness test"]
fn czi_jxr_real_timing_probe() {
    let Some(path) = real_file(SAMPLE) else {
        return;
    };
    let Some(manifest) = manifest() else {
        return;
    };

    let opened = std::time::Instant::now();
    let reader = CziReader::open(&path).expect("open the Axioscan CZI");
    let open_secs = opened.elapsed().as_secs_f64();

    let mut read_secs = 0.0;
    for region in regions(&manifest) {
        let started = std::time::Instant::now();
        let plane = reader
            .region(
                0,
                0,
                0,
                0,
                PixelRect::new(region.x, region.y, region.w, region.h),
            )
            .expect("decode the region");
        read_secs += started.elapsed().as_secs_f64();
        eprintln!(
            "region {}: {} bytes in {:.3} s",
            region.name,
            plane.data.len(),
            started.elapsed().as_secs_f64()
        );
    }
    eprintln!("timing: open {open_secs:.3} s, region reads {read_secs:.3} s");
}
