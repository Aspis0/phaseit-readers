use crate::error::Error;
use crate::types::{DType, Dims5, ImageReader, PlaneBuf};

use super::color::lut_color;
use super::LifReader;
use super::scene::dtype_from_channel;
use super::xml::{collect_images, decode_utf16le, parse_dom};

#[test]
fn utf16le_roundtrip_and_odd_length() {
    let s = "MB0";
    let bytes: Vec<u8> = s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    assert_eq!(decode_utf16le(&bytes).unwrap(), "MB0");
    assert!(matches!(decode_utf16le(&[0x00]), Err(Error::Parse(_))));
}

#[test]
fn dtype_mapping() {
    assert_eq!(dtype_from_channel(0, 8).unwrap(), DType::U8);
    assert_eq!(dtype_from_channel(0, 1).unwrap(), DType::U8);
    assert_eq!(dtype_from_channel(0, 16).unwrap(), DType::U16);
    assert_eq!(dtype_from_channel(0, 12).unwrap(), DType::U16);
    assert_eq!(dtype_from_channel(1, 32).unwrap(), DType::F32);
    assert!(dtype_from_channel(0, 32).is_err());
    assert!(dtype_from_channel(2, 8).is_err());
}

#[test]
fn lut_colors() {
    assert_eq!(lut_color("Green"), Some([0, 255, 0]));
    assert_eq!(lut_color("RED"), Some([255, 0, 0]));
    assert_eq!(lut_color("Gray"), Some([255, 255, 255]));
    assert_eq!(lut_color("Rhodamine"), None);
}

#[test]
fn dom_parse_and_navigate() {
    let xml = r#"<Root a="1"><Element Name="X"><Data><Image/></Data>
        <Memory MemoryBlockID="MB0" Size="16"/></Element></Root>"#;
    let root = parse_dom(xml).unwrap();
    let mut images = Vec::new();
    collect_images(&root, &mut images);
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].attr("Name"), Some("X"));
    assert_eq!(
        images[0].child("Memory").unwrap().attr("MemoryBlockID"),
        Some("MB0")
    );
}

#[test]
fn the_entities_of_an_attribute_are_resolved() {
    let xml = r#"<Root Name="A&amp;B"><Element Name="X &lt;Y&gt; &#233;"/></Root>"#;
    let root = parse_dom(xml).unwrap();
    let root = root.child("Root").expect("the root element is read");
    assert_eq!(root.attr("Name"), Some("A&B"));
    let element = root.child("Element").expect("the element is read");
    assert_eq!(element.attr("Name"), Some("X <Y> é"));
}

#[test]
fn an_entity_that_cannot_be_resolved_is_read_as_it_was_written() {
    let xml = r#"<Root Name="A&bogus;B"><Element Name="X&bogus;Y"/></Root>"#;
    let root = parse_dom(xml).unwrap();
    let root = root.child("Root").expect("the root element is read");
    assert_eq!(root.attr("Name"), Some("A&bogus;B"));
    let element = root.child("Element").expect("the element is read");
    assert_eq!(element.attr("Name"), Some("X&bogus;Y"));
}

#[test]
fn a_cdata_section_does_not_disturb_the_reader() {
    // CDATA is text, which the LIF reader ignores: the attributes around it are still read.
    let xml = r#"<Root Name="A"><Element Name="X"><![CDATA[<not markup>]]></Element></Root>"#;
    let root = parse_dom(xml).unwrap();
    let root = root.child("Root").expect("the root element is read");
    assert_eq!(root.attr("Name"), Some("A"));
    let element = root.child("Element").expect("the element is read");
    assert_eq!(element.attr("Name"), Some("X"));
}

