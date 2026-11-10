// SPDX-License-Identifier: Apache-2.0
//! An OME-TIFF's planes are the IFDs its `<TiffData>` blocks name. Each test
//! writes pages of known values and reads back the plane each block addresses.

mod support;

use phasit_imread::tiff_reader::TiffReader;
use phasit_imread::{ImageReader, PlaneBuf};
use support::{scratch_dir, write_grey, GreyPage};

fn page(value: u8) -> GreyPage {
    GreyPage {
        width: 2,
        height: 2,
        value,
    }
}

fn first_pixel(path: &std::path::Path, scene: usize, z: usize) -> u8 {
    let reader = TiffReader::open(path).expect("open the OME-TIFF");
    let plane = reader.plane(scene, 0, 0, z).expect("read the plane");
    match plane.data {
        PlaneBuf::U8(values) => values[0],
        _ => panic!("the fixture is 8-bit"),
    }
}

#[test]
fn interleaved_out_of_order_scenes_read_their_own_ifds() {
    // IFD 0 and 2 are scene A's planes; A's TiffData blocks are listed last plane first.
    let dir = scratch_dir("ome_interleaved");
    let path = dir.join("interleaved.ome.tif");
    let xml = r#"<?xml version="1.0"?>
<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06" UUID="urn:uuid:f1">
  <Image Name="a"><Pixels DimensionOrder="XYCZT" Type="uint8" SizeX="2" SizeY="2" SizeZ="2" SizeC="1" SizeT="1">
    <TiffData IFD="2" FirstZ="1"/>
    <TiffData IFD="0" FirstZ="0"/>
  </Pixels></Image>
  <Image Name="b"><Pixels DimensionOrder="XYCZT" Type="uint8" SizeX="2" SizeY="2" SizeZ="2" SizeC="1" SizeT="1">
    <TiffData IFD="1" FirstZ="0"/>
    <TiffData IFD="3" FirstZ="1"/>
  </Pixels></Image>
</OME>"#;
    write_grey(&path, Some(xml), &[page(10), page(20), page(11), page(21)]);

    assert_eq!(first_pixel(&path, 0, 0), 10);
    assert_eq!(first_pixel(&path, 0, 1), 11);
    assert_eq!(first_pixel(&path, 1, 0), 20);
    assert_eq!(first_pixel(&path, 1, 1), 21);
}

#[test]
fn plane_count_covers_consecutive_ifds() {
    // IFD 0 is a decoy: a positional reader would read 99 for plane z=0.
    let dir = scratch_dir("ome_plane_count");
    let path = dir.join("plane_count.ome.tif");
    let xml = r#"<?xml version="1.0"?>
<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
  <Image Name="stack"><Pixels DimensionOrder="XYCZT" Type="uint8" SizeX="2" SizeY="2" SizeZ="3" SizeC="1" SizeT="1">
    <TiffData IFD="1" PlaneCount="3"/>
  </Pixels></Image>
</OME>"#;
    write_grey(&path, Some(xml), &[page(99), page(1), page(2), page(3)]);

    assert_eq!(first_pixel(&path, 0, 0), 1);
    assert_eq!(first_pixel(&path, 0, 1), 2);
    assert_eq!(first_pixel(&path, 0, 2), 3);
}

#[test]
fn absent_tiff_data_reads_the_file_in_order() {
    // No <TiffData> in a single-scene file: the spec default takes IFD 0, 1, ...
    let dir = scratch_dir("ome_no_tiff_data");
    let path = dir.join("no_tiff_data.ome.tif");
    let xml = r#"<?xml version="1.0"?>
<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
  <Image Name="stack"><Pixels DimensionOrder="XYCZT" Type="uint8" SizeX="2" SizeY="2" SizeZ="2" SizeC="1" SizeT="1"/></Image>
</OME>"#;
    write_grey(&path, Some(xml), &[page(5), page(6)]);

    assert_eq!(first_pixel(&path, 0, 0), 5);
    assert_eq!(first_pixel(&path, 0, 1), 6);
}

#[test]
fn a_malformed_image_leaves_its_neighbours_readable() {
    // The middle <Image> has no SizeX: the scenes either side still read their own IFDs.
    let dir = scratch_dir("ome_malformed_image");
    let path = dir.join("malformed.ome.tif");
    let xml = r#"<?xml version="1.0"?>
<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
  <Image Name="good"><Pixels DimensionOrder="XYCZT" Type="uint8" SizeX="2" SizeY="2" SizeZ="1" SizeC="1" SizeT="1">
    <TiffData IFD="0"/>
  </Pixels></Image>
  <Image Name="broken"><Pixels DimensionOrder="XYCZT" Type="uint8" SizeY="2" SizeZ="1" SizeC="1" SizeT="1"/></Image>
  <Image Name="also good"><Pixels DimensionOrder="XYCZT" Type="uint8" SizeX="2" SizeY="2" SizeZ="1" SizeC="1" SizeT="1">
    <TiffData IFD="1"/>
  </Pixels></Image>
</OME>"#;
    write_grey(&path, Some(xml), &[page(7), page(8)]);

    let reader = phasit_imread::open(&path).expect("open the OME-TIFF");
    assert_eq!(reader.meta().scene_count(), 2);
    assert_eq!(first_pixel(&path, 0, 0), 7);
    assert_eq!(first_pixel(&path, 1, 0), 8);
    let refused = reader.refused_scenes();
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0].image_index, 1);
    assert_eq!(refused[0].name, "broken");
    // "also good" is the second readable scene but the third <Image> in the file.
    assert_eq!(reader.scene_file_index(1), 2);
}

