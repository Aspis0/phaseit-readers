// SPDX-License-Identifier: Apache-2.0
//! The channel-colours block of a Zeiss LSM file: the display colours and the
//! channel names it carries.
//!
//! The block sits at the file offset CZ-LSM-Info states at offset 108. Its
//! first 20 bytes are a header; at +12 it holds the offset *within the block*
//! of the colour table and at +16 the offset of the name table.
//!
//! * The **colour table** holds one colour word per channel: three bytes of
//!   red, green and blue, then a zero byte. Every file of the owner's corpus
//!   stores that zero byte, and the colours are the pure red, green, blue,
//!   white and black a Zeiss console displays.
//! * The **name table** holds one record per channel: an `int32` length and
//!   that many bytes — the name, with the NUL Zeiss terminates it with.
//!
//! The strings are Zeiss' own and carry the acquisition's suffixes: a
//! three-timepoint scan names its channels `Ch3-T1`, `Ch1-T2` and
//! `Ch2 GaAsP-T3`. The ScanInfo's detection-channel names are the bare `Ch3`,
//! `Ch1` and `Ch2 GaAsP`.
//!
//! The block is optional metadata, and a file that states none — or one whose
//! block does not fit the file — opens with unnamed, colourless channels
//! rather than failing to open.

use crate::lsm_bytes::read_i32;
use crate::types::{ChannelMeta, Endian};

/// Offset, within the channel-colours block, of the two i32s that point at
/// the colour table and at the name table.
const TABLE_POINTERS: usize = 12;

/// Bytes one colour word takes: red, green, blue and a zero byte.
const COLOR_WORD: usize = 4;

/// Upper bound on the length of a name this reader publishes. A real channel
/// name is a few dozen bytes; a longer one is not a name this reader can make
/// sense of, and the channel it belongs to stays unnamed.
const MAX_NAME_BYTES: usize = 1024;

/// The per-channel metadata a file's channel-colours block carries.
///
/// A channel the block says nothing about keeps its default metadata: no name
/// and no colour.
pub fn read_channel_meta(
    buf: &[u8],
    block_offset: usize,
    endian: Endian,
    channels: usize,
) -> Vec<ChannelMeta> {
    let mut meta = vec![ChannelMeta::default(); channels];
    if channels == 0 {
        return meta;
    }

    // The two table pointers live at +12 and +16. A block that does not fit
    // the file states no tables, and the file opens with unnamed channels.
    let Some(pointers) = block_offset.checked_add(TABLE_POINTERS) else {
        return meta;
    };
    let (Ok(color_table), Ok(name_table)) = (
        read_i32(buf, pointers, endian),
        read_i32(buf, pointers + 4, endian),
    ) else {
        return meta;
    };

    for (channel, meta) in meta.iter_mut().enumerate() {
        meta.color = read_color(buf, block_offset, color_table, channel);
    }
    for (channel, name) in read_names(buf, block_offset, name_table, endian, channels)
        .into_iter()
        .enumerate()
    {
        meta[channel].name = name;
    }
    meta
}

/// The display colour of one channel: the three colour bytes of its colour
/// word, or `None` when the file states no colour for it.
fn read_color(buf: &[u8], block_offset: usize, color_table: i32, channel: usize) -> Option<[u8; 3]> {
    if color_table <= 0 {
        return None;
    }
    let at = block_offset
        .checked_add(color_table as usize)?
        .checked_add(channel.checked_mul(COLOR_WORD)?)?;
    let word = buf.get(at..at + COLOR_WORD)?;
    Some([word[0], word[1], word[2]])
}

/// The channel names, in channel order.
///
/// The name table ends where the file ends: a record that reaches past the
/// last byte of the file ends the table, and the channels after it stay
/// unnamed. A record whose length is beyond what a name may be is skipped —
/// that channel stays unnamed and the next one is still read.
fn read_names(
    buf: &[u8],
    block_offset: usize,
    name_table: i32,
    endian: Endian,
    channels: usize,
) -> Vec<Option<String>> {
    let mut names = vec![None; channels];
    if name_table <= 0 {
        return names;
    }
    let Some(mut at) = block_offset.checked_add(name_table as usize) else {
        return names;
    };

    for name in names.iter_mut() {
        let Some(length) = read_i32(buf, at, endian).ok() else {
            break;
        };
        at += 4;
        if length < 0 {
            break;
        }
        let length = length as usize;
        let Some(end) = at.checked_add(length) else {
            break;
        };
        let Some(bytes) = buf.get(at..end) else {
            break;
        };
        at = end;
        if length > MAX_NAME_BYTES {
            continue;
        }
        *name = Some(decode_name(bytes));
    }
    names
}

