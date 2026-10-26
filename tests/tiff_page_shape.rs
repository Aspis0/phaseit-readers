// SPDX-License-Identifier: Apache-2.0
//! Every page a plane decodes must have the shape its scene states. A page of
//! the same pixel count but swapped sides is refused by name, not transposed.

mod support;

use phasit_imread::tiff_reader::TiffReader;
use phasit_imread::ImageReader;
use support::{scratch_dir, write_grey, GreyPage};

/// Page 0 is 2 wide and 3 high; page 1 has the same 6 pixels, 3 wide and 2 high.
fn swapped_pages() -> [GreyPage; 2] {
    [
        GreyPage {
            width: 2,
            height: 3,
            value: 1,
        },
        GreyPage {
            width: 3,
            height: 2,
            value: 2,
        },
    ]
}

#[test]
fn an_ome_page_of_swapped_sides_is_refused_by_name() {
    let dir = scratch_dir("page_shape_ome");
    let path = dir.join("swapped.ome.tif");
    let xml = r#"<?xml version="1.0"?>
<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
  <Image Name="stack"><Pixels DimensionOrder="XYCZT" Type="uint8" SizeX="2" SizeY="3" SizeZ="2" SizeC="1" SizeT="1"/></Image>
</OME>"#;
    write_grey(&path, Some(xml), &swapped_pages());

    let reader = TiffReader::open(&path).expect("open the OME-TIFF");
    assert!(reader.plane(0, 0, 0, 0).is_ok());
    let message = match reader.plane(0, 0, 0, 1) {
        Ok(_) => panic!("the swapped page must not be read"),
        Err(error) => error.to_string(),
    };
    assert!(message.contains("IFD 1"), "{message}");
    assert!(message.contains("3x2"), "{message}");
}

#[test]
fn a_plain_tiff_page_of_swapped_sides_is_refused_by_name() {
    let dir = scratch_dir("page_shape_plain");
    let path = dir.join("swapped.tif");
    write_grey(&path, None, &swapped_pages());

    let reader = TiffReader::open(&path).expect("open the plain TIFF");
    assert!(reader.plane(0, 0, 0, 0).is_ok());
    let message = match reader.plane(0, 0, 0, 1) {
        Ok(_) => panic!("the swapped page must not be read"),
        Err(error) => error.to_string(),
    };
    assert!(message.contains("IFD 1"), "{message}");
}
