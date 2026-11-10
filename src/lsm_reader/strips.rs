#[derive(Debug, Clone)]
pub(super) struct PageStrips {
    /// Absolute offsets in TIFF strip order: planar, so channel 0's strips come first.
    pub(super) offsets: Vec<u64>,
    pub(super) bytes: Vec<u64>,
}

/// The native window path needs each strip to hold whole rows and the channel's strips to cover the plane's rows;
/// the default path needs nothing of the strips.
pub(super) fn strips_hold_whole_rows(
    page: &PageStrips,
    strips_per_channel: usize,
    channels: usize,
    row_bytes: usize,
    plane_rows: usize,
) -> bool {
    if row_bytes == 0 {
        return false;
    }
    for channel in 0..channels {
        let start = match channel.checked_mul(strips_per_channel) {
            Some(start) => start,
            None => return false,
        };
        let end = match start.checked_add(strips_per_channel) {
            Some(end) => end,
            None => return false,
        };
        if end > page.bytes.len() {
            return false;
        }
        let mut rows = 0usize;
        for s in start..end {
            let count = match usize::try_from(page.bytes[s]) {
                Ok(count) => count,
                Err(_) => return false,
            };
            if count % row_bytes != 0 {
                return false;
            }
            rows += count / row_bytes;
        }
        if rows != plane_rows {
            return false;
        }
    }
    true
}
