// SPDX-License-Identifier: Apache-2.0
//! ImageJ's description (tag 270) states the stack layout. Pages are ordered CHANNEL-FIRST
//! (`c + z * channels + t * channels * slices`), the order Bio-Formats reports as `XYCZT`, so both readers agree.

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImageJLayout {
    pub images: Option<usize>,
    pub channels: Option<usize>,
    pub slices: Option<usize>,
    pub frames: Option<usize>,
    /// The stack mode (`mode`): "color", "composite", "grayscale".
    pub mode: Option<String>,
}

impl ImageJLayout {
    /// `None` when the counts do not multiply into the page count: the pages are then read one channel per page.
    pub fn stack_shape(&self, pages: usize) -> Option<(usize, usize, usize)> {
        let channels = self.channels.unwrap_or(1).max(1);
        let slices = self.slices.unwrap_or(1).max(1);
        let frames = self.frames.unwrap_or(1).max(1);
        let stated = channels.checked_mul(slices)?.checked_mul(frames)?;
        if stated != pages {
            return None;
        }
        if let Some(images) = self.images {
            if images != pages {
                return None;
            }
        }
        Some((channels, slices, frames))
    }
}

pub(crate) fn layout(description: &str) -> Option<ImageJLayout> {
    let mut lines = description.lines();
    let first = lines.next()?.trim();
    if !first.starts_with("ImageJ=") {
        return None;
    }

    let mut layout = ImageJLayout::default();
    for line in lines {
        let line = line.trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "images" => layout.images = value.parse().ok(),
            "channels" => layout.channels = value.parse().ok(),
            "slices" => layout.slices = value.parse().ok(),
            "frames" => layout.frames = value.parse().ok(),
            "mode" => layout.mode = Some(value.to_string()),
            _ => {}
        }
    }
    Some(layout)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HYPERSTACK: &str = "ImageJ=1.54p\nimages=44\nchannels=4\nslices=11\nhyperstack=true\nmode=composite\nunit=\u{b5}m\nspacing=0.9994\n";

    #[test]
    fn a_description_that_is_not_imagej_has_no_layout() {
        assert!(layout("<OME xmlns=\"http://www.openmicroscopy.org/Schemas/OME/2016-06\"/>").is_none());
        assert!(layout("").is_none());
    }

    #[test]
    fn the_hyperstack_layout_is_read() {
        let layout = layout(HYPERSTACK).expect("an ImageJ description");
        assert_eq!(layout.images, Some(44));
        assert_eq!(layout.channels, Some(4));
        assert_eq!(layout.slices, Some(11));
        assert_eq!(layout.frames, None);
        assert_eq!(layout.mode.as_deref(), Some("composite"));
    }

    #[test]
    fn a_layout_that_does_not_multiply_into_the_pages_is_refused() {
        let layout = layout(HYPERSTACK).expect("an ImageJ description");
        assert_eq!(layout.stack_shape(44), Some((4, 11, 1)));
        assert_eq!(layout.stack_shape(43), None);
    }

    #[test]
    fn a_description_without_a_layout_leaves_the_shape_to_the_file() {
        let layout = layout("ImageJ=1.53b\nimages=16\nslices=16\n").expect("an ImageJ description");
        assert_eq!(layout.stack_shape(16), Some((1, 16, 1)));
    }
}
