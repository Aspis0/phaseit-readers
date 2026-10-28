// SPDX-License-Identifier: Apache-2.0
//! Writes the small LIF containers the region tests read back.
//!
//! The container is written by hand (the reader's module docs spell the
//! framing out): a header, the UTF-16LE metadata XML, then one memory block
//! per image whose pixels live in this file.
//!
//! The memory block holds the image's frames in the file's own order — `[T]`
//! outermost, then `[Z]`, then the channels, then the rows — and the XML
//! states the matching `BytesInc` strides, so the reader's offset arithmetic
//! addresses the same bytes liffile would.

use std::path::Path;

/// One image of a synthetic LIF.
pub struct LifImage {
    /// The element's `Name` attribute.
    pub name: String,
    /// The memory block the image names.
    pub block_id: String,
    /// The image's X extent in pixels.
    pub x: usize,
    /// The image's Y extent in pixels.
    pub y: usize,
    /// The image's Z extent (planes), or `None` when it has no Z dimension.
    pub z: Option<usize>,
    /// The image's T extent (time points), or `None` when it has no T dimension.
    pub t: Option<usize>,
    /// Bytes per sample: 1 (U8), 2 (U16) or 4 (F32).
    pub itemsize: usize,
    /// The number of channels (the `ChannelDescription` list).
    pub channels: usize,
    /// The memory block's bytes, in the file's order.
    pub pixels: Vec<u8>,
    /// Whether the image's data block is in THIS file. `false` is the
    /// ext-resident case: the pixels live in the `.lifext` companion, so this
    /// file carries no block for them.
    pub resident: bool,
    /// The image's `<Dimensions>`/`<Channels>` description, used VERBATIM
    /// instead of the one the writer would generate from the geometry. A test
    /// that needs a malformed, ambiguous or unmodelled description states it
    /// here.
    pub body: Option<String>,
}

impl LifImage {
    /// A grayscale U8 image of `x` x `y` pixels whose samples follow the
    /// pattern `sample(f, y, x) = (f * 37 + y * 7 + x) as u8`, where `f`
    /// numbers the frames in the file's order (T, then Z, then channel).
    pub fn gray8(
        name: &str,
        block_id: &str,
        x: usize,
        y: usize,
        z: Option<usize>,
        t: Option<usize>,
        channels: usize,
    ) -> LifImage {
        let frames = t.unwrap_or(1) * z.unwrap_or(1) * channels;
        let mut pixels = Vec::with_capacity(frames * x * y);
        for f in 0..frames {
            for row in 0..y {
                for col in 0..x {
                    pixels.push((f * 37 + row * 7 + col) as u8);
                }
            }
        }
        LifImage {
            name: name.to_string(),
            block_id: block_id.to_string(),
            x,
            y,
            z,
            t,
            itemsize: 1,
            channels,
            pixels,
            resident: true,
            body: None,
        }
    }

    /// The image's pixels live in the `.lifext` companion, not in this file:
    /// the writer writes no block for them.
    pub fn ext_resident(mut self) -> LifImage {
        self.resident = false;
        self
    }

    /// Use `body` as the image's `<Dimensions>`/`<Channels>` description,
    /// instead of the one generated from the geometry.
    pub fn with_body(mut self, body: &str) -> LifImage {
        self.body = Some(body.to_string());
        self
    }

    /// The bytes of one frame of a U8 image, at `(t, z, channel)`.
    pub fn frame(&self, t: usize, z: usize, channel: usize) -> &[u8] {
        let frame = self.x * self.y;
        let z_index = z * self.channels + channel;
        let index = t * self.z.unwrap_or(1) * self.channels + z_index;
        &self.pixels[index * frame..(index + 1) * frame]
    }
}

/// The `DataType` / `Resolution` pair the reader maps to the item size.
fn pixel_type(itemsize: usize) -> (u32, u32) {
    match itemsize {
        1 => (0, 8),
        2 => (0, 16),
        4 => (1, 32),
        other => panic!("unsupported item size {other}"),
    }
}

