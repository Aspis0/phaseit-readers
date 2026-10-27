// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::error::Error;
use crate::tiff_tags;
use crate::types::{DType, Dims5};

const ONE_IMAGE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
  <Image Name="first">
    <Pixels ID="Pixels:0" DimensionOrder="XYCZT" Type="uint8" SizeX="4" SizeY="3" SizeZ="2" SizeC="2" SizeT="1" PhysicalSizeX="0.5" PhysicalSizeXUnit="um" PhysicalSizeY="0.5" PhysicalSizeYUnit="um">
      <Channel ID="Channel:0:0" Name="DAPI"/>
      <Channel ID="Channel:0:1" Name="GFP" Color="4294901760"/>
    </Pixels>
  </Image>
</OME>"#;

const TWO_IMAGES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
  <Image Name="first">
    <Pixels ID="Pixels:0" DimensionOrder="XYCZT" Type="uint16" SizeX="4" SizeY="3" SizeZ="2" SizeC="1" SizeT="1">
      <TiffData IFD="0" PlaneCount="2"/>
    </Pixels>
  </Image>
  <Image Name="second">
    <Pixels ID="Pixels:1" DimensionOrder="XYCZT" Type="uint8" SizeX="2" SizeY="2" SizeZ="1" SizeC="1" SizeT="1">
      <TiffData IFD="2"/>
    </Pixels>
  </Image>
</OME>"#;

/// The facts of a page of the given size and type, with one grey sample.
fn grey(width: usize, height: usize, dtype: DType) -> PageFacts {
    PageFacts {
        width,
        height,
        dtype,
        samples: 1,
        photometric: tiff_tags::PHOTOMETRIC_BLACK_IS_ZERO,
    }
}

/// Every IFD holds an 8-bit grey page of `width` x `height`.
fn every_page(width: usize, height: usize) -> impl FnMut(usize) -> Result<PageFacts> {
    move |_| Ok(grey(width, height, DType::U8))
}

/// A file of four pages: enough for every plane the fixtures below state.
const FILE_IFDS: usize = 4;

fn read(xml: &str) -> OmeScenes {
    parse_scenes(xml, FILE_IFDS, every_page(4, 3)).expect("parse")
}

#[test]
fn an_unparseable_size_refuses_its_scene_and_an_absent_one_is_one() {
    // SizeZ="-1" does not parse, so its scene is refused by name. The scene
    // with no SizeZ at all is one plane deep.
    let xml = r#"<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
      <Image Name="absent"><Pixels DimensionOrder="XYCZT" Type="uint8"
             SizeX="4" SizeY="3" SizeC="1" SizeT="1"/></Image>
      <Image Name="negative"><Pixels DimensionOrder="XYCZT" Type="uint8"
             SizeX="4" SizeY="3" SizeZ="-1" SizeC="1" SizeT="1"/></Image>
      </OME>"#;
    let read = parse_scenes(xml, FILE_IFDS, every_page(4, 3)).expect("parse");
    assert_eq!(read.scenes.len(), 1);
    assert_eq!(read.scenes[0].meta.dims.z, 1);
    assert_eq!(read.refused.len(), 1);
    assert_eq!(read.refused[0].name, "negative");
    assert!(read.refused[0].reason.contains("SizeZ"), "{:?}", read.refused);
}

#[test]
fn one_image_is_one_scene() {
    let read = read(ONE_IMAGE);
    assert_eq!(read.scenes.len(), 1);
    let scene = &read.scenes[0];
    assert_eq!(scene.meta.name, "first");
    assert_eq!(scene.meta.dims, Dims5 { t: 1, c: 2, z: 2, y: 3, x: 4 });
    assert_eq!(scene.meta.dtype, DType::U8);
    assert_eq!(scene.meta.samples, 1);
    assert_eq!(scene.meta.channels.len(), 2);
    assert_eq!(scene.meta.channels[0].name.as_deref(), Some("DAPI"));
    assert_eq!(scene.plan.ifds, vec![0, 1, 2, 3]);
    assert_eq!(scene.meta.px_size_um.unwrap().x, 0.5);
    assert!(read.refused.is_empty());
}