/// A minimal LIF with one planar Z=2, C=2, 2x2 u8 image; each byte is a unique tag.
fn synthetic_lif() -> Vec<u8> {
    // Layout: Y.BytesInc 2, channel1 at 4, Z.BytesInc 8 over a 16-byte block.
    let xml = concat!(
        r#"<LMSDataContainerHeader Version="2"><Element Name="Root"><Children>"#,
        r#"<Element Name="Img"><Data><Image><ImageDescription><Dimensions>"#,
        r#"<DimensionDescription DimID="1" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="1" BitInc="0"/>"#,
        r#"<DimensionDescription DimID="2" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="2" BitInc="0"/>"#,
        r#"<DimensionDescription DimID="3" NumberOfElements="2" Origin="0" Length="2e-6" Unit="m" BytesInc="8" BitInc="0"/>"#,
        r#"</Dimensions><Channels>"#,
        r#"<ChannelDescription DataType="0" ChannelTag="0" Resolution="8" NameOfMeasuredQuantity="" Min="0" Max="255" Unit="" LUTName="Green" IsLUTInverted="0" BytesInc="0" BitInc="0"/>"#,
        r#"<ChannelDescription DataType="0" ChannelTag="0" Resolution="8" NameOfMeasuredQuantity="" Min="0" Max="255" Unit="" LUTName="Red" IsLUTInverted="0" BytesInc="4" BitInc="0"/>"#,
        r#"</Channels></ImageDescription></Image></Data>"#,
        r#"<Memory MemoryBlockID="MB0" Size="16"/></Element></Children></Element></LMSDataContainerHeader>"#,
    );
    wrap_lif_bytes(xml)
}

/// Header + XML + one `MB0` block of 16 bytes; tests write only the XML.
fn wrap_lif_bytes(xml: &str) -> Vec<u8> {
    let xml_utf16: Vec<u8> = xml.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let xml_chars = (xml_utf16.len() / 2) as u32;

    let mut f: Vec<u8> = Vec::new();
    f.extend_from_slice(&0x70u32.to_le_bytes());
    f.extend_from_slice(&0u32.to_le_bytes());
    f.push(0x2A);
    f.extend_from_slice(&xml_chars.to_le_bytes());
    f.extend_from_slice(&xml_utf16);

    let id: Vec<u8> = "MB0".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    f.extend_from_slice(&0x70u32.to_le_bytes());
    f.extend_from_slice(&0u32.to_le_bytes());
    f.push(0x2A);
    f.extend_from_slice(&16u64.to_le_bytes());
    f.push(0x2A);
    f.extend_from_slice(&(("MB0".len()) as u32).to_le_bytes());
    f.extend_from_slice(&id);
    f.extend_from_slice(&(0u8..16).collect::<Vec<u8>>());
    f
}

fn append_lif_block(f: &mut Vec<u8>, id: &str, data: &[u8]) {
    let id_bytes: Vec<u8> = id.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    f.extend_from_slice(&0x70u32.to_le_bytes());
    f.extend_from_slice(&0u32.to_le_bytes());
    f.push(0x2A);
    f.extend_from_slice(&(data.len() as u64).to_le_bytes());
    f.push(0x2A);
    f.extend_from_slice(&(id.len() as u32).to_le_bytes());
    f.extend_from_slice(&id_bytes);
    f.extend_from_slice(data);
}

/// Like `wrap_lif_bytes`, with two named blocks, so a bad image reaches its own validation.
fn wrap_lif_bytes_two_blocks(xml: &str, id0: &str, data0: &[u8], id1: &str, data1: &[u8]) -> Vec<u8> {
    let xml_utf16: Vec<u8> = xml.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let xml_chars = (xml_utf16.len() / 2) as u32;

    let mut f: Vec<u8> = Vec::new();
    f.extend_from_slice(&0x70u32.to_le_bytes());
    f.extend_from_slice(&0u32.to_le_bytes());
    f.push(0x2A);
    f.extend_from_slice(&xml_chars.to_le_bytes());
    f.extend_from_slice(&xml_utf16);

    append_lif_block(&mut f, id0, data0);
    append_lif_block(&mut f, id1, data1);
    f
}

fn open_synth(bytes: &[u8]) -> LifReader {
    let dir = std::env::temp_dir();
    // Unique per call: tests run in parallel threads, so a name keyed on the process id alone would race.
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!(
        "phasit_synth_{:?}_{}_{}.lif",
        std::thread::current().id(),
        std::process::id(),
        n
    ));
    std::fs::write(&path, bytes).unwrap();
    let r = LifReader::open(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    r
}

