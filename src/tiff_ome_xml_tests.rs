// SPDX-License-Identifier: Apache-2.0
use super::*;

#[test]
fn ome_color_parses_rgba_int() {
    // 0xFF000000 => red channel full.
    assert_eq!(parse_ome_color("-16777216"), Some([255, 0, 0]));
    // 0x00FF00FF => green + alpha; RGB = [0,255,0].
    assert_eq!(parse_ome_color("16711935"), Some([0, 255, 0]));
    assert_eq!(parse_ome_color("not-a-number"), None);
}

#[test]
fn ome_color_outside_32_bits_names_no_colour() {
    // 2^32 wraps to 0 under `as u32`, which would publish black.
    assert_eq!(parse_ome_color("4294967296"), None);
    assert_eq!(parse_ome_color("-2147483649"), None);
}

#[test]
fn a_description_without_an_image_is_refused() {
    assert!(parse_images("<OME/>").is_err());
}

#[test]
fn an_image_count_past_the_cap_refuses_the_file() {
    let mut xml = String::from("<OME>");
    for _ in 0..=MAX_OME_IMAGES {
        xml.push_str("<Image/>");
    }
    xml.push_str("</OME>");
    assert!(matches!(parse_images(&xml), Err(Error::Parse(_))));
}

/// An OME description whose `<Image>`s name these many `<tag>` elements each.
fn description_with(images: &[usize], tag: &str) -> String {
    let element = format!("<{tag}/>");
    let mut xml = String::from("<OME>");
    for count in images {
        xml.push_str(r#"<Image Name="many"><Pixels>"#);
        for _ in 0..*count {
            xml.push_str(&element);
        }
        xml.push_str("</Pixels></Image>");
    }
    xml.push_str("</OME>");
    xml
}

/// The scene counts of a file whose `<TiffData>` blocks fill its plane budget:
/// one scene per `MAX_OME_TIFF_DATA_PER_FILE / MAX_OME_TIFF_DATA_PER_IMAGE` of
/// them, each at the per-scene cap.
fn scenes_at_the_tiff_data_budget() -> Vec<usize> {
    vec![
        MAX_OME_TIFF_DATA_PER_IMAGE;
        MAX_OME_TIFF_DATA_PER_FILE / MAX_OME_TIFF_DATA_PER_IMAGE
    ]
}

#[test]
fn an_image_at_the_plane_cap_of_tiff_data_is_admitted() {
    // One <TiffData> per plane, as many as a scene's plane budget holds: the
    // file states what the reader reads, so it is admitted.
    let xml = description_with(&[MAX_OME_TIFF_DATA_PER_IMAGE], "TiffData");
    let document = parse_images(&xml).expect("an image at the plane cap is read");
    assert_eq!(
        document.images[0].tiff_data.len(),
        MAX_OME_TIFF_DATA_PER_IMAGE
    );
}

#[test]
fn an_image_past_the_plane_cap_of_tiff_data_is_refused_by_name() {
    // One block past the scene's plane budget: the list must not grow past it.
    let xml = description_with(&[MAX_OME_TIFF_DATA_PER_IMAGE + 1], "TiffData");
    match parse_images(&xml) {
        Err(Error::Parse(message)) => assert!(message.contains("<TiffData>"), "{message}"),
        other => panic!("expected a refusal by name, got {other:?}"),
    }
}

#[test]
fn a_file_at_the_plane_cap_of_tiff_data_is_admitted() {
    // Scenes at the per-scene cap, as many as the file's own plane budget
    // holds: the file states what the reader reads, so it is admitted.
    let counts = scenes_at_the_tiff_data_budget();
    let xml = description_with(&counts, "TiffData");
    let document = parse_images(&xml).expect("a file at the plane cap is read");
    assert_eq!(document.images.len(), counts.len());
}

#[test]
fn a_file_past_the_plane_cap_of_tiff_data_is_refused_by_name() {
    // Every scene stays under the per-image cap; the file's own plane budget
    // is what the description exceeds.
    let mut counts = scenes_at_the_tiff_data_budget();
    counts.push(1);
    let xml = description_with(&counts, "TiffData");
    match parse_images(&xml) {
        Err(Error::Parse(message)) => {
            assert!(message.contains("<TiffData>"), "{message}");
            assert!(message.contains("one file"), "{message}");
        }
        other => panic!("expected a refusal by name, got {other:?}"),
    }
}

#[test]
fn an_image_at_the_plane_cap_of_channels_is_admitted() {
    let xml = description_with(&[MAX_OME_CHANNELS_PER_IMAGE], "Channel");
    let document = parse_images(&xml).expect("an image at the plane cap is read");
    assert_eq!(
        document.images[0].channels.len(),
        MAX_OME_CHANNELS_PER_IMAGE
    );
}

#[test]
fn an_image_past_the_plane_cap_of_channels_is_refused_by_name() {
    let xml = description_with(&[MAX_OME_CHANNELS_PER_IMAGE + 1], "Channel");
    match parse_images(&xml) {
        Err(Error::Parse(message)) => assert!(message.contains("<Channel>"), "{message}"),
        other => panic!("expected a refusal by name, got {other:?}"),
    }
}

#[test]
fn a_file_at_the_plane_cap_of_channels_is_admitted() {
    let scenes = MAX_OME_CHANNELS_PER_FILE / MAX_OME_CHANNELS_PER_IMAGE;
    let counts = vec![MAX_OME_CHANNELS_PER_IMAGE; scenes];
    let xml = description_with(&counts, "Channel");
    let document = parse_images(&xml).expect("a file at the plane cap is read");
    assert_eq!(document.images.len(), scenes);
}

#[test]
fn a_file_past_the_plane_cap_of_channels_is_refused_by_name() {
    let scenes = MAX_OME_CHANNELS_PER_FILE / MAX_OME_CHANNELS_PER_IMAGE;
    let mut counts = vec![MAX_OME_CHANNELS_PER_IMAGE; scenes];
    counts.push(1);
    let xml = description_with(&counts, "Channel");
    match parse_images(&xml) {
        Err(Error::Parse(message)) => {
            assert!(message.contains("<Channel>"), "{message}");
            assert!(message.contains("one file"), "{message}");
        }
        other => panic!("expected a refusal by name, got {other:?}"),
    }
}

#[test]
fn every_image_is_listed_in_file_order_with_its_tiff_data() {
    let xml = r#"<OME UUID="urn:uuid:root">
      <Image Name="a"><Pixels SizeX="1" SizeY="1">
        <TiffData IFD="3" PlaneCount="2"><UUID FileName="x.ome.tif">urn:uuid:1</UUID></TiffData>
        <TiffData FirstT="1"/>
      </Pixels></Image>
      <Image Name="b"><Pixels SizeX="1" SizeY="1"/></Image>
    </OME>"#;
    let document = parse_images(xml).unwrap();
    assert_eq!(document.uuid.as_deref(), Some("urn:uuid:root"));
    let images = &document.images;
    assert_eq!(images.len(), 2);
    assert_eq!(images[0].name.as_deref(), Some("a"));
    assert_eq!(images[0].tiff_data.len(), 2);
    assert_eq!(images[0].tiff_data[0].ifd.as_deref(), Some("3"));
    assert_eq!(images[0].tiff_data[0].uuid.as_deref(), Some("urn:uuid:1"));
    assert_eq!(images[0].tiff_data[1].first_t.as_deref(), Some("1"));
    assert!(images[1].tiff_data.is_empty());
}
