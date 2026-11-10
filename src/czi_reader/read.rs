use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use crate::error::{Error, Result};
use crate::types::{checked_region, empty_region_plane, Dims5, Endian, PixelRect, Plane, PlaneBuf};

use super::codec::{bgr_to_rgb, zstd1_decode, zstd_raw_decode};
use super::mosaic::{blit_tile, blit_tile_clipped};
use super::scene::CziScene;
use super::subblock::SubBlock;
use super::CziReader;

/// Where one tile overlaps the requested rectangle, in the tile's, the rectangle's and the output's coordinates.
struct Overlap {
    src_row: usize,
    src_col: usize,
    dst_row: usize,
    dst_col: usize,
    h: usize,
    w: usize,
}

impl CziReader {
    /// Composes a whole plane from its tiles; uncovered pixels stay zero.
    pub(super) fn read_plane(&self, scene: usize, t: usize, c: usize, z: usize) -> Result<Plane> {
        let sc = self.scene_checked(scene, t, c, z)?;
        let d = sc.dims;

        // Size the output BEFORE reserving memory; a pixel is all its samples.
        let pixel_bytes = pixel_bytes_of(sc)?;
        let plane_bytes = d.checked_plane_bytes_samples(sc.dtype, sc.samples)?;

        let key = (sc.c_values[c], sc.z_values[z], sc.t_values[t]);
        let tiles = sc.tiles.get(&key).ok_or(Error::OutOfRange { scene, t, c, z })?;

        // Composed little-endian, converted to native order once.
        let mut composed = vec![0u8; plane_bytes];
        let row_stride = d
            .x
            .checked_mul(pixel_bytes)
            .ok_or_else(|| Error::Decode("CZI composed row stride overflow".to_string()))?;

        for &sb_idx in tiles {
            let sb = &self.subblocks[sb_idx];
            let tile = self.decode_tile(sb)?;
            blit_tile(
                &mut composed,
                d.x,
                d.y,
                row_stride,
                pixel_bytes,
                &tile,
                sb.stored_x,
                sb.stored_y,
                // i64 offsets: a crafted mosaic makes `Start - origin` exceed i32::MAX (`Start`: attacker-controlled).
                i64::from(sb.start_x) - i64::from(sc.origin_x),
                i64::from(sb.start_y) - i64::from(sc.origin_y),
            )?;
        }

        // `composed` is little-endian and already RGB (decode_tile swapped B and R).
        let data = PlaneBuf::from_bytes(sc.dtype, Endian::Little, &composed)?;
        Plane::new_samples(data, [d.y, d.x], sc.samples)
    }

    /// Decodes only the tiles that meet `rect`, composed into a rect-sized buffer.
    pub(super) fn read_region(
        &self,
        scene: usize,
        t: usize,
        c: usize,
        z: usize,
        rect: PixelRect,
    ) -> Result<Plane> {
        let sc = self.scene_checked(scene, t, c, z)?;
        let d = sc.dims;

        // The shared helper gives every reader the same refusal order and wording.
        checked_region("CZI region", &d, scene, t, c, z, rect)?;

        // An empty rect is an empty plane, as the trait's default returns.
        if rect.is_empty() {
            return empty_region_plane(sc.dtype, sc.samples, rect);
        }

        // Look up the tiles before allocating: a sparse (c,z,t) fails cheaply.
        let key = (sc.c_values[c], sc.z_values[z], sc.t_values[t]);
        let tiles = sc.tiles.get(&key).ok_or(Error::OutOfRange { scene, t, c, z })?;

        let composed = compose_region(&self.mmap, sc, &self.subblocks, tiles, rect, None)?;
        let data = PlaneBuf::from_bytes(sc.dtype, Endian::Little, &composed)?;
        Plane::new_samples(data, [rect.h, rect.w], sc.samples)
    }

    /// Shared by `plane()` and `region()`, so the two bounds checks cannot drift apart.
    fn scene_checked(&self, scene: usize, t: usize, c: usize, z: usize) -> Result<&CziScene> {
        let sc = self.scenes.get(scene).ok_or(Error::SceneOutOfRange {
            scene,
            count: self.scenes.len(),
        })?;
        let d = sc.dims;
        if t >= d.t || c >= d.c || z >= d.z {
            return Err(Error::OutOfRange { scene, t, c, z });
        }
        Ok(sc)
    }

    /// Little-endian sample bytes, already swapped from BGR to RGB for color tiles.
    pub(super) fn decode_tile(&self, sb: &SubBlock) -> Result<Vec<u8>> {
        decode_subblock(&self.mmap, sb)
    }
}

/// The decoded subblocks a composition reuses.
///
/// WHY it exists: a composition of a rect decodes the whole tiles that meet it, so a fill that
/// reads the level-0 pixels in blocks would decode the same tile once per block.
#[derive(Default)]
pub(super) struct DecodeCache {
    /// The decoded pixels of the subblocks, by their index.
    tiles: HashMap<usize, Rc<Vec<u8>>>,
    /// The indexes in the order they were used, the least recent first.
    order: VecDeque<usize>,
    /// The bytes the cached tiles hold.
    bytes: usize,
}

