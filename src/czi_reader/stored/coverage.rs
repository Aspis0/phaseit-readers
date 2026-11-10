//! The part of the scene a set of tiles covers: the bands of rows and the intervals of columns
//! they cover. It is what the tiles leave out that the fill fills.

use crate::types::PixelRect;

/// A rectangle of source pixels.
pub(super) struct Area {
    /// The left edge, in source pixels.
    pub x: i64,
    /// The top edge, in source pixels.
    pub y: i64,
    /// The width, in source pixels.
    pub w: i64,
    /// The height, in source pixels.
    pub h: i64,
}

/// The part of the scene a level's tiles cover.
///
/// WHY row bands: the tiles are rectangles of many sizes, so the covered area is the union of
/// their extents; a scan over the rows of that union names it exactly.
pub(super) struct Coverage {
    /// The bands of the covered area, sorted by their top edge.
    bands: Vec<Band>,
    /// The top edge of the covered area, in source pixels.
    top: i64,
    /// The bottom edge of the covered area, in source pixels.
    bottom: i64,
}

/// One band of the covered area: the rows between two edges, and the x-intervals they cover.
struct Band {
    y0: i64,
    y1: i64,
    /// The covered x-intervals, sorted and disjoint.
    covered: Vec<(i64, i64)>,
}

impl Coverage {
    /// The coverage of a plane that has no tile.
    pub(super) const fn empty() -> Coverage {
        Coverage {
            bands: Vec::new(),
            top: 0,
            bottom: 0,
        }
    }

    /// The area the tiles of one plane of a level cover.
    pub(super) fn of(areas: &[Area]) -> Coverage {
        let mut edges: Vec<i64> = Vec::with_capacity(areas.len() * 2);
        for area in areas {
            edges.push(area.y);
            edges.push(area.y + area.h);
        }
        edges.sort_unstable();
        edges.dedup();

        let mut order: Vec<usize> = (0..areas.len()).collect();
        order.sort_by_key(|&index| areas[index].y);

        let mut bands: Vec<Band> = Vec::new();
        let mut active: Vec<usize> = Vec::new();
        let mut next = 0;
        for pair in edges.windows(2) {
            let (y0, y1) = (pair[0], pair[1]);
            while next < order.len() && areas[order[next]].y <= y0 {
                active.push(order[next]);
                next += 1;
            }
            active.retain(|&index| areas[index].y + areas[index].h > y0);
            let mut intervals: Vec<(i64, i64)> = active
                .iter()
                .map(|&index| {
                    let area = &areas[index];
                    (area.x, area.x + area.w)
                })
                .collect();
            intervals.sort_unstable();
            bands.push(Band {
                y0,
                y1,
                covered: merge(intervals),
            });
        }
        let top = edges.first().copied().unwrap_or(0);
        let bottom = edges.last().copied().unwrap_or(0);
        Coverage { bands, top, bottom }
    }

    /// The same coverage, measured from another corner.
    pub(super) fn shifted(&self, dx: i64, dy: i64) -> Coverage {
        Coverage {
            bands: self
                .bands
                .iter()
                .map(|band| Band {
                    y0: band.y0 + dy,
                    y1: band.y1 + dy,
                    covered: band
                        .covered
                        .iter()
                        .map(|&(x0, x1)| (x0 + dx, x1 + dx))
                        .collect(),
                })
                .collect(),
            top: self.top + dy,
            bottom: self.bottom + dy,
        }
    }

    /// The parts of `rect` the tiles do not cover.
    pub(super) fn uncovered(&self, rect: PixelRect) -> Vec<PixelRect> {
        let rect_x0 = rect.x as i64;
        let rect_y0 = rect.y as i64;
        let rect_x1 = rect_x0 + rect.w as i64;
        let rect_y1 = rect_y0 + rect.h as i64;
        let mut out = Vec::new();

        // The rows above the level and below it are uncovered whole.
        if rect_y0 < self.top {
            let height = (self.top - rect_y0) as usize;
            out.push(PixelRect::new(rect.x, rect.y, rect.w, height));
        }
        if rect_y1 > self.bottom {
            let y = self.bottom.max(rect_y0) as usize;
            let height = (rect_y1 - self.bottom.max(rect_y0)) as usize;
            out.push(PixelRect::new(rect.x, y, rect.w, height));
        }
        for band in &self.bands {
            let y0 = band.y0.max(rect_y0);
            let y1 = band.y1.min(rect_y1);
            if y0 >= y1 {
                continue;
            }
            for (x0, x1) in gaps(&band.covered, rect_x0, rect_x1) {
                out.push(PixelRect::new(
                    x0 as usize,
                    y0 as usize,
                    (x1 - x0) as usize,
                    (y1 - y0) as usize,
                ));
            }
        }
        out
    }

    /// The parts of `rect` the tiles cover.
    pub(super) fn covered(&self, rect: PixelRect) -> Vec<PixelRect> {
        let rect_x0 = rect.x as i64;
        let rect_x1 = rect_x0 + rect.w as i64;
        let mut out = Vec::new();
        for band in &self.bands {
            let y0 = band.y0.max(rect.y as i64);
            let y1 = band.y1.min((rect.y + rect.h) as i64);
            if y0 >= y1 {
                continue;
            }
            for (x0, x1) in covered(&band.covered, rect_x0, rect_x1) {
                out.push(PixelRect::new(
                    x0 as usize,
                    y0 as usize,
                    (x1 - x0) as usize,
                    (y1 - y0) as usize,
                ));
            }
        }
        out
    }

    /// The covered area within `rect`, in source pixels.
    pub(super) fn area(&self, rect: PixelRect) -> usize {
        let rect_x0 = rect.x as i64;
        let rect_x1 = rect_x0 + rect.w as i64;
        let mut total = 0usize;
        for band in &self.bands {
            let y0 = band.y0.max(rect.y as i64);
            let y1 = band.y1.min((rect.y + rect.h) as i64);
            if y0 >= y1 {
                continue;
            }
            for (x0, x1) in covered(&band.covered, rect_x0, rect_x1) {
                total += ((x1 - x0) * (y1 - y0)) as usize;
            }
        }
        total
    }
}

/// The intervals of `covered` that fall inside `[x0, x1)`.
fn covered(covered: &[(i64, i64)], x0: i64, x1: i64) -> Vec<(i64, i64)> {
    covered
        .iter()
        .filter_map(|&(start, end)| {
            let start = start.max(x0);
            let end = end.min(x1);
            (start < end).then_some((start, end))
        })
        .collect()
}

/// The intervals of `covered` that fall outside `[x0, x1)`.
fn gaps(covered: &[(i64, i64)], x0: i64, x1: i64) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    let mut cursor = x0;
    for &(start, end) in covered {
        if end <= cursor {
            continue;
        }
        if start >= x1 {
            break;
        }
        if start > cursor {
            out.push((cursor, start.min(x1)));
        }
        cursor = cursor.max(end);
        if cursor >= x1 {
            break;
        }
    }
    if cursor < x1 {
        out.push((cursor, x1));
    }
    out
}

/// The disjoint intervals of a sorted list of intervals.
fn merge(intervals: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    let mut out: Vec<(i64, i64)> = Vec::with_capacity(intervals.len());
    for (start, end) in intervals {
        match out.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => out.push((start, end)),
        }
    }
    out
}