#[test]
fn synthetic_geometry_and_plane_mapping() {
    let bytes = synthetic_lif();
    let reader = open_synth(&bytes);

    assert_eq!(reader.meta().scene_count(), 1);
    let scene = reader.meta().scene(0).unwrap();
    assert_eq!(scene.name, "Img");
    assert_eq!(scene.dims, Dims5 { t: 1, c: 2, z: 2, y: 2, x: 2 });
    assert_eq!(scene.dtype, DType::U8);
    assert_eq!(scene.channels.len(), 2);
    assert_eq!(scene.channels[0].name.as_deref(), Some("Green"));
    assert_eq!(scene.channels[1].color, Some([255, 0, 0]));

    let cases = [
        (0usize, 0usize, vec![0u8, 1, 2, 3]),
        (1, 0, vec![4, 5, 6, 7]),
        (0, 1, vec![8, 9, 10, 11]),
        (1, 1, vec![12, 13, 14, 15]),
    ];
    for (c, z, expect) in cases {
        let p = reader.plane(0, 0, c, z).unwrap();
        assert_eq!(p.shape, [2, 2]);
        match p.data {
            PlaneBuf::U8(v) => assert_eq!(v, expect, "plane c{c} z{z}"),
            other => panic!("expected U8, got {:?}", other.dtype()),
        }
    }
}

#[test]
fn synthetic_out_of_range() {
    let reader = open_synth(&synthetic_lif());
    assert!(matches!(
        reader.plane(0, 0, 0, 2),
        Err(Error::OutOfRange { z: 2, .. })
    ));
    assert!(matches!(
        reader.plane(0, 0, 2, 0),
        Err(Error::OutOfRange { c: 2, .. })
    ));
    assert!(matches!(
        reader.plane(0, 1, 0, 0),
        Err(Error::OutOfRange { t: 1, .. })
    ));
    assert!(matches!(
        reader.plane(1, 0, 0, 0),
        Err(Error::SceneOutOfRange { scene: 1, count: 1 })
    ));
}

/// A channel dtype the reader does not model refuses that image, not the whole file.
#[test]
fn unmodeled_channel_dtype_refuses_image_but_opens_file() {
    let xml = concat!(
        r#"<LMSDataContainerHeader Version="2"><Element Name="Root"><Children>"#,
        r#"<Element Name="Good"><Data><Image><ImageDescription><Dimensions>"#,
        r#"<DimensionDescription DimID="1" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="1" BitInc="0"/>"#,
        r#"<DimensionDescription DimID="2" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="2" BitInc="0"/>"#,
        r#"</Dimensions><Channels>"#,
        r#"<ChannelDescription DataType="0" ChannelTag="0" Resolution="8" NameOfMeasuredQuantity="" Min="0" Max="255" Unit="" LUTName="Gray" IsLUTInverted="0" BytesInc="0" BitInc="0"/>"#,
        r#"</Channels></ImageDescription></Image></Data>"#,
        r#"<Memory MemoryBlockID="MB0" Size="16"/></Element>"#,
        // Bad image: unsigned 32-bit (DataType 0, Resolution 32) is not modelled.
        r#"<Element Name="Bad"><Data><Image><ImageDescription><Dimensions>"#,
        r#"<DimensionDescription DimID="1" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="1" BitInc="0"/>"#,
        r#"<DimensionDescription DimID="2" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="2" BitInc="0"/>"#,
        r#"</Dimensions><Channels>"#,
        r#"<ChannelDescription DataType="0" ChannelTag="0" Resolution="32" NameOfMeasuredQuantity="" Min="0" Max="255" Unit="" LUTName="Gray" IsLUTInverted="0" BytesInc="0" BitInc="0"/>"#,
        r#"</Channels></ImageDescription></Image></Data>"#,
        r#"<Memory MemoryBlockID="MB1" Size="16"/></Element>"#,
        r#"</Children></Element></LMSDataContainerHeader>"#,
    );
    // Two real blocks: the bad image can only be skipped by its dtype, not a missing block.
    let bytes = wrap_lif_bytes_two_blocks(
        xml,
        "MB0",
        &(0u8..16).collect::<Vec<u8>>(),
        "MB1",
        &(0u8..16).collect::<Vec<u8>>(),
    );
    let reader = open_synth(&bytes);
    assert_eq!(
        reader.meta().scene_count(),
        1,
        "the unmodelled-dtype image must be REFUSED, not abort the whole file"
    );
    assert_eq!(reader.meta().scene(0).unwrap().name, "Good");
}

