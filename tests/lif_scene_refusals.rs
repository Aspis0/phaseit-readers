// SPDX-License-Identifier: Apache-2.0
//! What the LIF reader says about the images it could not read.
//!
//! The container is written by the shared writer in `tests/support/lif.rs`
//! (the reader's module docs spell the framing out): a header, the UTF-16LE
//! metadata XML, then one memory block per image whose pixels live in this
//! file. An image whose data block is absent — the ext-resident case, whose
//! pixels live in the `.lifext` companion — cannot be decoded.
//!
//! A reader that drops an image without saying so renumbers every image after
//! it: the caller asking for image N of the file then gets a different one. So
//! a refused image must be NAMED, and the images around it must keep their own
//! place in the file.

mod support;

use phasit_imread::lif_reader::LifReader;
use phasit_imread::{Error, ImageReader};
use support::lif::{write_lif, LifImage};
use support::scratch::lif_scratch;

/// A 4x2 grayscale image of the fixture, whose pixels live in this file
/// (`resident`) or in the `.lifext` companion (not `resident`).
fn gray(name: &str, block_id: &str, resident: bool) -> LifImage {
    let image = LifImage::gray8(name, block_id, 4, 2, None, None, 1);
    if resident {
        image
    } else {
        image.ext_resident()
    }
}

/// A 4x2 grayscale image whose `<Dimensions>`/`<Channels>` description is
/// stated verbatim — the malformed, ambiguous and unmodelled cases.
fn with_body(name: &str, block_id: &str, body: &str) -> LifImage {
    LifImage::gray8(name, block_id, 4, 2, None, None, 1).with_body(body)
}

#[test]
fn an_ext_resident_image_is_refused_with_its_reason() {
    let path = lif_scratch("ext_resident");
    // The pixels of the second image live in the `.lifext` companion next to
    // the `.lif`, so this `.lif` has no block for it.
    let companion = path.with_extension("lifext");
    std::fs::write(&companion, b"").expect("write the companion");
    write_lif(
        &path,
        &[
            gray("Image 0", "0", true),
            gray("Image 1", "ext", false),
            gray("Image 2", "2", true),
        ],
    );

    let reader = LifReader::open(&path).expect("the file opens");
    assert_eq!(reader.meta().scene_count(), 2, "the two resident images are published");

    let refused = reader.refused_scenes();
    assert_eq!(refused.len(), 1, "the ext-resident image is refused");
    assert_eq!(refused[0].image_index, 1, "it is the file's image 1");
    assert_eq!(refused[0].name, "Image 1", "the refusal names the image");
    assert!(
        refused[0].reason.contains("ext-resident"),
        "the reason names the cause: {}",
        refused[0].reason
    );
}

#[test]
fn an_image_whose_block_is_missing_without_a_companion_is_not_called_ext_resident() {
    let path = lif_scratch("missing_block");
    write_lif(
        &path,
        &[
            gray("Image 0", "0", true),
            // No block for it in this `.lif`, and no `.lifext` next to it
            // either: the file is short, not ext-resident.
            gray("Image 1", "ext", false),
        ],
    );

    let reader = LifReader::open(&path).expect("the file opens");
    let refused = reader.refused_scenes();
    assert_eq!(refused.len(), 1, "the image without a block is refused");
    assert!(
        !refused[0].reason.contains("ext-resident"),
        "the reason does not blame the companion: {}",
        refused[0].reason
    );
    assert!(
        refused[0].reason.contains("absent"),
        "the reason names the cause: {}",
        refused[0].reason
    );
}

#[test]
fn a_refused_image_does_not_renumber_the_images_after_it() {
    let path = lif_scratch("positions");
    write_lif(
        &path,
        &[
            gray("Image 0", "0", true),
            gray("Image 1", "ext", false),
            gray("Image 2", "2", true),
        ],
    );

    let reader = LifReader::open(&path).expect("the file opens");
    // The published images are the file's images 0 and 2: the middle one was
    // refused, and the last image must still say it is image 2 of the file.
    assert_eq!(reader.scene_file_index(0), 0);
    assert_eq!(reader.scene_file_index(1), 2);
}