/// The most bytes of decoded tiles the cache holds.
const MAX_CACHED_BYTES: usize = 64 * 1024 * 1024;

impl DecodeCache {
    /// The decoded pixels of a subblock, when the cache holds them.
    fn get(&mut self, index: usize) -> Option<Rc<Vec<u8>>> {
        let pixels = self.tiles.get(&index)?.clone();
        self.order.retain(|&cached| cached != index);
        self.order.push_back(index);
        Some(pixels)
    }

    /// Keeps the decoded pixels of a subblock, dropping the oldest tiles when the cache is full.
    fn put(&mut self, index: usize, pixels: Rc<Vec<u8>>) {
        self.bytes += pixels.len();
        self.tiles.insert(index, pixels);
        self.order.push_back(index);
        while self.bytes > MAX_CACHED_BYTES && self.order.len() > 1 {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(dropped) = self.tiles.remove(&oldest) {
                self.bytes -= dropped.len();
            }
        }
    }
}

/// The scene's pixels over `rect`, composed from the tiles that meet it; the rest stays zero.
pub(super) fn compose_region(
    bytes: &[u8],
    scene: &CziScene,
    subblocks: &[SubBlock],
    tiles: &[usize],
    rect: PixelRect,
    mut cache: Option<&mut DecodeCache>,
) -> Result<Vec<u8>> {
    // The adds cannot overflow: the caller has already rejected that case.
    let rect_x1 = rect.x.checked_add(rect.w).ok_or_else(|| {
        Error::Decode(format!(
            "CZI region rect x+w overflows usize (x={}, w={})",
            rect.x, rect.w
        ))
    })?;
    let rect_y1 = rect.y.checked_add(rect.h).ok_or_else(|| {
        Error::Decode(format!(
            "CZI region rect y+h overflows usize (y={}, h={})",
            rect.y, rect.h
        ))
    })?;

    let pixel_bytes = pixel_bytes_of(scene)?;

    // Size the output to the rect, never the plane; a huge rect is refused on its own.
    let rect_dims = Dims5 {
        t: 1,
        c: 1,
        z: 1,
        y: rect.h,
        x: rect.w,
    };
    let region_bytes = rect_dims.checked_plane_bytes_samples(scene.dtype, scene.samples)?;
    let dst_row_stride = rect
        .w
        .checked_mul(pixel_bytes)
        .ok_or_else(|| Error::Decode("CZI region row stride overflow".to_string()))?;

    let mut composed = vec![0u8; region_bytes];
    for &sb_idx in tiles {
        let sb = &subblocks[sb_idx];
        let Some(o) = tile_overlap(sb, scene, rect, rect_x1, rect_y1)? else {
            continue;
        };
        // A compressed codec cannot decode a sub-rectangle: the whole tile is decoded, for the tiles intersecting.
        let tile = match cache.as_mut() {
            Some(cache) => decoded_tile(bytes, sb, sb_idx, cache)?,
            None => Rc::new(decode_subblock(bytes, sb)?),
        };
        blit_tile_clipped(
            &mut composed,
            dst_row_stride,
            pixel_bytes,
            &tile,
            sb.stored_x,
            sb.stored_y,
            o.src_row,
            o.src_col,
            o.dst_row,
            o.dst_col,
            o.w,
            o.h,
        )?;
    }
    Ok(composed)
}

/// The decoded pixels of a subblock: from the cache when it holds them.
fn decoded_tile(
    bytes: &[u8],
    sb: &SubBlock,
    index: usize,
    cache: &mut DecodeCache,
) -> Result<Rc<Vec<u8>>> {
    if let Some(pixels) = cache.get(index) {
        return Ok(pixels);
    }
    let pixels = Rc::new(decode_subblock(bytes, sb)?);
    cache.put(index, pixels.clone());
    Ok(pixels)
}

/// Decodes one subblock's pixels to little-endian samples, already swapped from BGR to RGB.
pub(super) fn decode_subblock(bytes: &[u8], sb: &SubBlock) -> Result<Vec<u8>> {
    // Guard the size before allocating: the plane cap, samples-aware.
    let tile_dims = Dims5 {
        t: 1,
        c: 1,
        z: 1,
        y: sb.stored_y,
        x: sb.stored_x,
    };
    let expected = tile_dims.checked_plane_bytes_samples(sb.dtype, sb.samples)?;

    // Pixel bytes were bounds-checked at open(); re-check defensively.
    let end = sb
        .pixel_off
        .checked_add(sb.data_size)
        .filter(|&e| e <= bytes.len())
        .ok_or_else(|| Error::Decode("CZI subblock pixel span out of bounds".to_string()))?;
    let raw = &bytes[sb.pixel_off..end];

    let le_bytes = codec_bytes(raw, sb, expected)?;

    // Swap B and R per pixel so callers always get RGB; a no-op for grayscale.
    // Whole samples are swapped, which is correct before PlaneBuf::from_bytes byteswaps to native order.
    if sb.samples == 3 {
        Ok(bgr_to_rgb(le_bytes, sb.dtype.size_bytes()))
    } else {
        Ok(le_bytes)
    }
}