#[test]
fn two_images_are_two_scenes_in_file_order() {
    // IFD 2 is the second scene's page: 2x2 and 8-bit, unlike the first scene's.
    let pages = |ifd: usize| {
        Ok(if ifd == 2 {
            grey(2, 2, DType::U8)
        } else {
            grey(4, 3, DType::U16)
        })
    };
    let read = parse_scenes(TWO_IMAGES, FILE_IFDS, pages).expect("parse");
    assert_eq!(read.scenes.len(), 2);
    assert_eq!(read.scenes[0].meta.name, "first");
    assert_eq!(read.scenes[1].meta.name, "second");
    assert_eq!(read.scenes[0].meta.dims.x, 4);
    assert_eq!(read.scenes[1].meta.dims.x, 2);
    // Each scene's planes are the IFDs its own TiffData names.
    assert_eq!(read.scenes[0].plan.ifds, vec![0, 1]);
    assert_eq!(read.scenes[1].plan.ifds, vec![2]);
}

#[test]
fn a_description_without_an_image_is_refused() {
    assert!(parse_scenes("<OME/>", FILE_IFDS, every_page(4, 3)).is_err());
}

#[test]
fn the_colour_model_is_read_from_plane_zero_not_the_first_listed_block() {
    // The first <TiffData> is plane 1, at IFD 0, an RGB page. Plane 0 is IFD 1,
    // a grey page, so the scene is grey.
    let xml = r#"<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
      <Image><Pixels DimensionOrder="XYCZT" Type="uint8" SizeX="4" SizeY="3" SizeZ="2" SizeC="1" SizeT="1">
        <TiffData IFD="0" FirstZ="1"/>
        <TiffData IFD="1" FirstZ="0"/>
      </Pixels></Image></OME>"#;
    let pages = |ifd: usize| {
        Ok(if ifd == 0 {
            PageFacts {
                samples: 3,
                photometric: tiff_tags::PHOTOMETRIC_RGB,
                ..grey(4, 3, DType::U8)
            }
        } else {
            grey(4, 3, DType::U8)
        })
    };
    let read = parse_scenes(xml, FILE_IFDS, pages).expect("parse");
    let scene = &read.scenes[0];
    assert_eq!(scene.meta.samples, 1);
    assert_eq!(scene.plan.ifds, vec![1, 0]);
}

#[test]
fn scenes_past_the_file_plane_budget_are_refused_one_by_one() {
    // Five scenes of a million planes each: four fit the file's budget, the fifth does not.
    let scene = |name: &str| {
        format!(
            r#"<Image Name="{name}"><Pixels DimensionOrder="XYCZT" Type="uint8" SizeX="4" SizeY="3" SizeZ="1000000" SizeC="1" SizeT="1"><TiffData IFD="0" PlaneCount="1000000"/></Pixels></Image>"#
        )
    };
    let images: String = (0..5).map(|i| scene(&format!("s{i}"))).collect();
    let xml = format!("<OME>{images}</OME>");
    let read = parse_scenes(&xml, 1_000_000, every_page(4, 3)).expect("parse");
    assert_eq!(read.scenes.len(), 4);
    assert_eq!(read.refused.len(), 1);
    assert_eq!(read.refused[0].name, "s4");
    assert!(read.refused[0].reason.contains("more than"), "{:?}", read.refused);
}

#[test]
fn parse_ome_interleaved_is_unsupported() {
    // Interleaved (chunky-C) storage: the only scene is refused, so the file
    // has nothing to read, and the error says why.
    let xml = r#"<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
      <Image><Pixels DimensionOrder="XYCZT" Type="uint8" Interleaved="true"
             SizeX="4" SizeY="3" SizeZ="1" SizeC="3" SizeT="1"/></Image>
      </OME>"#;
    assert!(matches!(
        parse_scenes(xml, FILE_IFDS, every_page(4, 3)),
        Err(Error::Unsupported(_))
    ));
}

