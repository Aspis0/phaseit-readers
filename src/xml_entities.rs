// SPDX-License-Identifier: Apache-2.0
//! Element text arrives split at each entity and is joined here; attributes keep entities until `normalized_value`.

use quick_xml::events::attributes::Attribute;
use quick_xml::XmlVersion;

/// Version is fixed at XML 1.0 (the declaration is never read); unresolvable entities pass through as written.
pub(crate) fn attribute(attr: &Attribute<'_>) -> String {
    match attr.normalized_value(XmlVersion::Implicit1_0) {
        Ok(value) => value.into_owned(),
        Err(_) => attr.value.to_string(),
    }
}

pub(crate) fn reference(content: &str) -> String {
    let raw = format!("&{content};");
    match quick_xml::escape::unescape(&raw) {
        Ok(value) => value.into_owned(),
        Err(_) => raw,
    }
}