#[test]
fn a_file_without_a_refused_image_refuses_nothing() {
    let path = lif_scratch("clean");
    write_lif(
        &path,
        &[
            gray("Image 0", "0", true),
            gray("Image 1", "1", true),
        ],
    );

    let reader = LifReader::open(&path).expect("the file opens");
    assert_eq!(reader.meta().scene_count(), 2);
    assert!(reader.refused_scenes().is_empty(), "nothing was refused");
    assert_eq!(reader.scene_file_index(0), 0);
    assert_eq!(reader.scene_file_index(1), 1);
}

#[test]
fn a_file_whose_images_are_all_ext_resident_fails_to_open() {
    let path = lif_scratch("all_ext");
    write_lif(
        &path,
        &[
            gray("Image 0", "ext", false),
            gray("Image 1", "ext2", false),
        ],
    );

    match LifReader::open(&path) {
        Err(Error::Unsupported(message)) => {
            assert!(
                message.contains("no decodable images"),
                "the error names the cause: {message}"
            );
        }
        Err(other) => panic!("the file must not open, got {other}"),
        Ok(_) => panic!("the file must not open"),
    }
}

#[test]
fn an_image_with_an_unmodelled_dimension_is_refused_with_its_reason() {
    let path = lif_scratch("unmodelled_dim");
    write_lif(
        &path,
        &[
            with_body(
                "Image 0",
                "0",
                r#"
          <Dimensions>
            <DimensionDescription DimID="1" NumberOfElements="4" BytesInc="1" Length="0.000004"/>
            <DimensionDescription DimID="2" NumberOfElements="2" BytesInc="4" Length="0.000002"/>
            <DimensionDescription DimID="7" NumberOfElements="3" BytesInc="1" Length="0.000001"/>
          </Dimensions>
          <Channels>
            <ChannelDescription DataType="0" Resolution="8" BytesInc="0" LUTName=""/>
          </Channels>"#,
            ),
            gray("Image 1", "1", true),
        ],
    );

    let reader = LifReader::open(&path).expect("the file opens");
    assert_eq!(reader.meta().scene_count(), 1, "the other image is published");
    let refused = reader.refused_scenes();
    assert_eq!(refused.len(), 1, "the unmodelled dimension is refused");
    assert_eq!(refused[0].image_index, 0);
    assert!(
        refused[0].reason.contains("DimID 7"),
        "the reason names the dimension: {}",
        refused[0].reason
    );
}

#[test]
fn an_image_with_an_unsupported_pixel_type_is_refused_with_its_reason() {
    let path = lif_scratch("unsupported_dtype");
    write_lif(
        &path,
        &[
            with_body(
                "Image 0",
                "0",
                r#"
          <Dimensions>
            <DimensionDescription DimID="1" NumberOfElements="4" BytesInc="1" Length="0.000004"/>
            <DimensionDescription DimID="2" NumberOfElements="2" BytesInc="4" Length="0.000002"/>
          </Dimensions>
          <Channels>
            <ChannelDescription DataType="0" Resolution="32" BytesInc="0" LUTName=""/>
          </Channels>"#,
            ),
            gray("Image 1", "1", true),
        ],
    );

    let reader = LifReader::open(&path).expect("the file opens");
    assert_eq!(reader.meta().scene_count(), 1, "the other image is published");
    let refused = reader.refused_scenes();
    assert_eq!(refused.len(), 1, "the unsupported pixel type is refused");
    assert!(
        refused[0].reason.contains("pixel type"),
        "the reason names the cause: {}",
        refused[0].reason
    );
}