/// Two channels with equal BytesInc alias the same bytes: the image is refused, not decoded.
#[test]
fn equal_channel_bytes_inc_refuses_image_but_opens_file() {
    let xml = concat!(
        r#"<LMSDataContainerHeader Version="2"><Element Name="Root"><Children>"#,
        r#"<Element Name="Good"><Data><Image><ImageDescription><Dimensions>"#,
        r#"<DimensionDescription DimID="1" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="1" BitInc="0"/>"#,
        r#"<DimensionDescription DimID="2" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="2" BitInc="0"/>"#,
        r#"</Dimensions><Channels>"#,
        r#"<ChannelDescription DataType="0" ChannelTag="0" Resolution="8" NameOfMeasuredQuantity="" Min="0" Max="255" Unit="" LUTName="Gray" IsLUTInverted="0" BytesInc="0" BitInc="0"/>"#,
        r#"</Channels></ImageDescription></Image></Data>"#,
        r#"<Memory MemoryBlockID="MB0" Size="16"/></Element>"#,
        r#"<Element Name="Bad"><Data><Image><ImageDescription><Dimensions>"#,
        r#"<DimensionDescription DimID="1" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="1" BitInc="0"/>"#,
        r#"<DimensionDescription DimID="2" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="2" BitInc="0"/>"#,
        r#"</Dimensions><Channels>"#,
        r#"<ChannelDescription DataType="0" ChannelTag="0" Resolution="8" NameOfMeasuredQuantity="" Min="0" Max="255" Unit="" LUTName="Green" IsLUTInverted="0" BytesInc="0" BitInc="0"/>"#,
        r#"<ChannelDescription DataType="0" ChannelTag="0" Resolution="8" NameOfMeasuredQuantity="" Min="0" Max="255" Unit="" LUTName="Red" IsLUTInverted="0" BytesInc="0" BitInc="0"/>"#,
        r#"</Channels></ImageDescription></Image></Data>"#,
        r#"<Memory MemoryBlockID="MB1" Size="16"/></Element>"#,
        r#"</Children></Element></LMSDataContainerHeader>"#,
    );
    let bytes = wrap_lif_bytes_two_blocks(
        xml,
        "MB0",
        &(0u8..16).collect::<Vec<u8>>(),
        "MB1",
        &(0u8..16).collect::<Vec<u8>>(),
    );
    let reader = open_synth(&bytes);
    assert_eq!(
        reader.meta().scene_count(),
        1,
        "an image whose channels have equal BytesInc (aliasing) must be REFUSED"
    );
    assert_eq!(reader.meta().scene(0).unwrap().name, "Good");
}

