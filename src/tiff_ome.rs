// SPDX-License-Identifier: Apache-2.0
//! The `<Image>` list of an OME-XML description, read as the file states it. Ported from `tifffile`'s OME-XML shapes
//! (Christoph Gohlke, BSD-3-Clause): keep this attribution. `tiff_ome_geometry`, `tiff_ome_planes` and `tiff_ome_scene`
//! interpret it.

use std::str::FromStr;

use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;

use crate::error::{Error, Result};
use crate::tiff_ome_planes::MAX_PLANES_PER_SCENE;
use crate::tiff_ome_scene::MAX_PLANES_PER_FILE;
use crate::types::ChannelMeta;
use crate::xml_entities;

/// One `<TiffData>`, its attributes as the file wrote them; an attribute that does not parse refuses the scene.
#[derive(Debug, Clone, Default)]
pub(crate) struct TiffData {
    pub first_c: Option<String>,
    pub first_t: Option<String>,
    pub first_z: Option<String>,
    pub ifd: Option<String>,
    pub plane_count: Option<String>,
    pub uuid: Option<String>,
}

#[derive(Debug)]
pub(crate) struct OmeXml {
    pub uuid: Option<String>,
    pub images: Vec<Pixels>,
}

#[derive(Debug, Default)]
pub(crate) struct Pixels {
    pub name: Option<String>,
    pub size_x: Option<usize>,
    pub size_y: Option<usize>,
    /// Kept as written: an unparseable count refuses the scene instead of reading as absent.
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

fn attr_str(e: &BytesStart, key: &str) -> Option<String> {
    for a in e.attributes().flatten() {
        if a.key.local_name().as_ref() == key {
            return Some(xml_entities::attribute(&a));
        }
    }
    None
}

fn attr_parsed<T: FromStr>(e: &BytesStart, key: &str) -> Option<T> {
    attr_str(e, key).and_then(|v| v.trim().parse().ok())
}

/// Parses a signed 32-bit RGBA `Channel/@Color`; a value outside 32 bits names no colour.
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

/// Caps `<Image>` elements: each becomes a `Pixels` struct before any scene is built.
pub(crate) const MAX_OME_IMAGES: usize = 65_536;

/// One `<TiffData>` per plane is the plain OME-TIFF encoding, so the cap is the scene's plane budget.
pub(crate) const MAX_OME_TIFF_DATA_PER_IMAGE: usize = MAX_PLANES_PER_SCENE;

/// Every channel holds a plane, so this is the scene's plane budget too.
pub(crate) const MAX_OME_CHANNELS_PER_IMAGE: usize = MAX_PLANES_PER_SCENE;

/// The file's plane budget; a description read at that budget costs about 1 GiB while the block list grows.
pub(crate) const MAX_OME_TIFF_DATA_PER_FILE: usize = MAX_PLANES_PER_FILE;

pub(crate) const MAX_OME_CHANNELS_PER_FILE: usize = MAX_PLANES_PER_FILE;

/// Only the XML can fail here: a wrong `<Image>` is returned as written, and the scene that needs it refuses it.
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
            Ok(Event::CData(c)) => walk.text(c.as_ref()),
            Ok(Event::GeneralRef(r)) => walk.reference(r.as_ref()),
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

#[derive(Default)]
struct Walk {
    uuid: Option<String>,
    images: Vec<Pixels>,
    tiff_data: usize,
    channels: usize,
    /// Inside a `<Pixels>` that has children, so its `<Channel>` elements are its own.
    in_pixels: bool,
    /// Inside a `<TiffData>` that has children, so its `<UUID>` is its own.
    in_tiff_data: bool,
    in_uuid: bool,
    uuid_text: String,
}

impl Walk {
    /// Handles a start or empty element; an empty one has no matching end event.
    fn element(&mut self, e: &BytesStart, has_children: bool) -> Result<()> {
        match e.local_name().as_ref() {
            "OME" => self.uuid = attr_str(e, "UUID"),
            "Image" => {
                let name = attr_str(e, "Name");
                self.images.push(Pixels { name, ..Pixels::default() });
                self.in_pixels = false;
            }
            "Pixels" => {
                let Some(image) = self.images.last_mut() else {
                    // A <Pixels> outside any <Image> describes no scene.
                    return Ok(());
                };
                fill_pixels(image, e);
                self.in_pixels = has_children;
            }
            "Channel" if self.in_pixels => {
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
                    let name = attr_str(e, "Name");
                    let color = attr_str(e, "Color").and_then(|v| parse_ome_color(&v));
                    image.channels.push(ChannelMeta { name, color });
                }
            }
            "TiffData" if self.in_pixels => {
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
            "UUID" if self.in_tiff_data => {
                self.in_uuid = has_children;
                // A `<UUID>` starts empty, so text left by a misnested document cannot attach to it.
                self.uuid_text.clear();
            }
            _ => {}
        }
        Ok(())
    }

