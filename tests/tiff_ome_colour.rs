// SPDX-License-Identifier: Apache-2.0
//! An OME-TIFF scene takes its colour model from the page of its plane 0: an RGB
//! page is one channel of three components, the model the plain-TIFF path uses.

mod support;

use std::fs::File;
use std::path::Path;

use phasit_imread::tiff_reader::TiffReader;
use phasit_imread::{DType, ImageReader, PlaneBuf};
use support::{scratch_dir, write_rgb, RgbPage};
use tiff::encoder::{colortype, TiffEncoder};
use tiff::tags::Tag;

/// A TIFF whose IFD 0 is a flat 8-bit grey page and IFD 1 a flat RGB page, with
/// the OME-XML description on IFD 0. `support` writes one kind of page only.
fn write_grey_then_rgb(path: &Path, description: &str, grey: u8, rgb: [u8; 3]) {
    let mut encoder = TiffEncoder::new(File::create(path).expect("create the TIFF")).unwrap();
    let mut grey_image = encoder.new_image::<colortype::Gray8>(2, 2).unwrap();
    grey_image
        .encoder()
        .write_tag(Tag::ImageDescription, description)
        .unwrap();
    grey_image.write_data(&[grey; 4]).unwrap();
    let rgb_image = encoder.new_image::<colortype::RGB8>(2, 2).unwrap();
    let samples: Vec<u8> = rgb.iter().copied().cycle().take(12).collect();
    rgb_image.write_data(&samples).unwrap();
}

#[test]
fn an_rgb_ome_tiff_is_one_channel_of_three_components() {
    // SizeC=3 with RGB pages: the three channels are the page's components.
    let dir = scratch_dir("ome_rgb");
    let path = dir.join("rgb.ome.tif");
    let xml = r#"<?xml version="1.0"?>
<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
  <Image Name="rgb"><Pixels DimensionOrder="XYCZT" Type="uint8" SizeX="2" SizeY="2" SizeZ="1" SizeC="3" SizeT="1"/></Image>
</OME>"#;
    write_rgb(
        &path,
        Some(xml),
        &[RgbPage {
            width: 2,
            height: 2,
            rgb: [10, 20, 30],
        }],
    );

    let reader = TiffReader::open(&path).expect("open the RGB OME-TIFF");
    let scene = reader.meta().scene(0).expect("one scene");
    assert_eq!(scene.dims.c, 1);
    assert_eq!(scene.samples, 3);
    assert_eq!(scene.channels.len(), 1);

    let plane = reader.plane(0, 0, 0, 0).expect("read the RGB plane");
    assert_eq!(plane.samples, 3);
    assert_eq!(plane.shape, [2, 2]);
    match plane.data {
        PlaneBuf::U8(values) => assert_eq!(&values[..6], &[10, 20, 30, 10, 20, 30]),
        _ => panic!("the fixture is 8-bit"),
    }
}

#[test]
fn an_rgb_scene_keeps_the_name_of_its_one_channel() {
    // SizeC=1 over an RGB page: the one channel is the <Channel> the XML names.
    let dir = scratch_dir("ome_rgb_channel_name");
    let path = dir.join("rgb_named.ome.tif");
    let xml = r#"<?xml version="1.0"?>
<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
  <Image Name="rgb"><Pixels DimensionOrder="XYCZT" Type="uint8" SizeX="2" SizeY="2" SizeZ="1" SizeC="1" SizeT="1">
    <Channel Name="Colour"/>
  </Pixels></Image>
</OME>"#;
    write_rgb(
        &path,
        Some(xml),
        &[RgbPage {
            width: 2,
            height: 2,
            rgb: [10, 20, 30],
        }],
    );

    let reader = TiffReader::open(&path).expect("open the RGB OME-TIFF");
    let scene = reader.meta().scene(0).expect("one scene");
    assert_eq!(scene.channels.len(), 1);
    assert_eq!(scene.channels[0].name.as_deref(), Some("Colour"));
}

#[test]
fn an_rgb_scene_whose_size_c_names_neither_one_nor_its_samples_is_refused() {
    // SizeC=2 over 3-sample pages: a plane holds all its channels in one page,
    // so the scene states nothing this reader can address.
    let dir = scratch_dir("ome_rgb_sizec");
    let path = dir.join("rgb_sizec.ome.tif");
    let xml = r#"<?xml version="1.0"?>
<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
  <Image Name="rgb"><Pixels DimensionOrder="XYCZT" Type="uint8" SizeX="2" SizeY="2" SizeZ="1" SizeC="2" SizeT="1"/></Image>
</OME>"#;
    write_rgb(
        &path,
        Some(xml),
        &[RgbPage {
            width: 2,
            height: 2,
            rgb: [1, 2, 3],
        }],
    );

    assert!(TiffReader::open(&path).is_err());
}

#[test]
fn a_grey_scene_and_an_rgb_scene_in_one_file_both_read_right() {
    // Scene 0 is a grey page at IFD 0 and scene 1 an RGB page at IFD 1. Each
    // takes its colour model from its own page.
    let dir = scratch_dir("ome_grey_and_rgb");
    let path = dir.join("grey_and_rgb.ome.tif");
    let xml = r#"<?xml version="1.0"?>
<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
  <Image Name="grey"><Pixels DimensionOrder="XYCZT" Type="uint8" SizeX="2" SizeY="2" SizeZ="1" SizeC="1" SizeT="1"><TiffData IFD="0"/></Pixels></Image>
  <Image Name="rgb"><Pixels DimensionOrder="XYCZT" Type="uint8" SizeX="2" SizeY="2" SizeZ="1" SizeC="1" SizeT="1"><TiffData IFD="1"/></Pixels></Image>
</OME>"#;
    write_grey_then_rgb(&path, xml, 40, [10, 20, 30]);

    let reader = phasit_imread::open(&path).expect("open the mixed OME-TIFF");
    let grey = reader.meta().scene(0).expect("the grey scene");
    assert_eq!((grey.samples, grey.dims.c, grey.dtype), (1, 1, DType::U8));
    let rgb = reader.meta().scene(1).expect("the RGB scene");
    assert_eq!((rgb.samples, rgb.dims.c, rgb.dtype), (3, 1, DType::U8));

    let grey_plane = reader.plane(0, 0, 0, 0).expect("read the grey plane");
    assert_eq!(grey_plane.samples, 1);
    match grey_plane.data {
        PlaneBuf::U8(values) => assert_eq!(values[0], 40),
        _ => panic!("the fixture is 8-bit"),
    }
    let rgb_plane = reader.plane(1, 0, 0, 0).expect("read the RGB plane");
    assert_eq!(rgb_plane.samples, 3);
    match rgb_plane.data {
        PlaneBuf::U8(values) => assert_eq!(&values[..3], &[10, 20, 30]),
        _ => panic!("the fixture is 8-bit"),
    }
}
