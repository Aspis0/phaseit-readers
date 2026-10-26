// SPDX-License-Identifier: Apache-2.0
//! A plain TIFF's colour model comes from its PhotometricInterpretation. A
//! ColorMap on a grey page is not an RGBPalette, so it is not published.

mod support;

use std::fs::File;

use phasit_imread::tiff_reader::TiffReader;
use phasit_imread::{ImageReader, PlaneBuf};
use support::scratch_dir;
use tiff::encoder::{colortype, TiffEncoder};
use tiff::tags::Tag;

#[test]
fn a_colormap_on_a_grey_page_is_not_published_and_its_values_are_kept() {
    // ImageJ writes a ColorMap for its LUT on a grey page. The page's samples are
    // its values, unchanged, and the LUT is not in the scene.
    let dir = scratch_dir("plain_colormap");
    let path = dir.join("lut.tif");
    {
        let mut encoder = TiffEncoder::new(File::create(&path).expect("create the TIFF")).unwrap();
        let mut image = encoder.new_image::<colortype::Gray8>(2, 2).unwrap();
        let lut = vec![0x0100u16; 3 * 256];
        image.encoder().write_tag(Tag::ColorMap, &lut[..]).unwrap();
        image.write_data(&[1u8, 2, 3, 4]).unwrap();
    }

    let reader = TiffReader::open(&path).expect("open the grey TIFF");
    let scene = reader.meta().scene(0).expect("one scene");
    assert_eq!(scene.samples, 1);
    assert!(scene.palette.is_none());
    match reader.plane(0, 0, 0, 0).expect("read the plane").data {
        PlaneBuf::U8(values) => assert_eq!(values, vec![1, 2, 3, 4]),
        _ => panic!("the fixture is 8-bit"),
    }
}