/// The bytes of one pixel: its per-sample size times the samples it carries.
fn pixel_bytes_of(sc: &CziScene) -> Result<usize> {
    sc.dtype
        .size_bytes()
        .checked_mul(sc.samples as usize)
        .ok_or_else(|| Error::Decode("CZI per-pixel byte size overflow".to_string()))
}

/// The reason this build cannot decode a compression id, or `None` when it can.
///
/// WHY it is the one place: a level is offered only when every tile of it can be decoded, and the
/// codec below must not disagree with that check.
pub(super) fn undecodable_compression(compression: i32) -> Option<String> {
    match compression {
        0 | 5 | 6 => None,
        1 => Some("a JpgFile compression this build cannot decode".to_string()),
        2 => Some("an LZW compression this build cannot decode".to_string()),
        #[cfg(feature = "jpegxr")]
        4 => None,
        #[cfg(not(feature = "jpegxr"))]
        4 => Some("a JPEG-XR compression this build cannot decode without the jpegxr feature".to_string()),
        other => Some(format!("a compression this build cannot decode (id {other})")),
    }
}

/// The overlap of one tile with `rect`, or `None` when they do not meet (such a tile is never decoded).
fn tile_overlap(
    sb: &SubBlock,
    sc: &CziScene,
    rect: PixelRect,
    rect_x1: usize,
    rect_y1: usize,
) -> Result<Option<Overlap>> {
    let tile_x0_i = i64::from(sb.start_x) - i64::from(sc.origin_x);
    let tile_y0_i = i64::from(sb.start_y) - i64::from(sc.origin_y);
    if tile_x0_i < 0 || tile_y0_i < 0 {
        return Err(Error::Decode(
            "CZI tile placed before scene origin".to_string(),
        ));
    }
    let tile_x0 = usize::try_from(tile_x0_i)
        .map_err(|_| Error::Decode("CZI tile column offset out of range".to_string()))?;
    let tile_y0 = usize::try_from(tile_y0_i)
        .map_err(|_| Error::Decode("CZI tile row offset out of range".to_string()))?;
    let tile_x1 = tile_x0
        .checked_add(sb.stored_x)
        .ok_or_else(|| Error::Decode("CZI tile x-extent overflow".to_string()))?;
    let tile_y1 = tile_y0
        .checked_add(sb.stored_y)
        .ok_or_else(|| Error::Decode("CZI tile y-extent overflow".to_string()))?;

    let inter_x0 = tile_x0.max(rect.x);
    let inter_y0 = tile_y0.max(rect.y);
    let inter_x1 = tile_x1.min(rect_x1);
    let inter_y1 = tile_y1.min(rect_y1);
    // An empty intersection skips the tile without decoding: this is the out-of-core win.
    if inter_x0 >= inter_x1 || inter_y0 >= inter_y1 {
        return Ok(None);
    }
    Ok(Some(Overlap {
        src_row: inter_y0 - tile_y0,
        src_col: inter_x0 - tile_x0,
        dst_row: inter_y0 - rect.y,
        dst_col: inter_x0 - rect.x,
        h: inter_y1 - inter_y0,
        w: inter_x1 - inter_x0,
    }))
}

/// Decodes one subblock's raw bytes to little-endian samples, by its compression id.
fn codec_bytes(raw: &[u8], sb: &SubBlock, expected: usize) -> Result<Vec<u8>> {
    // The table names the compressions this build decodes; the arms below decode them.
    if let Some(reason) = undecodable_compression(sb.compression) {
        return Err(Error::Unsupported(format!(
            "CZI subblock cannot be decoded: {reason}"
        )));
    }
    // Color tiles stay BGR until the one swap in `decode_tile`, so every codec path gets the same treatment.
    Ok(match sb.compression {
        0 => {
            if raw.len() != expected {
                return Err(Error::Decode(format!(
                    "CZI uncompressed subblock has {} bytes, expected {expected}",
                    raw.len()
                )));
            }
            raw.to_vec()
        }
        // Some Zeiss files store raw bytes under Compression 5/6 when data_size is the plane size: treat those as raw.
        // This matches the reference reader; feeding non-zstd bytes to the decoder would fail.
        5 if raw.len() == expected => raw.to_vec(),
        5 => zstd_raw_decode(raw, expected)?,
        6 if raw.len() == expected => raw.to_vec(),
        6 => zstd1_decode(raw, expected, sb.dtype.size_bytes())?,
        // Declared gap: a JPEG-XR subblock holding several packed `M` frames decodes as one frame, because this path
        // has no frame-count check (the uncompressed and zstd paths refuse a mismatched length). No such sample exists.
        #[cfg(feature = "jpegxr")]
        4 => crate::jpegxr_ffi::decode_jpegxr(raw, sb.stored_x, sb.stored_y, sb.dtype, sb.samples)?,
        _ => {
            return Err(Error::Decode(format!(
                "CZI compression id {} reached no codec",
                sb.compression
            )))
        }
    })
}

#[cfg(test)]
mod tests;
