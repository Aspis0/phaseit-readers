// SPDX-License-Identifier: Apache-2.0
//! A TIFF of striped pages whose pixels vary from pixel to pixel, written
//! with the `tiff` crate's own encoder.
//!
//! WHY the pixels vary: a region read is checked against a whole-plane read,
//! and a file whose rows all carry the same value cannot tell a right window
//! from a wrong one. The encoder writes flat pages, so the pattern is written
//! here, pixel by pixel.
//!
//! The encoder writes the STRIPED layouts only (any sample type it supports,
//! any compression, any rows-per-strip); the layouts it cannot write are in
//! [`super::tiff_layout`].

use std::fs::File;
use std::path::Path;

use tiff::encoder::colortype::{self, ColorType};
use tiff::encoder::{Compression, TiffEncoder, TiffValue};
use tiff::tags::Tag;

use super::tiff_page::{Pattern, SampleType};

/// One page of a striped TIFF file, written with the `tiff` crate's encoder.
pub struct StripedPage {
    pub width: u32,
    pub height: u32,
    /// Samples per pixel: 1 = grey, 3 = RGB.
    pub samples: u16,
    pub sample_type: SampleType,
    /// How many rows each strip holds.
    pub rows_per_strip: u32,
    /// Whether the page states `PlanarConfiguration = 2` (each sample in its
    /// own plane). With one sample per pixel the layout is the same either
    /// way; with more, the file says it cannot be addressed.
    pub planar: bool,
}

/// Write a TIFF of striped pages whose pixels vary from pixel to pixel.
///
/// The description, when given, goes on the first page, where the reader
/// looks for OME-XML.
pub fn write_striped(
    path: &Path,
    description: Option<&str>,
    pages: &[StripedPage],
    compression: Compression,
) {
    let mut encoder = TiffEncoder::new(File::create(path).expect("create the TIFF"))
        .expect("the TIFF encoder")
        .with_compression(compression);
    for (index, page) in pages.iter().enumerate() {
        let tag = if index == 0 { description } else { None };
        match (page.samples, page.sample_type) {
            (1, SampleType::U8) => write_page_with::<colortype::Gray8>(&mut encoder, page, tag),
            (1, SampleType::U16) => write_page_with::<colortype::Gray16>(&mut encoder, page, tag),
            (1, SampleType::I16) => write_page_with::<colortype::GrayI16>(&mut encoder, page, tag),
            (1, SampleType::F32) => {
                write_page_with::<colortype::Gray32Float>(&mut encoder, page, tag)
            }
            (3, SampleType::U8) => write_page_with::<colortype::RGB8>(&mut encoder, page, tag),
            (3, SampleType::U16) => write_page_with::<colortype::RGB16>(&mut encoder, page, tag),
            other => panic!("no TIFF writer for {other:?}"),
        }
    }
}

/// Write one striped page, whose pixels vary from pixel to pixel.
fn write_page_with<C: ColorType>(
    encoder: &mut TiffEncoder<File>,
    page: &StripedPage,
    description: Option<&str>,
) where
    C::Inner: Pattern,
    [C::Inner]: TiffValue,
{
    let mut image = encoder
        .new_image::<C>(page.width, page.height)
        .expect("the page");
    if let Some(text) = description {
        image
            .encoder()
            .write_tag(Tag::ImageDescription, text)
            .expect("the description");
    }
    image
        .rows_per_strip(page.rows_per_strip)
        .expect("the rows per strip");
    if page.planar {
        image
            .encoder()
            .write_tag(Tag::PlanarConfiguration, 2u16)
            .expect("the planar configuration");
    }
    let mut data: Vec<C::Inner> =
        Vec::with_capacity((page.width * page.height * u32::from(page.samples)) as usize);
    for y in 0..page.height {
        for x in 0..page.width {
            for s in 0..page.samples {
                data.push(C::Inner::at(x, y, s));
            }
        }
    }
    image.write_data(&data).expect("the page data");
}
