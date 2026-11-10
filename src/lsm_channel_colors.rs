// SPDX-License-Identifier: Apache-2.0
//! The channel-colours block of a Zeiss LSM file: each channel's display colour and name.
//! Colours are one 4-byte word per channel; names are an `int32` length and that many bytes, NUL-terminated by Zeiss.
//! The block is optional metadata: one that is missing or does not fit the file leaves the channels unnamed and
//! colourless.

use crate::lsm_bytes::read_i32;
use crate::types::{ChannelMeta, Endian};

const TABLE_POINTERS: usize = 12;

/// Bytes of one colour word: red, green, blue and the zero byte the corpus always stores.
const COLOR_WORD: usize = 4;

/// The longest name the reader publishes: a longer record is not usable, and its channel stays unnamed.
const MAX_NAME_BYTES: usize = 1024;

/// A channel the block does not describe keeps its default: no name, no colour.
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

    // A block that does not fit the file states no tables: the channels stay unnamed.
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

/// A record reaching past the file's end ends the table; one too long for a name is skipped, and reading continues.
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

/// An empty name is a stated channel with no name, which is not a channel the file says nothing about.
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

    const COLOR_TABLE_AT: usize = 40;

    const COLORS: [[u8; 3]; 3] = [[255, 0, 0], [0, 255, 0], [0, 0, 255]];

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
        buf[16..20].copy_from_slice(&0i32.to_le_bytes());
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
        let buf = vec![0u8; 32];
        let meta = read_channel_meta(&buf, 20, Endian::Little, 2);
        assert!(meta.iter().all(|m| m.name.is_none() && m.color.is_none()));
    }
}