/// The XML of one image element: its description and the block it names.
fn image_element(image: &LifImage) -> String {
    // A test may state the description verbatim; otherwise it is generated
    // from the image's geometry.
    let description = match &image.body {
        Some(body) => body.clone(),
        None => generated_description(image),
    };

    format!(
        r#"  <Element Name="{name}" Type="Image">
    <Data>
      <Image>
        <ImageDescription>
{description}
        </ImageDescription>
      </Image>
    </Data>
    <Memory MemoryBlockID="{block}"/>
  </Element>
"#,
        name = image.name,
        description = description,
        block = image.block_id
    )
}

/// The `<Dimensions>` and `<Channels>` the writer generates from the image's
/// geometry.
fn generated_description(image: &LifImage) -> String {
    let frame = image.x * image.y * image.itemsize;
    let row = image.x * image.itemsize;
    // One `(t,z)` slab holds every channel of that plane.
    let slab = image.channels * frame;

    let mut dimensions = String::new();
    dimensions.push_str(&format!(
        r#"<DimensionDescription DimID="1" NumberOfElements="{}" BytesInc="{}" Length="0.000004"/>"#,
        image.x, image.itemsize
    ));
    dimensions.push_str(&format!(
        r#"<DimensionDescription DimID="2" NumberOfElements="{}" BytesInc="{}" Length="0.000002"/>"#,
        image.y, row
    ));
    if let Some(z) = image.z {
        dimensions.push_str(&format!(
            r#"<DimensionDescription DimID="3" NumberOfElements="{}" BytesInc="{}" Length="0.000100"/>"#,
            z, slab
        ));
    }
    if let Some(t) = image.t {
        let t_span = match image.z {
            Some(z) => z * slab,
            None => slab,
        };
        dimensions.push_str(&format!(
            r#"<DimensionDescription DimID="4" NumberOfElements="{}" BytesInc="{}" Length="0.000100"/>"#,
            t, t_span
        ));
    }

    let (data_type, resolution) = pixel_type(image.itemsize);
    let mut channels = String::new();
    for c in 0..image.channels {
        channels.push_str(&format!(
            r#"<ChannelDescription DataType="{data_type}" Resolution="{resolution}" BytesInc="{}" LUTName=""/>"#,
            c * frame
        ));
    }

    format!(
        "          <Dimensions>{dimensions}</Dimensions>\n          <Channels>{channels}</Channels>",
        dimensions = dimensions,
        channels = channels
    )
}

/// Write a LIF holding `images`; each RESIDENT image gets its own memory
/// block. An image that is not resident gets no block: its pixels live in the
/// `.lifext` companion.
pub fn write_lif(path: &Path, images: &[LifImage]) {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="utf-8"?>
<LMSDataContainerHeader Version="2">
"#,
    );
    for image in images {
        xml.push_str(&image_element(image));
    }
    xml.push_str("</LMSDataContainerHeader>\n");

    let utf16: Vec<u16> = xml.encode_utf16().collect();
    let mut buf: Vec<u8> = Vec::new();
    // The container header: id0 | descSize | id1 | the XML's length in chars.
    buf.extend_from_slice(&0x70u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.push(0x2A);
    buf.extend_from_slice(&(utf16.len() as u32).to_le_bytes());
    for unit in &utf16 {
        buf.extend_from_slice(&unit.to_le_bytes());
    }

    // One memory block per RESIDENT image: id0 | descSize | id1 | size | id2 | id | data.
    for image in images.iter().filter(|i| i.resident) {
        let id: Vec<u16> = image.block_id.encode_utf16().collect();
        buf.extend_from_slice(&0x70u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.push(0x2A);
        buf.extend_from_slice(&(image.pixels.len() as u64).to_le_bytes());
        buf.push(0x2A);
        buf.extend_from_slice(&(id.len() as u32).to_le_bytes());
        for unit in &id {
            buf.extend_from_slice(&unit.to_le_bytes());
        }
        buf.extend_from_slice(&image.pixels);
    }

    std::fs::write(path, &buf).expect("write the synthetic LIF");
}
