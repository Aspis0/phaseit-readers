// SPDX-License-Identifier: Apache-2.0
//! Writes the small TIFF files the reader tests read back, with the `tiff`
//! crate's own encoder. Each page is a flat colour, so a test states which IFD
//! holds which value and reads the value back from the plane it addresses.

// Each test crate includes this module and uses only some of its writers.
#![allow(dead_code)]

pub mod czi;
pub mod lif;
pub mod lsm;
pub mod nd2;
pub mod scratch;
pub mod tiff_layout;
pub mod tiff_page;
pub mod tiff_striped;

use std::fs::{self, File};
use std::path::Path;

use tiff::encoder::{colortype, TiffEncoder};
use tiff::tags::Tag;

/// A folder for one test's files, emptied of any earlier run and removed when the guard is dropped.
pub fn scratch_dir(name: &str) -> scratch::ScratchDir {
    let dir = std::env::temp_dir().join(format!("phasit_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create the scratch folder");
    scratch::ScratchDir::new(dir)
}

/// One 8-bit grey page of a single flat value.
pub struct GreyPage {
    pub width: u32,
    pub height: u32,
    pub value: u8,
}

/// One 8-bit RGB page of a single flat colour.
pub struct RgbPage {
    pub width: u32,
    pub height: u32,
    pub rgb: [u8; 3],
}

/// Write a TIFF of RGB pages, one IFD per page, with the description on the first.
pub fn write_rgb(path: &Path, description: Option<&str>, pages: &[RgbPage]) {
    let mut encoder = TiffEncoder::new(File::create(path).expect("create the TIFF")).unwrap();
    for (index, page) in pages.iter().enumerate() {
        let mut image = encoder
            .new_image::<colortype::RGB8>(page.width, page.height)
            .unwrap();
        if let (0, Some(text)) = (index, description) {
            image.encoder().write_tag(Tag::ImageDescription, text).unwrap();
        }
        let samples: Vec<u8> = page
            .rgb
            .iter()
            .copied()
            .cycle()
            .take((page.width * page.height * 3) as usize)
            .collect();
        image.write_data(&samples).unwrap();
    }
}

/// Write a TIFF whose IFD `i` is `pages[i]`. The description, when given, goes on
/// the first page, where the reader looks for OME-XML.
pub fn write_grey(path: &Path, description: Option<&str>, pages: &[GreyPage]) {
    let mut encoder = TiffEncoder::new(File::create(path).expect("create the TIFF")).unwrap();
    for (index, page) in pages.iter().enumerate() {
        let mut image = encoder
            .new_image::<colortype::Gray8>(page.width, page.height)
            .unwrap();
        if let (0, Some(text)) = (index, description) {
            image.encoder().write_tag(Tag::ImageDescription, text).unwrap();
        }
        let samples = vec![page.value; (page.width * page.height) as usize];
        image.write_data(&samples).unwrap();
    }
}
