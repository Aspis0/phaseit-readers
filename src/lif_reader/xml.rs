use quick_xml::events::Event;
use quick_xml::reader::Reader;

use crate::error::{Error, Result};
use crate::xml_entities;

/// Refuses an absurd declared XML length before decoding, so a corrupt header cannot force a huge allocation.
pub(super) const MAX_XML_BYTES: usize = 256 * 1024 * 1024;

const MAX_XML_DEPTH: usize = 256;

/// Text is ignored: the LIF metadata this reader consumes is attribute-based.
pub(super) struct XmlNode {
    pub(super) tag: String,
    pub(super) attrs: Vec<(String, String)>,
    pub(super) children: Vec<XmlNode>,
}

impl XmlNode {
    pub(super) fn new(tag: impl Into<String>) -> Self {
        XmlNode {
            tag: tag.into(),
            attrs: Vec::new(),
            children: Vec::new(),
        }
    }

    pub(super) fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub(super) fn child(&self, tag: &str) -> Option<&XmlNode> {
        self.children.iter().find(|c| c.tag == tag)
    }

    pub(super) fn children_named<'a>(&'a self, tag: &'a str) -> impl Iterator<Item = &'a XmlNode> {
        self.children.iter().filter(move |c| c.tag == tag)
    }
}

pub(super) fn parse_dom(xml: &str) -> Result<XmlNode> {
    let mut reader = Reader::from_str(xml);
    let mut stack: Vec<XmlNode> = vec![XmlNode::new("")];

    let node_from = |e: &quick_xml::events::BytesStart| -> Result<XmlNode> {
        let tag = e.name().into_inner().to_string();
        let mut node = XmlNode::new(tag);
        for a in e.attributes() {
            let a = a.map_err(|err| Error::Parse(format!("LIF XML attribute: {err}")))?;
            let key = a.key.as_ref().to_string();
            let val = xml_entities::attribute(&a);
            node.attrs.push((key, val));
        }
        Ok(node)
    };

    loop {
        match reader
            .read_event()
            .map_err(|e| Error::Parse(format!("LIF XML parse: {e}")))?
        {
            Event::Start(e) => {
                if stack.len() >= MAX_XML_DEPTH {
                    return Err(Error::Parse("LIF XML nesting too deep".to_string()));
                }
                stack.push(node_from(&e)?);
            }
            Event::Empty(e) => {
                let node = node_from(&e)?;
                stack
                    .last_mut()
                    .expect("synthetic root always present")
                    .children
                    .push(node);
            }
            Event::End(_) => {
                // Never pop the synthetic root: more ends than starts = malformed.
                if stack.len() <= 1 {
                    return Err(Error::Parse("LIF XML: unbalanced end tag".to_string()));
                }
                let node = stack.pop().expect("checked len > 1");
                stack
                    .last_mut()
                    .expect("synthetic root always present")
                    .children
                    .push(node);
            }
            Event::Eof => break,
            _ => {}
        }
    }

    if stack.len() != 1 {
        return Err(Error::Parse("LIF XML: unclosed element(s)".to_string()));
    }
    Ok(stack.pop().expect("synthetic root"))
}

/// The container `Version`, which the memory-block framing depends on.
pub(super) fn xml_version(root: &XmlNode) -> Result<u32> {
    let header = root
        .children
        .iter()
        .find(|c| c.tag.starts_with("LMSDataContainer"))
        .ok_or_else(|| Error::Parse("LIF XML: LMSDataContainerHeader not found".to_string()))?;
    let v = header
        .attr("Version")
        .ok_or_else(|| Error::Parse("LIF XML: Version attribute not found".to_string()))?;
    v.trim()
        .parse::<u32>()
        .map_err(|_| Error::Parse(format!("LIF XML: bad Version {v:?}")))
}

/// Every image-bearing Element, pre-order: liffile's `_image_iter` order, parent before children.
pub(super) fn collect_images<'a>(node: &'a XmlNode, out: &mut Vec<&'a XmlNode>) {
    for child in &node.children {
        if child.tag == "Element" {
            let is_image = child
                .child("Data")
                .and_then(|d| d.child("Image"))
                .is_some();
            if is_image {
                out.push(child);
            }
        }
        collect_images(child, out);
    }
}

/// The numeric attribute, or the refusal reason (missing or not an integer), passed on as is.
pub(super) fn attr_u64(node: &XmlNode, key: &str) -> std::result::Result<u64, String> {
    let v = node
        .attr(key)
        .ok_or_else(|| format!("the image states no {key} attribute"))?;
    v.trim().parse::<u64>().map_err(|_| {
        format!("the image states a malformed {key} attribute ({v:?} is not an integer)")
    })
}

pub(super) fn attr_f64(node: &XmlNode, key: &str) -> Option<f64> {
    node.attr(key).and_then(|v| v.trim().parse::<f64>().ok())
}

/// Decodes LE UTF-16 bytes into a `String`.
// Plain `%`: `is_multiple_of` (Rust 1.87) would also work, but the modulo is the shorter spelling here.
#[allow(clippy::manual_is_multiple_of)]
pub(super) fn decode_utf16le(bytes: &[u8]) -> Result<String> {
    if bytes.len() % 2 != 0 {
        return Err(Error::Parse(
            "LIF UTF-16 string has an odd byte length".to_string(),
        ));
    }
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    String::from_utf16(&units).map_err(|_| Error::Parse("LIF: invalid UTF-16 in header".to_string()))
}
