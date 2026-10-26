// SPDX-License-Identifier: Apache-2.0
//! The `<Image>` list of an OME-XML description, read as the file states it.
//!
//! The OME-XML shapes read here are ported from the reference reader `tifffile`
//! (Christoph Gohlke, BSD-3-Clause). Keep this attribution.
//!
//! Each `<Image>` is one scene (Bio-Formats calls it a series). This module only
//! collects what the XML says about them. `tiff_ome_geometry` decides what a
//! scene means, `tiff_ome_planes` decides which IFD holds each of its planes,
//! and `tiff_ome_scene` puts the three together.
//!
//! The `<Channel>` children of a `<Pixels>` name the channels and give their
//! display colour.

use std::str::FromStr;

use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;

use crate::error::{Error, Result};
use crate::tiff_ome_planes::MAX_PLANES_PER_SCENE;
use crate::tiff_ome_scene::MAX_PLANES_PER_FILE;
use crate::types::ChannelMeta;

/// One `<TiffData>` of a `<Pixels>`, its attributes kept as the file wrote them.
/// They are parsed by the planes module: an attribute that does not parse must
/// refuse the scene, not read as its default.
#[derive(Debug, Clone, Default)]
pub(crate) struct TiffData {
    pub first_c: Option<String>,
    pub first_t: Option<String>,
    pub first_z: Option<String>,
    pub ifd: Option<String>,
    pub plane_count: Option<String>,
    /// The text of a `<UUID>` child: the identity of the file that holds the planes.
    pub uuid: Option<String>,
}

/// The OME-XML of one file: the document's own UUID and its `<Image>` list.
#[derive(Debug)]
pub(crate) struct OmeXml {
    /// `OME/@UUID`, the identity of this file.
    pub uuid: Option<String>,
    pub images: Vec<Pixels>,
}

/// The `<Pixels>` facts of one `<Image>`, read out of the XML.
#[derive(Debug, Default)]
pub(crate) struct Pixels {
    pub name: Option<String>,
    pub size_x: Option<usize>,
    pub size_y: Option<usize>,
    /// Kept as written: the geometry parses them, so a count that does not parse
    /// refuses the scene instead of reading as absent.
    pub size_z: Option<String>,
    pub size_c: Option<String>,
    pub size_t: Option<String>,
    pub pixel_type: Option<String>,
    pub dim_order: Option<String>,
    pub interleaved: bool,
    pub px_x: Option<f32>,
    pub px_y: Option<f32>,
    pub px_z: Option<f32>,
    pub px_x_unit: Option<String>,
    pub px_y_unit: Option<String>,
    pub px_z_unit: Option<String>,
    pub channels: Vec<ChannelMeta>,
    pub tiff_data: Vec<TiffData>,
}

/// Read a named attribute off a start/empty element as an owned String.
fn attr_str(e: &BytesStart, key: &[u8]) -> Option<String> {
    for a in e.attributes().flatten() {
        if a.key.local_name().as_ref() == key {
            // OME attribute values are plain ASCII numerics/identifiers with
            // no XML entities in practice, so a direct UTF-8 decode is safe.
            return std::str::from_utf8(a.value.as_ref())
                .ok()
                .map(|v| v.to_string());
        }
    }
    None
}

/// A numeric attribute, `None` when it is absent or does not parse.
fn attr_parsed<T: FromStr>(e: &BytesStart, key: &[u8]) -> Option<T> {
    attr_str(e, key).and_then(|v| v.trim().parse().ok())
}

/// Parse an OME `Channel/@Color` (a signed 32-bit RGBA integer) to `[R, G, B]`.
/// A value outside the 32-bit range names no colour, rather than wrapping to one.
fn parse_ome_color(s: &str) -> Option<[u8; 3]> {
    let raw: i64 = s.trim().parse().ok()?;
    if !(i64::from(i32::MIN)..=i64::from(u32::MAX)).contains(&raw) {
        return None;
    }
    let rgba = raw as u32;
    Some([
        ((rgba >> 24) & 0xFF) as u8,
        ((rgba >> 16) & 0xFF) as u8,
        ((rgba >> 8) & 0xFF) as u8,
    ])
}

/// The most `<Image>` elements one file may name. Each one becomes a `Pixels`
/// struct before any scene is built, so a 64 MiB description of empty images
/// must not be able to make that many of them.
pub(crate) const MAX_OME_IMAGES: usize = 65_536;