#[test]
fn an_indexed_first_page_is_refused_by_name() {
    let indexed = |_| {
        Ok(PageFacts {
            photometric: tiff_tags::PHOTOMETRIC_PALETTE,
            ..grey(4, 3, DType::U8)
        })
    };
    match parse_scenes(ONE_IMAGE, FILE_IFDS, indexed) {
        Err(Error::Unsupported(message)) => assert!(message.contains("RGBPalette"), "{message}"),
        other => panic!("expected a refusal by name, got {other:?}"),
    }
}

#[test]
fn a_malformed_image_refuses_only_its_own_scene() {
    // The middle <Image> has no SizeX. Its neighbours still read, and the refusal names it.
    let xml = r#"<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
      <Image Name="good"><Pixels DimensionOrder="XYCZT" Type="uint8"
             SizeX="4" SizeY="3" SizeZ="1" SizeC="1" SizeT="1"/></Image>
      <Image Name="broken"><Pixels DimensionOrder="XYCZT" Type="uint8"
             SizeY="3" SizeZ="1" SizeC="1" SizeT="1"/></Image>
      <Image Name="also good"><Pixels DimensionOrder="XYCZT" Type="uint8"
             SizeX="2" SizeY="2" SizeZ="1" SizeC="1" SizeT="1">
        <TiffData IFD="1"/></Pixels></Image>
      </OME>"#;
    let pages = |ifd: usize| Ok(if ifd == 1 { grey(2, 2, DType::U8) } else { grey(4, 3, DType::U8) });
    let read = parse_scenes(xml, FILE_IFDS, pages).expect("parse");
    let names: Vec<&str> = read.scenes.iter().map(|s| s.meta.name.as_str()).collect();
    assert_eq!(names, vec!["good", "also good"]);
    assert_eq!(read.refused.len(), 1);
    assert_eq!(read.refused[0].image_index, 1);
    assert_eq!(read.refused[0].name, "broken");
    assert!(read.refused[0].reason.contains("SizeX"), "{:?}", read.refused);
}

#[test]
fn a_later_scene_without_tiff_data_is_refused() {
    // The file does not say where scene 1's planes are, so scene 1 is refused
    // and scene 0 still reads.
    let xml = r#"<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
      <Image Name="first"><Pixels DimensionOrder="XYCZT" Type="uint8"
             SizeX="4" SizeY="3" SizeZ="1" SizeC="1" SizeT="1"/></Image>
      <Image Name="second"><Pixels DimensionOrder="XYCZT" Type="uint8"
             SizeX="4" SizeY="3" SizeZ="1" SizeC="1" SizeT="1"/></Image>
      </OME>"#;
    let read = read(xml);
    assert_eq!(read.scenes.len(), 1);
    assert_eq!(read.scenes[0].meta.name, "first");
    assert_eq!(read.refused.len(), 1);
    assert_eq!(read.refused[0].image_index, 1);
    assert_eq!(read.refused[0].name, "second");
}

#[test]
fn a_file_whose_every_scene_is_refused_is_an_error() {
    let xml = r#"<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
      <Image><Pixels DimensionOrder="XYCZT" Type="uint8" SizeY="3" SizeZ="1" SizeC="1" SizeT="1"/></Image>
      </OME>"#;
    assert!(matches!(
        parse_scenes(xml, FILE_IFDS, every_page(4, 3)),
        Err(Error::Unsupported(_))
    ));
}

#[test]
fn a_scene_whose_size_differs_from_its_first_page_is_refused() {
    // SizeX/SizeY say 4x3 but the page is 2x2.
    let read = parse_scenes(ONE_IMAGE, FILE_IFDS, every_page(2, 2));
    assert!(matches!(read, Err(Error::Unsupported(_))));
}

