use crate::error::{Error, Result};

use super::bytes::{position_in_file, rd_i64, segment_id_is};

pub(super) const SEGMENT_HEADER_LEN: usize = 32;

pub(super) fn parse_file_header(buf: &[u8]) -> Result<(usize, usize)> {
    if !segment_id_is(buf, 0, b"ZISRAWFILE") {
        return Err(Error::Parse(
            "CZI: first segment is not ZISRAWFILE".to_string(),
        ));
    }
    let sd = SEGMENT_HEADER_LEN;
    let dir_pos = rd_i64(buf, sd + 52)?;
    let meta_pos = rd_i64(buf, sd + 60)?;
    let dir_pos = position_in_file(dir_pos, buf.len(), "DirectoryPosition")?;
    let meta_pos = position_in_file(meta_pos, buf.len(), "MetadataPosition")?;
    Ok((dir_pos, meta_pos))
}