#[test]
fn an_image_whose_x_dimension_is_not_contiguous_is_refused_with_its_reason() {
    let path = lif_scratch("non_contiguous_x");
    write_lif(
        &path,
        &[
            with_body(
                "Image 0",
                "0",
                r#"
          <Dimensions>
            <DimensionDescription DimID="1" NumberOfElements="4" BytesInc="2" Length="0.000004"/>
            <DimensionDescription DimID="2" NumberOfElements="2" BytesInc="4" Length="0.000002"/>
          </Dimensions>
          <Channels>
            <ChannelDescription DataType="0" Resolution="8" BytesInc="0" LUTName=""/>
          </Channels>"#,
            ),
            gray("Image 1", "1", true),
        ],
    );

    let reader = LifReader::open(&path).expect("the file opens");
    assert_eq!(reader.meta().scene_count(), 1, "the other image is published");
    let refused = reader.refused_scenes();
    assert_eq!(refused.len(), 1, "the non-contiguous image is refused");
    assert!(
        refused[0].reason.contains("X dimension"),
        "the reason names the cause: {}",
        refused[0].reason
    );
}

#[test]
fn an_image_whose_dimension_declares_no_elements_is_refused_with_its_reason() {
    let path = lif_scratch("no_elements");
    write_lif(
        &path,
        &[
            with_body(
                "Image 0",
                "0",
                r#"
          <Dimensions>
            <DimensionDescription DimID="1" NumberOfElements="0" BytesInc="1" Length="0.000004"/>
            <DimensionDescription DimID="2" NumberOfElements="2" BytesInc="4" Length="0.000002"/>
          </Dimensions>
          <Channels>
            <ChannelDescription DataType="0" Resolution="8" BytesInc="0" LUTName=""/>
          </Channels>"#,
            ),
            gray("Image 1", "1", true),
        ],
    );

    let reader = LifReader::open(&path).expect("the file opens");
    assert_eq!(reader.meta().scene_count(), 1, "the other image is published");
    let refused = reader.refused_scenes();
    assert_eq!(refused.len(), 1, "the empty dimension is refused");
    assert!(
        refused[0].reason.contains("0 elements"),
        "the reason names the cause: {}",
        refused[0].reason
    );
}

#[test]
fn an_image_with_a_duplicate_dimension_is_refused_with_its_reason() {
    let path = lif_scratch("duplicate_dim");
    write_lif(
        &path,
        &[
            with_body(
                "Image 0",
                "0",
                r#"
          <Dimensions>
            <DimensionDescription DimID="1" NumberOfElements="4" BytesInc="1" Length="0.000004"/>
            <DimensionDescription DimID="1" NumberOfElements="4" BytesInc="1" Length="0.000004"/>
            <DimensionDescription DimID="2" NumberOfElements="2" BytesInc="4" Length="0.000002"/>
          </Dimensions>
          <Channels>
            <ChannelDescription DataType="0" Resolution="8" BytesInc="0" LUTName=""/>
          </Channels>"#,
            ),
            gray("Image 1", "1", true),
        ],
    );

    let reader = LifReader::open(&path).expect("the file opens");
    assert_eq!(reader.meta().scene_count(), 1, "the other image is published");
    let refused = reader.refused_scenes();
    assert_eq!(refused.len(), 1, "the duplicate dimension is refused");
    assert!(
        refused[0].reason.contains("duplicate"),
        "the reason names the cause: {}",
        refused[0].reason
    );
}

#[test]
fn an_image_with_a_malformed_numeric_attribute_is_refused_with_its_reason() {
    let path = lif_scratch("malformed_attribute");
    write_lif(
        &path,
        &[
            with_body(
                "Image 0",
                "0",
                r#"
          <Dimensions>
            <DimensionDescription DimID="1" NumberOfElements="four" BytesInc="1" Length="0.000004"/>
            <DimensionDescription DimID="2" NumberOfElements="2" BytesInc="4" Length="0.000002"/>
          </Dimensions>
          <Channels>
            <ChannelDescription DataType="0" Resolution="8" BytesInc="0" LUTName=""/>
          </Channels>"#,
            ),
            gray("Image 1", "1", true),
        ],
    );

    let reader = LifReader::open(&path).expect("the file opens");
    assert_eq!(reader.meta().scene_count(), 1, "the other image is published");
    let refused = reader.refused_scenes();
    assert_eq!(refused.len(), 1, "the malformed attribute is refused");
    assert!(
        refused[0].reason.contains("malformed"),
        "the reason names the cause: {}",
        refused[0].reason
    );
}

