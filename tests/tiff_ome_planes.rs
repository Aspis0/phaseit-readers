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

/// The first pixel of plane `(t=0, c=0, z)` of scene `scene`.
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
    // IFD 0 and 2 are scene A's planes, IFD 1 and 3 are scene B's, and A's
    // TiffData blocks are listed last plane first.
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
    // The middle <Image> has no SizeX. The scenes either side still read their
    // own IFDs, and the reader names the one it refused.
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

    // Through open(), the path every caller takes: the refusal is on the reader.
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
    // SizeZ of a billion would be a 16 GB plane table. The file is a few hundred
    // bytes, and the scene must be refused before any table is allocated.
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

/// The `<TiffData>` blocks one `<Image>` may name plus one: the crate's plane
/// budget of one scene (1 Mi planes) and one block past it. The constant is
/// private to the crate, so it is stated here; the assertion on the refusal
/// message below fails loudly if the crate's cap ever moves.
const TIFF_DATA_OVER_THE_PER_IMAGE_CAP: usize = 1_048_577;

#[test]
fn an_ome_tiff_with_more_tiff_data_than_the_cap_is_refused_at_open_by_name() {
    // One <Image> naming one <TiffData> per plane, one block past the plane
    // budget of a scene: each block becomes a struct, so the file is refused
    // at open, by name, before the list grows past the cap.
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
    // The refusal is the cap's own, not some later complaint about the blocks.
    assert!(message.contains("this reader reads at most"), "{message}");
}

#[test]
fn a_first_z_block_without_ifd_or_plane_count_takes_the_next_page() {
    // The spec's default count is the file's IFD count, cut at the scene: the
    // block with FirstZ=1 is plane 1 at IFD 0. Plane 0 is the block naming IFD 1.
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
    // Three planes, a file of two pages, and no PlaneCount: the default run
    // covers two planes, so plane 2 has no IFD and the scene is refused.
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