#[test]
fn parse_ome_physical_size_units_convert() {
    // 100 nm is 0.1 µm; 0.002 mm is 2 µm. The unit attributes are part of
    // the declaration, not decoration.
    let xml = r#"<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
      <Image><Pixels DimensionOrder="XYCZT" Type="uint8"
             SizeX="4" SizeY="3" SizeZ="2" SizeC="1" SizeT="1"
             PhysicalSizeX="100" PhysicalSizeXUnit="nm"
             PhysicalSizeY="100" PhysicalSizeYUnit="nm"
             PhysicalSizeZ="0.002" PhysicalSizeZUnit="mm"/>
      </Image></OME>"#;
    let read = read(xml);
    let px = read.scenes[0].meta.px_size_um.expect("the sizes are declared");
    assert!((px.x - 0.1).abs() < 1e-6);
    assert!((px.y - 0.1).abs() < 1e-6);
    assert!((px.z.expect("the Z step") - 2.0).abs() < 1e-6);
}

#[test]
fn parse_ome_a_placeholder_z_step_is_dropped_and_says_so() {
    // 1e-7 µm is a hundredth of the smallest step a microscope could have
    // measured: the file wrote a placeholder where the Z step would go.
    let xml = r#"<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
      <Image><Pixels DimensionOrder="XYCZT" Type="uint8"
             SizeX="4" SizeY="3" SizeZ="1" SizeC="1" SizeT="1"
             PhysicalSizeX="0.1" PhysicalSizeY="0.1" PhysicalSizeZ="1e-7"/>
      </Image></OME>"#;
    let read = read(xml);
    let scene = &read.scenes[0];
    let px = scene.meta.px_size_um.expect("the sizes are declared");
    assert_eq!(px.z, None, "the placeholder is not a step");
    assert!((px.x - 0.1).abs() < 1e-6, "the X size is published");
    assert_eq!(
        scene.meta.notes,
        vec![crate::calibration::z_step_note(1e-7).expect("a placeholder earns a note")],
        "the note names the placeholder the file stated"
    );
}

#[test]
fn parse_ome_unreadable_size_unit_publishes_no_size() {
    // A unit the table cannot read names no scale: publish none rather
    // than silently reading the number as micrometres.
    let xml = r#"<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
      <Image><Pixels DimensionOrder="XYCZT" Type="uint8"
             SizeX="4" SizeY="3" SizeZ="1" SizeC="1" SizeT="1"
             PhysicalSizeX="100" PhysicalSizeXUnit="parsec"
             PhysicalSizeY="100"/>
      </Image></OME>"#;
    let read = read(xml);
    assert!(read.scenes[0].meta.px_size_um.is_none());
}

#[test]
fn parse_ome_minimal_pixels() {
    let xml = r#"<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
      <Image><Pixels DimensionOrder="XYCZT" Type="uint16"
             SizeX="4" SizeY="3" SizeZ="2" SizeC="2" SizeT="1"
             PhysicalSizeX="0.1" PhysicalSizeY="0.1" PhysicalSizeZ="0.5">
        <Channel Name="DAPI"/>
        <Channel Name="GFP"/>
      </Pixels></Image></OME>"#;
    let read = parse_scenes(xml, FILE_IFDS, |_| Ok(grey(4, 3, DType::U16))).expect("parse");
    let scene = &read.scenes[0];
    assert_eq!(scene.meta.dims, Dims5 { t: 1, c: 2, z: 2, y: 3, x: 4 });
    assert_eq!(scene.meta.dtype, DType::U16);
    assert_eq!(scene.meta.channels[1].name.as_deref(), Some("GFP"));
    let px = scene.meta.px_size_um.expect("the sizes are declared");
    assert!((px.z.expect("the Z step") - 0.5).abs() < 1e-6);
}
