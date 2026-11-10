use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;

use crate::calibration::Calibration;
use crate::error::Result;
use crate::xml_entities;

use super::bytes::{nonneg_usize, rd_i32, segment_id_is};
use super::segment::SEGMENT_HEADER_LEN;

pub(super) const METADATA_FIXED: usize = 256;

/// Upper bound on the metadata XML we will decode (best-effort metadata only).
pub(super) const MAX_XML_BYTES: usize = 256 * 1024 * 1024;

/// The channel names, in order, and the X/Y/Z pixel size in µm. Best-effort: a failure yields defaults,
/// so the metadata never gates decoding.
pub(super) fn parse_metadata(buf: &[u8], meta_pos: usize) -> Result<(Vec<String>, Calibration)> {
    if !segment_id_is(buf, meta_pos, b"ZISRAWMETADATA") {
        return Ok((Vec::new(), Calibration::default()));
    }
    let sd = meta_pos + SEGMENT_HEADER_LEN;
    let xml_size = nonneg_usize(rd_i32(buf, sd)?, "MetadataSize")?;
    if xml_size > MAX_XML_BYTES {
        return Ok((Vec::new(), Calibration::default()));
    }
    let start = sd + METADATA_FIXED;
    let end = match start.checked_add(xml_size).filter(|&e| e <= buf.len()) {
        Some(e) => e,
        None => return Ok((Vec::new(), Calibration::default())),
    };
    let xml = match std::str::from_utf8(&buf[start..end]) {
        Ok(s) => s,
        Err(_) => return Ok((Vec::new(), Calibration::default())),
    };

    let mut reader = Reader::from_str(xml);
    let mut walk = MetadataWalk::default();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => walk.open(&e),
            // Self-closing Channel/Distance still carry attributes.
            Ok(Event::Empty(e)) => {
                walk.note(&e);
            }
            Ok(Event::Text(t)) => walk.text(t.as_ref()),
            Ok(Event::CData(c)) => walk.cdata(c.as_ref()),
            Ok(Event::GeneralRef(r)) => walk.reference(r.as_ref()),
            Ok(Event::End(_)) => walk.close(),
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    Ok((walk.channel_names, calibration_of(walk.px_x, walk.px_y, walk.px_z)))
}

/// The walk over one metadata document: the channel names, the distances, and the open element stack.
#[derive(Default)]
struct MetadataWalk {
    stack: Vec<String>,
    channel_names: Vec<String>,
    cur_dist_id: Option<char>,
    in_value: bool,
    value_text: String,
    px_x: Option<f64>,
    px_y: Option<f64>,
    px_z: Option<f64>,
}

impl MetadataWalk {
    /// Records what a start tag says (a channel name, or a distance axis) and returns its name.
    fn note(&mut self, e: &BytesStart) -> String {
        let tag = e.name().into_inner().to_string();
        if tag == "Channel" && self.stack.iter().any(|t| t == "Dimensions") {
            if let Some(name) = attr_value(e, "Name") {
                self.channel_names.push(name);
            }
        }
        if tag == "Distance" {
            self.cur_dist_id = attr_value(e, "Id").and_then(|v| v.chars().next());
        }
        tag
    }

    fn open(&mut self, e: &BytesStart) {
        let tag = self.note(e);
        self.in_value = tag == "Value" && self.stack.last().map(String::as_str) == Some("Distance");
        self.stack.push(tag);
    }

    fn close(&mut self) {
        let Some(tag) = self.stack.pop() else {
            return;
        };
        if tag == "Distance" {
            self.cur_dist_id = None;
        }
        if tag == "Value" {
            self.in_value = false;
            if let Some((id, v)) = distance_value(self.cur_dist_id, &self.value_text) {
                match id {
                    'X' => self.px_x = Some(v),
                    'Y' => self.px_y = Some(v),
                    'Z' => self.px_z = Some(v),
                    _ => {}
                }
            }
            self.value_text.clear();
        }
    }

    /// Entities in text arrive as events of their own, so the text of a `<Value>` comes in pieces.
    fn text(&mut self, s: &str) {
        if self.in_value {
            self.value_text.push_str(s);
        }
    }

    /// CDATA is literal text: it is not escaped, so it is not resolved.
    fn cdata(&mut self, s: &str) {
        if self.in_value {
            self.value_text.push_str(s);
        }
    }

    fn reference(&mut self, content: &str) {
        if self.in_value {
            self.value_text.push_str(&xml_entities::reference(content));
        }
    }
}

/// Distances are in metres, converted to micrometres; both X and Y must be positive for a calibration to exist.
fn calibration_of(px_x: Option<f64>, px_y: Option<f64>, px_z: Option<f64>) -> Calibration {
    match (px_x, px_y) {
        (Some(x), Some(y)) if x > 0.0 && y > 0.0 => Calibration::sized(
            (x * 1e6) as f32,
            (y * 1e6) as f32,
            px_z.map(|v| (v * 1e6) as f32),
        ),
        _ => Calibration::default(),
    }
}

/// Metres, not converted to micrometres.
fn distance_value(id: Option<char>, text: &str) -> Option<(char, f64)> {
    let id = id?;
    let v = text.trim().parse::<f64>().ok()?;
    Some((id, v))
}

fn attr_value(e: &BytesStart, key: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .map(|a| xml_entities::attribute(&a))
}