/// A Z step below one frame makes consecutive planes overlap: the image is refused.
#[test]
fn undersized_z_stride_refuses_image_but_opens_file() {
    let xml = concat!(
        r#"<LMSDataContainerHeader Version="2"><Element Name="Root"><Children>"#,
        r#"<Element Name="Good"><Data><Image><ImageDescription><Dimensions>"#,
        r#"<DimensionDescription DimID="1" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="1" BitInc="0"/>"#,
        r#"<DimensionDescription DimID="2" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="2" BitInc="0"/>"#,
        r#"</Dimensions><Channels>"#,
        r#"<ChannelDescription DataType="0" ChannelTag="0" Resolution="8" NameOfMeasuredQuantity="" Min="0" Max="255" Unit="" LUTName="Gray" IsLUTInverted="0" BytesInc="0" BitInc="0"/>"#,
        r#"</Channels></ImageDescription></Image></Data>"#,
        r#"<Memory MemoryBlockID="MB0" Size="16"/></Element>"#,
        r#"<Element Name="Bad"><Data><Image><ImageDescription><Dimensions>"#,
        r#"<DimensionDescription DimID="1" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="1" BitInc="0"/>"#,
        r#"<DimensionDescription DimID="2" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="2" BitInc="0"/>"#,
        r#"<DimensionDescription DimID="3" NumberOfElements="2" Origin="0" Length="2e-6" Unit="m" BytesInc="2" BitInc="0"/>"#,
        r#"</Dimensions><Channels>"#,
        r#"<ChannelDescription DataType="0" ChannelTag="0" Resolution="8" NameOfMeasuredQuantity="" Min="0" Max="255" Unit="" LUTName="Gray" IsLUTInverted="0" BytesInc="0" BitInc="0"/>"#,
        r#"</Channels></ImageDescription></Image></Data>"#,
        r#"<Memory MemoryBlockID="MB1" Size="16"/></Element>"#,
        r#"</Children></Element></LMSDataContainerHeader>"#,
    );
    let bytes = wrap_lif_bytes_two_blocks(
        xml,
        "MB0",
        &(0u8..16).collect::<Vec<u8>>(),
        "MB1",
        &(0u8..16).collect::<Vec<u8>>(),
    );
    let reader = open_synth(&bytes);
    assert_eq!(
        reader.meta().scene_count(),
        1,
        "an image whose Z stride is smaller than one frame (overlapping Z planes) must be REFUSED"
    );
    assert_eq!(reader.meta().scene(0).unwrap().name, "Good");
}

/// An unmodelled `DimID` with zero elements refuses only its image.
#[test]
fn unmodeled_dimid_with_zero_elements_refuses_image_but_opens_file() {
    let xml = concat!(
        r#"<LMSDataContainerHeader Version="2"><Element Name="Root"><Children>"#,
        r#"<Element Name="Good"><Data><Image><ImageDescription><Dimensions>"#,
        r#"<DimensionDescription DimID="1" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="1" BitInc="0"/>"#,
        r#"<DimensionDescription DimID="2" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="2" BitInc="0"/>"#,
        r#"</Dimensions><Channels>"#,
        r#"<ChannelDescription DataType="0" ChannelTag="0" Resolution="8" NameOfMeasuredQuantity="" Min="0" Max="255" Unit="" LUTName="Gray" IsLUTInverted="0" BytesInc="0" BitInc="0"/>"#,
        r#"</Channels></ImageDescription></Image></Data>"#,
        r#"<Memory MemoryBlockID="MB0" Size="16"/></Element>"#,
        r#"<Element Name="Bad"><Data><Image><ImageDescription><Dimensions>"#,
        r#"<DimensionDescription DimID="1" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="1" BitInc="0"/>"#,
        r#"<DimensionDescription DimID="2" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="2" BitInc="0"/>"#,
        r#"<DimensionDescription DimID="5" NumberOfElements="0" Origin="0" Length="0" Unit="" BytesInc="0" BitInc="0"/>"#,
        r#"</Dimensions><Channels>"#,
        r#"<ChannelDescription DataType="0" ChannelTag="0" Resolution="8" NameOfMeasuredQuantity="" Min="0" Max="255" Unit="" LUTName="Gray" IsLUTInverted="0" BytesInc="0" BitInc="0"/>"#,
        r#"</Channels></ImageDescription></Image></Data>"#,
        r#"<Memory MemoryBlockID="MB1" Size="16"/></Element>"#,
        r#"</Children></Element></LMSDataContainerHeader>"#,
    );
    // Two real blocks, so the image is skipped by its DimID, not by a missing block.
    let bytes = wrap_lif_bytes_two_blocks(
        xml,
        "MB0",
        &(0u8..16).collect::<Vec<u8>>(),
        "MB1",
        &(0u8..16).collect::<Vec<u8>>(),
    );
    let reader = open_synth(&bytes);
    assert_eq!(
        reader.meta().scene_count(),
        1,
        "the unmodelled-DimID/0-elements image must be REFUSED, not abort the whole file"
    );
    assert_eq!(reader.meta().scene(0).unwrap().name, "Good");
}