/// The most `<TiffData>` elements one `<Image>` may name. A file may name one
/// per plane — the plain OME-TIFF encoding — so this cap is the plane budget of
/// one scene ([`MAX_PLANES_PER_SCENE`]): a scene that names one block per plane
/// is read, and only a scene past that budget is refused.
pub(crate) const MAX_OME_TIFF_DATA_PER_IMAGE: usize = MAX_PLANES_PER_SCENE;

/// The most `<Channel>` elements one `<Image>` may name. Every channel of a
/// scene holds at least one plane, so this is the plane budget of one scene.
pub(crate) const MAX_OME_CHANNELS_PER_IMAGE: usize = MAX_PLANES_PER_SCENE;

/// The most `<TiffData>` elements one file may name across all its images: the
/// plane budget of the file ([`MAX_PLANES_PER_FILE`]). One description read at
/// that budget costs about 700 MiB of structs and description, and up to 1 GiB
/// while the block list grows, which is why the cap is the plane budget and no
/// more.
pub(crate) const MAX_OME_TIFF_DATA_PER_FILE: usize = MAX_PLANES_PER_FILE;

/// The most `<Channel>` elements one file may name across all its images: the
/// plane budget of the file ([`MAX_PLANES_PER_FILE`]).
pub(crate) const MAX_OME_CHANNELS_PER_FILE: usize = MAX_PLANES_PER_FILE;

/// Parse the OME-XML of a file: its UUID and every `<Image>`, in file order.
///
/// Only the XML itself can fail here. An `<Image>` whose contents are wrong is
/// returned as it is written, and the scene that needs it refuses it.
pub(crate) fn parse_images(xml: &str) -> Result<OmeXml> {
    let mut reader = Reader::from_str(xml);
    let mut walk = Walk::default();
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(Error::Parse(format!("OME-XML parse error: {e}")));
            }
            Ok(Event::Start(e)) => walk.element(&e, true)?,
            Ok(Event::Empty(e)) => walk.element(&e, false)?,
            Ok(Event::End(e)) => walk.close(e.local_name().as_ref()),
            Ok(Event::Text(t)) => walk.text(t.as_ref()),
            Ok(_) => {}
        }
        if walk.images.len() > MAX_OME_IMAGES {
            return Err(Error::Parse(format!(
                "OME-XML names more than {MAX_OME_IMAGES} <Image> elements; this reader reads at most {MAX_OME_IMAGES}"
            )));
        }
    }
    if walk.images.is_empty() {
        return Err(Error::Parse(
            "OME-XML names no <Image>, so the file states no scene".to_string(),
        ));
    }
    Ok(OmeXml {
        uuid: walk.uuid,
        images: walk.images,
    })
}

/// The element walk: which `<Image>` the current child belongs to.
#[derive(Default)]
struct Walk {
    uuid: Option<String>,
    images: Vec<Pixels>,
    /// The `<TiffData>` elements collected in the file so far.
    tiff_data: usize,
    /// The `<Channel>` elements collected in the file so far.
    channels: usize,
    /// Inside a `<Pixels>` that has children, so its `<Channel>` elements are its own.
    in_pixels: bool,
    /// Inside a `<TiffData>` that has children, so its `<UUID>` is its own.
    in_tiff_data: bool,
    /// Inside a `<UUID>`, so the text that follows is its identity.
    in_uuid: bool,
}

