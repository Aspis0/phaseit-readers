// SPDX-License-Identifier: Apache-2.0
//! A TIFF of striped pages with pixels that vary, written by the `tiff` crate's encoder (which writes flat pages only).
//! WHY the pixels vary: a file whose rows carry one value cannot tell a right window from a wrong one; the layouts the
//! encoder cannot write are in `tiff_layout`.

use std::fs::File;
use std::path::Path;

use tiff::encoder::colortype::{self, ColorType};
use tiff::encoder::{Compression, TiffEncoder, TiffValue};
use tiff::tags::Tag;

use super::tiff_page::{Pattern, SampleType};

pub struct StripedPage {
    pub width: u32,
    pub height: u32,
    pub samples: u16,
    pub sample_type: SampleType,
    pub rows_per_strip: u32,
    /// `PlanarConfiguration = 2`: with more than one sample the file says it cannot be addressed.
    pub planar: bool,
}

/// The description, when given, goes on the first page, where the reader looks for OME-XML.
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
