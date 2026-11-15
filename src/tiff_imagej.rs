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

/// The stack shape an ImageJ description states, with the notes for the counts it read as one.
#[derive(Debug, Clone)]
pub struct StackShape {
    pub channels: usize,
    pub slices: usize,
    pub frames: usize,
    /// The notes for the counts the description states as zero.
    pub notes: Vec<String>,
}

impl ImageJLayout {
    /// `None` when the counts do not multiply into the page count: the pages are then read one channel per page.
    pub fn stack_shape(&self, pages: usize) -> Option<StackShape> {
        let (channels, channels_note) = self.count("channels", self.channels, "channel");
        let (slices, slices_note) = self.count("slices", self.slices, "z plane");
        let (frames, frames_note) = self.count("frames", self.frames, "time point");
        let stated = channels.checked_mul(slices)?.checked_mul(frames)?;
        if stated != pages {
            return None;
        }
        if let Some(images) = self.images {
            if images != pages {
                return None;
            }
        }
        Some(StackShape {
            channels,
            slices,
            frames,
            notes: [channels_note, slices_note, frames_note]
                .into_iter()
                .flatten()
                .collect(),
        })
    }

    /// One count of the description: a count of zero is read as one, and the note names the field and the
    /// value the file stated.
    fn count(&self, field: &str, stated: Option<usize>, plane: &str) -> (usize, Option<String>) {
        match stated {
            Some(0) => (
                1,
                Some(format!(
                    "the file states ImageJ {field} as 0; a count of zero is read as one {plane}"
                )),
            ),
            Some(count) => (count, None),
            None => (1, None),
        }
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

    /// The shape the layout makes of a file with this many pages.
    fn shape(layout: &ImageJLayout, pages: usize) -> Option<(usize, usize, usize)> {
        let shape = layout.stack_shape(pages)?;
        Some((shape.channels, shape.slices, shape.frames))
    }

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
        assert_eq!(shape(&layout, 44), Some((4, 11, 1)));
        assert_eq!(shape(&layout, 43), None);
    }

    #[test]
    fn a_description_without_a_layout_leaves_the_shape_to_the_file() {
        let layout = layout("ImageJ=1.53b\nimages=16\nslices=16\n").expect("an ImageJ description");
        assert_eq!(shape(&layout, 16), Some((1, 16, 1)));
    }

    /// A count of zero is read as one, and the note names the field and the value the file stated.
    #[test]
    fn a_count_of_zero_is_read_as_one_and_says_so() {
        let layout = layout("ImageJ=1.54p\nimages=1\nchannels=0\nslices=0\nframes=0\n")
            .expect("an ImageJ description");
        let stack = layout.stack_shape(1).expect("one page, one channel");
        assert_eq!(
            (stack.channels, stack.slices, stack.frames),
            (1, 1, 1),
            "a count of zero is read as one"
        );
        assert_eq!(stack.notes.len(), 3, "each count the file states is named");
        assert!(stack.notes[0].contains("channels"), "{}", stack.notes[0]);
        assert!(stack.notes[1].contains("slices"), "{}", stack.notes[1]);
        assert!(stack.notes[2].contains("frames"), "{}", stack.notes[2]);
        assert!(stack.notes[0].contains("as 0"), "{}", stack.notes[0]);
    }

    #[test]
    fn a_count_the_reader_uses_earns_no_note() {
        let layout = layout(HYPERSTACK).expect("an ImageJ description");
        let stack = layout.stack_shape(44).expect("the hyperstack's shape");
        assert!(stack.notes.is_empty(), "{:?}", stack.notes);
    }
}