    fn close(&mut self, name: &str) {
        match name {
            "Pixels" => self.in_pixels = false,
            "TiffData" => self.in_tiff_data = false,
            "UUID" => {
                // Only a `<UUID>` this walk opened has text to keep.
                if self.in_uuid {
                    self.close_uuid();
                }
                self.in_uuid = false;
            }
            _ => {}
        }
    }

    fn close_uuid(&mut self) {
        let identity = self.uuid_text.trim().to_string();
        self.uuid_text.clear();
        if identity.is_empty() {
            return;
        }
        if let Some(data) = self.images.last_mut().and_then(|i| i.tiff_data.last_mut()) {
            data.uuid = Some(identity);
        }
    }

    fn text(&mut self, raw: &str) {
        if !self.in_uuid {
            return;
        }
        // Entities arrive as events of their own, so the `<UUID>` text is rejoined here.
        self.uuid_text.push_str(raw);
    }

    /// Resolves a reference inside an open `<UUID>` only: resolving every reference in a description costs an
    /// allocation each,
    /// and a free-text description can carry hundreds of thousands.
    fn reference(&mut self, content: &str) {
        if !self.in_uuid {
            return;
        }
        self.uuid_text.push_str(&xml_entities::reference(content));
    }
}

/// The refusal for a description naming more children of one kind than its cap, with the element named.
fn too_many_children(element: &str, count: usize, cap: usize, scope: &str) -> Error {
    Error::Parse(format!(
        "OME-XML names {count} <{element}> elements in {scope}; this reader reads at most {cap}"
    ))
}

fn fill_pixels(image: &mut Pixels, e: &BytesStart) {
    image.size_x = attr_parsed(e, "SizeX");
    image.size_y = attr_parsed(e, "SizeY");
    image.size_z = attr_str(e, "SizeZ");
    image.size_c = attr_str(e, "SizeC");
    image.size_t = attr_str(e, "SizeT");
    image.pixel_type = attr_str(e, "Type").or_else(|| attr_str(e, "PixelType"));
    image.dim_order = attr_str(e, "DimensionOrder");
    image.interleaved = attr_str(e, "Interleaved")
        .map(|v| v.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    image.px_x = attr_parsed(e, "PhysicalSizeX");
    image.px_y = attr_parsed(e, "PhysicalSizeY");
    image.px_z = attr_parsed(e, "PhysicalSizeZ");
    image.px_x_unit = attr_str(e, "PhysicalSizeXUnit");
    image.px_y_unit = attr_str(e, "PhysicalSizeYUnit");
    image.px_z_unit = attr_str(e, "PhysicalSizeZUnit");
}

fn tiff_data(e: &BytesStart) -> TiffData {
    TiffData {
        first_c: attr_str(e, "FirstC"),
        first_t: attr_str(e, "FirstT"),
        first_z: attr_str(e, "FirstZ"),
        ifd: attr_str(e, "IFD"),
        plane_count: attr_str(e, "PlaneCount"),
        uuid: None,
    }
}

#[cfg(test)]
#[path = "tiff_ome_xml_tests.rs"]
mod tests;