/// One name record's bytes: UTF-8, without the trailing NULs Zeiss writes.
///
/// A name that is empty stays empty: the file states a channel with no name,
/// which is not the same as a channel the file says nothing about.
fn decode_name(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .rposition(|&b| b != 0)
        .map_or(0, |last| last + 1);
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where the test blocks put their colour table.
    const COLOR_TABLE_AT: usize = 40;

    /// The colours the test blocks write, one per channel.
    const COLORS: [[u8; 3]; 3] = [[255, 0, 0], [0, 255, 0], [0, 0, 255]];

    /// A channel-colours block with `names` at its name table and one colour
    /// word per channel.
    fn block(names: &[&str]) -> Vec<u8> {
        let mut records = Vec::new();
        for name in names {
            let mut bytes = name.as_bytes().to_vec();
            bytes.push(0);
            records.extend_from_slice(&(bytes.len() as i32).to_le_bytes());
            records.extend_from_slice(&bytes);
        }
        let name_table_at = COLOR_TABLE_AT + COLOR_WORD * names.len();
        let mut buf = vec![0u8; name_table_at + records.len()];
        buf[12..16].copy_from_slice(&(COLOR_TABLE_AT as i32).to_le_bytes());
        buf[16..20].copy_from_slice(&(name_table_at as i32).to_le_bytes());
        for channel in 0..names.len() {
            let at = COLOR_TABLE_AT + channel * COLOR_WORD;
            buf[at..at + 3].copy_from_slice(&COLORS[channel % COLORS.len()]);
        }
        buf[name_table_at..].copy_from_slice(&records);
        buf
    }

    #[test]
    fn the_names_are_read_in_channel_order() {
        let buf = block(&["Ch3-T1", "Ch1-T2", "Ch2 GaAsP-T3"]);
        let meta = read_channel_meta(&buf, 0, Endian::Little, 3);
        let names: Vec<Option<&str>> = meta.iter().map(|m| m.name.as_deref()).collect();
        assert_eq!(
            names,
            vec![Some("Ch3-T1"), Some("Ch1-T2"), Some("Ch2 GaAsP-T3")]
        );
    }

    #[test]
    fn a_trailing_nul_is_stripped() {
        let buf = block(&["Ch2 GaAsP"]);
        let meta = read_channel_meta(&buf, 0, Endian::Little, 1);
        assert_eq!(meta[0].name.as_deref(), Some("Ch2 GaAsP"));
    }

    #[test]
    fn an_empty_name_stays_empty() {
        // The file states a channel with no name, which is not the same as a
        // channel the file says nothing about.
        let buf = block(&[""]);
        let meta = read_channel_meta(&buf, 0, Endian::Little, 1);
        assert_eq!(meta[0].name.as_deref(), Some(""));
    }

    #[test]
    fn a_too_long_name_is_skipped_and_the_next_one_is_read() {
        let long = "x".repeat(MAX_NAME_BYTES + 1);
        let buf = block(&[&long, "Ch2 GaAsP"]);
        let meta = read_channel_meta(&buf, 0, Endian::Little, 2);
        assert_eq!(meta[0].name, None, "the long name is skipped");
        assert_eq!(meta[1].name.as_deref(), Some("Ch2 GaAsP"));
    }

    #[test]
    fn only_as_many_names_as_the_file_holds_are_returned() {
        let buf = block(&["Ch2 GaAsP"]);
        let meta = read_channel_meta(&buf, 0, Endian::Little, 3);
        assert_eq!(meta[0].name.as_deref(), Some("Ch2 GaAsP"));
        assert_eq!(meta[1].name, None);
        assert_eq!(meta[2].name, None);
    }

    #[test]
    fn the_colour_words_are_read_per_channel() {
        let buf = block(&["Ch1", "Ch2"]);
        let meta = read_channel_meta(&buf, 0, Endian::Little, 2);
        assert_eq!(meta[0].color, Some([255, 0, 0]));
        assert_eq!(meta[1].color, Some([0, 255, 0]));
    }

    #[test]
    fn a_block_without_a_name_table_yields_no_names() {
        let mut buf = vec![0u8; 32];
        buf[16..20].copy_from_slice(&0i32.to_le_bytes()); // namesOffset = 0
        let meta = read_channel_meta(&buf, 0, Endian::Little, 2);
        assert!(meta.iter().all(|m| m.name.is_none()));
    }

    #[test]
    fn a_name_table_past_the_end_of_the_file_yields_no_names() {
        let mut buf = vec![0u8; 32];
        buf[16..20].copy_from_slice(&9000i32.to_le_bytes());
        let meta = read_channel_meta(&buf, 0, Endian::Little, 2);
        assert!(meta.iter().all(|m| m.name.is_none()));
    }

    #[test]
    fn a_block_that_does_not_fit_the_file_yields_nothing() {
        // The block starts near the end of the file: its table pointers are
        // past the last byte, so the file opens with unnamed channels.
        let buf = vec![0u8; 32];
        let meta = read_channel_meta(&buf, 20, Endian::Little, 2);
        assert!(meta.iter().all(|m| m.name.is_none() && m.color.is_none()));
    }
}