impl Walk {
    /// Handle a start or empty element. `has_children` is false for an empty one,
    /// which has no matching end event.
    fn element(&mut self, e: &BytesStart, has_children: bool) -> Result<()> {
        match e.local_name().as_ref() {
            b"OME" => self.uuid = attr_str(e, b"UUID"),
            b"Image" => {
                let name = attr_str(e, b"Name");
                self.images.push(Pixels { name, ..Pixels::default() });
                self.in_pixels = false;
            }
            b"Pixels" => {
                let Some(image) = self.images.last_mut() else {
                    // A <Pixels> outside any <Image> describes no scene.
                    return Ok(());
                };
                fill_pixels(image, e);
                self.in_pixels = has_children;
            }
            b"Channel" if self.in_pixels => {
                if let Some(image) = self.images.last_mut() {
                    if image.channels.len() >= MAX_OME_CHANNELS_PER_IMAGE {
                        return Err(too_many_children(
                            "Channel",
                            image.channels.len() + 1,
                            MAX_OME_CHANNELS_PER_IMAGE,
                            "one <Image>",
                        ));
                    }
                    if self.channels >= MAX_OME_CHANNELS_PER_FILE {
                        return Err(too_many_children(
                            "Channel",
                            self.channels + 1,
                            MAX_OME_CHANNELS_PER_FILE,
                            "one file",
                        ));
                    }
                    self.channels += 1;
                    let name = attr_str(e, b"Name");
                    let color = attr_str(e, b"Color").and_then(|v| parse_ome_color(&v));
                    image.channels.push(ChannelMeta { name, color });
                }
            }
            b"TiffData" if self.in_pixels => {
                if let Some(image) = self.images.last_mut() {
                    if image.tiff_data.len() >= MAX_OME_TIFF_DATA_PER_IMAGE {
                        return Err(too_many_children(
                            "TiffData",
                            image.tiff_data.len() + 1,
                            MAX_OME_TIFF_DATA_PER_IMAGE,
                            "one <Image>",
                        ));
                    }
                    if self.tiff_data >= MAX_OME_TIFF_DATA_PER_FILE {
                        return Err(too_many_children(
                            "TiffData",
                            self.tiff_data + 1,
                            MAX_OME_TIFF_DATA_PER_FILE,
                            "one file",
                        ));
                    }
                    self.tiff_data += 1;
                    image.tiff_data.push(tiff_data(e));
                    self.in_tiff_data = has_children;
                }
            }
            b"UUID" if self.in_tiff_data => self.in_uuid = has_children,
            _ => {}
        }
        Ok(())
    }

    fn close(&mut self, name: &[u8]) {
        match name {
            b"Pixels" => self.in_pixels = false,
            b"TiffData" => self.in_tiff_data = false,
            b"UUID" => self.in_uuid = false,
            _ => {}
        }
    }

    /// The identity text of the `<UUID>` that is open, kept on its `<TiffData>`.
    fn text(&mut self, raw: &[u8]) {
        if !self.in_uuid {
            return;
        }
        let Ok(text) = std::str::from_utf8(raw) else {
            return;
        };
        let identity = text.trim().to_string();
        if let Some(data) = self.images.last_mut().and_then(|i| i.tiff_data.last_mut()) {
            data.uuid = Some(identity);
        }
    }
}

/// The refusal of a description that names more children of one kind than a
/// cap allows. The element is named, so the reader says which one overflowed.
fn too_many_children(element: &str, count: usize, cap: usize, scope: &str) -> Error {
    Error::Parse(format!(
        "OME-XML names {count} <{element}> elements in {scope}; this reader reads at most {cap}"
    ))
}

/// Copy the `<Pixels>` attributes onto the `<Image>` they belong to.
fn fill_pixels(image: &mut Pixels, e: &BytesStart) {
    image.size_x = attr_parsed(e, b"SizeX");
    image.size_y = attr_parsed(e, b"SizeY");
    image.size_z = attr_str(e, b"SizeZ");
    image.size_c = attr_str(e, b"SizeC");
    image.size_t = attr_str(e, b"SizeT");
    image.pixel_type = attr_str(e, b"Type").or_else(|| attr_str(e, b"PixelType"));
    image.dim_order = attr_str(e, b"DimensionOrder");
    image.interleaved = attr_str(e, b"Interleaved")
        .map(|v| v.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    image.px_x = attr_parsed(e, b"PhysicalSizeX");
    image.px_y = attr_parsed(e, b"PhysicalSizeY");
    image.px_z = attr_parsed(e, b"PhysicalSizeZ");
    image.px_x_unit = attr_str(e, b"PhysicalSizeXUnit");
    image.px_y_unit = attr_str(e, b"PhysicalSizeYUnit");
    image.px_z_unit = attr_str(e, b"PhysicalSizeZUnit");
}

/// The attributes of one `<TiffData>`.
fn tiff_data(e: &BytesStart) -> TiffData {
    TiffData {
        first_c: attr_str(e, b"FirstC"),
        first_t: attr_str(e, b"FirstT"),
        first_z: attr_str(e, b"FirstZ"),
        ifd: attr_str(e, b"IFD"),
        plane_count: attr_str(e, b"PlaneCount"),
        uuid: None,
    }
}

#[cfg(test)]
#[path = "tiff_ome_xml_tests.rs"]
mod tests;