#[test]
fn an_image_whose_data_block_does_not_hold_its_frame_is_refused_with_its_reason() {
    let path = lif_scratch("short_block");
    write_lif(
        &path,
        &[
            // The frame is 8x2 gray pixels, but the block holds 4x2: the span
            // of the image runs past its own block.
            with_body(
                "Image 0",
                "0",
                r#"
          <Dimensions>
            <DimensionDescription DimID="1" NumberOfElements="8" BytesInc="1" Length="0.000004"/>
            <DimensionDescription DimID="2" NumberOfElements="2" BytesInc="8" Length="0.000002"/>
          </Dimensions>
          <Channels>
            <ChannelDescription DataType="0" Resolution="8" BytesInc="0" LUTName=""/>
          </Channels>"#,
            ),
            gray("Image 1", "1", true),
        ],
    );

    let reader = LifReader::open(&path).expect("the file opens");
    assert_eq!(reader.meta().scene_count(), 1, "the other image is published");
    let refused = reader.refused_scenes();
    assert_eq!(refused.len(), 1, "the image that overruns its block is refused");
    assert!(
        refused[0].reason.contains("data span"),
        "the reason names the cause: {}",
        refused[0].reason
    );
}

/// An image that states more elements than the reader's decode ceiling: the
/// file says 4294967297 gray pixels in a row, which is one byte more than the
/// 4 GiB cap.
///
/// The CEILING does not refuse this image: it guards one materialisation, not
/// the image's existence, so an image this big would be published and read one
/// region at a time (see `tests/plane_ceiling.rs`). What refuses it here is
/// the FILE: its memory block holds 8 bytes, not the 4 GiB the image states,
/// so the bytes are simply not there.
const OVERSIZED_BODY: &str = r#"
          <Dimensions>
            <DimensionDescription DimID="1" NumberOfElements="4294967297" BytesInc="1" Length="0.000004"/>
            <DimensionDescription DimID="2" NumberOfElements="1" BytesInc="4294967297" Length="0.000002"/>
          </Dimensions>
          <Channels>
            <ChannelDescription DataType="0" Resolution="8" BytesInc="0" LUTName=""/>
          </Channels>"#;

#[test]
fn an_image_whose_plane_is_over_the_ceiling_is_not_refused_by_the_ceiling() {
    let path = lif_scratch("oversized_image");
    write_lif(
        &path,
        &[
            with_body("Image 0", "0", OVERSIZED_BODY),
            gray("Image 1", "1", true),
        ],
    );

    let reader = LifReader::open(&path).expect("the file opens");
    assert_eq!(reader.meta().scene_count(), 1, "the other image is published");
    let refused = reader.refused_scenes();
    assert_eq!(refused.len(), 1, "the oversized image is refused");
    assert_eq!(refused[0].image_index, 0, "it is the file's image 0");
    // The reason names the MISSING BYTES, not the ceiling: the ceiling no
    // longer refuses an image, so a refusal that still spoke of the cap would
    // mean the old behaviour is back. On a 32-bit target the element count
    // does not fit at all and is refused earlier.
    assert!(
        refused[0].reason.contains("data span")
            || refused[0].reason.contains("does not fit the reader"),
        "the reason names the missing bytes: {}",
        refused[0].reason
    );
    assert!(
        !refused[0].reason.contains("cap"),
        "the reason does not blame the decode ceiling: {}",
        refused[0].reason
    );
}

#[test]
fn a_file_whose_only_image_overflows_fails_with_the_reason_of_that_image() {
    let path = lif_scratch("oversized_only_image");
    write_lif(&path, &[with_body("Image 0", "0", OVERSIZED_BODY)]);

    match LifReader::open(&path) {
        Err(Error::Unsupported(message)) => {
            assert!(
                message.contains("no decodable images"),
                "the error says why the file cannot be read: {message}"
            );
            assert!(
                message.contains("data span") || message.contains("does not fit the reader"),
                "the error names the missing bytes: {message}"
            );
        }
        Err(other) => panic!("the file must not open, got {other}"),
        Ok(_) => panic!("the file must not open"),
    }
}