#[test]
fn a_declared_plane_count_past_the_cap_is_refused_by_name_at_open() {
    // SizeZ of a billion would be a 16 GB plane table: the scene must be refused before any table is allocated.
    let dir = scratch_dir("ome_huge_size_z");
    let path = dir.join("huge.ome.tif");
    let xml = r#"<?xml version="1.0"?>
<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
  <Image Name="huge"><Pixels DimensionOrder="XYCZT" Type="uint8" SizeX="2" SizeY="2" SizeZ="1000000000" SizeC="1" SizeT="1">
    <TiffData IFD="0"/>
  </Pixels></Image>
</OME>"#;
    write_grey(&path, Some(xml), &[page(1)]);

    let err = TiffReader::open(&path).err().expect("the file is refused at open");
    let message = err.to_string();
    assert!(message.contains("\"huge\""), "{message}");
    assert!(message.contains("oversized allocation refused"), "{message}");
}

/// One block past the crate's per-scene plane budget (1 Mi planes); the refusal assertion below fails if the cap moves.
const TIFF_DATA_OVER_THE_PER_IMAGE_CAP: usize = 1_048_577;

#[test]
fn an_ome_tiff_with_more_tiff_data_than_the_cap_is_refused_at_open_by_name() {
    // One <TiffData> per plane, one block past the scene's plane budget: refused at open, by name.
    let dir = scratch_dir("ome_too_many_tiff_data");
    let path = dir.join("too_many.ome.tif");
    let mut xml = String::from(
        r#"<?xml version="1.0"?>
<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
  <Image Name="many"><Pixels DimensionOrder="XYCZT" Type="uint8" SizeX="2" SizeY="2" SizeZ="1" SizeC="1" SizeT="1">"#,
    );
    for _ in 0..TIFF_DATA_OVER_THE_PER_IMAGE_CAP {
        xml.push_str("<TiffData/>");
    }
    xml.push_str("</Pixels></Image>\n</OME>");
    write_grey(&path, Some(&xml), &[page(1)]);

    let err = TiffReader::open(&path).err().expect("the file is refused at open");
    let message = err.to_string();
    assert!(message.contains("<TiffData>"), "{message}");
    assert!(message.contains("this reader reads at most"), "{message}");
}

#[test]
fn a_first_z_block_without_ifd_or_plane_count_takes_the_next_page() {
    // FirstZ=1 names plane 1 at IFD 0: the spec's default count is the file's IFD count, cut at the scene.
    let dir = scratch_dir("ome_first_z_default");
    let path = dir.join("first_z.ome.tif");
    let xml = r#"<?xml version="1.0"?>
<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
  <Image Name="stack"><Pixels DimensionOrder="XYCZT" Type="uint8" SizeX="2" SizeY="2" SizeZ="2" SizeC="1" SizeT="1">
    <TiffData FirstZ="1"/>
    <TiffData IFD="1" FirstZ="0"/>
  </Pixels></Image>
</OME>"#;
    write_grey(&path, Some(xml), &[page(99), page(7)]);

    assert_eq!(first_pixel(&path, 0, 0), 7);
    assert_eq!(first_pixel(&path, 0, 1), 99);
}

#[test]
fn a_scene_longer_than_the_files_pages_is_refused_at_open() {
    // Three planes, two pages, no PlaneCount: plane 2 has no IFD, so the scene is refused.
    let dir = scratch_dir("ome_default_past_pages");
    let path = dir.join("past_pages.ome.tif");
    let xml = r#"<?xml version="1.0"?>
<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
  <Image Name="stack"><Pixels DimensionOrder="XYCZT" Type="uint8" SizeX="2" SizeY="2" SizeZ="3" SizeC="1" SizeT="1">
    <TiffData/>
  </Pixels></Image>
</OME>"#;
    write_grey(&path, Some(xml), &[page(1), page(2)]);

    let err = TiffReader::open(&path).err().expect("the file is refused at open");
    let message = err.to_string();
    assert!(message.contains("plane 2"), "{message}");
}

#[test]
fn an_ifd_past_the_files_pages_is_refused_at_open_by_name() {
    // The file has two pages; the second plane names IFD 7.
    let dir = scratch_dir("ome_ifd_past_pages");
    let path = dir.join("ifd_past_pages.ome.tif");
    let xml = r#"<?xml version="1.0"?>
<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
  <Image Name="stack"><Pixels DimensionOrder="XYCZT" Type="uint8" SizeX="2" SizeY="2" SizeZ="2" SizeC="1" SizeT="1">
    <TiffData IFD="0" FirstZ="0"/>
    <TiffData IFD="7" FirstZ="1"/>
  </Pixels></Image>
</OME>"#;
    write_grey(&path, Some(xml), &[page(1), page(2)]);

    let err = TiffReader::open(&path).err().expect("the file is refused at open");
    let message = err.to_string();
    assert!(message.contains("IFD 7"), "{message}");
}
