use crate::calibration::Calibration;
use crate::error::{Error, Result};
use crate::lsm_bytes::read_u32;
use crate::lsm_cz_info::{self, CzInfo, CzTag};
use crate::lsm_positions::LsmLayout;
use crate::types::{DType, Endian};

use super::ifd::{parse_ifd, read_tiff_header, FirstPage, IfdProps};
use super::strips::PageStrips;

pub(super) struct ParsedLsm {
    pub(super) endian: Endian,
    pub(super) dtype: DType,
    pub(super) t: usize,
    pub(super) channels: usize,
    pub(super) z: usize,
    pub(super) y: usize,
    pub(super) x: usize,
    pub(super) strips_per_channel: usize,
    pub(super) calibration: Calibration,
    pub(super) layout: LsmLayout,
    pub(super) channel_colors_offset: Option<usize>,
    pub(super) real_pages: Vec<PageStrips>,
}

/// Walks the IFD chain, keeps the real pages and their strips, and reads geometry from CZ-LSM-Info.
pub(super) fn parse_lsm(buf: &[u8]) -> Result<ParsedLsm> {
    let endian = read_tiff_header(buf)?;
    let walk = walk_real_pages(buf, endian)?;
    if walk.real_pages.is_empty() {
        return Err(Error::Decode(
            "LSM has no full-resolution pages".to_string(),
        ));
    }
    let first = walk.first.expect("at least one real page => Some");

    // Only uncompressed LSM is modelled; compressed variants need a codec and are refused.
    if first.compression != 1 {
        return Err(Error::Unsupported(format!(
            "LSM compression {} not supported (only uncompressed)",
            first.compression
        )));
    }
    let dtype = map_dtype(first.bits, first.sample_format)?;

    // Geometry from CZ-LSM-Info, cross-checked against the pixel data.
    let geo = read_cz_geometry(buf, walk.cz_tag, endian, &first)?;
    let channels = first.samples.max(1) as usize;
    let z = geo.dim_z.max(1) as usize;
    let t = geo.dim_time.max(1) as usize;
    let strips_per_channel = strips_per_channel(&first, t, channels)?;

    let layout = LsmLayout::new(geo.dim_p, geo.dim_m, z)?;
    check_page_count(&layout, walk.real_pages.len())?;

    Ok(ParsedLsm {
        endian,
        dtype,
        t,
        channels,
        z,
        y: first.height as usize,
        x: first.width as usize,
        strips_per_channel,
        calibration: geo.calibration,
        layout,
        channel_colors_offset: geo.channel_colors_offset,
        real_pages: walk.real_pages,
    })
}

struct Walk {
    real_pages: Vec<PageStrips>,
    first: Option<FirstPage>,
    cz_tag: Option<CzTag>,
}

fn walk_real_pages(buf: &[u8], endian: Endian) -> Result<Walk> {
    let mut walk = Walk {
        real_pages: Vec::new(),
        first: None,
        cz_tag: None,
    };
    let mut next = read_u32(buf, 4, endian)? as u64;
    let mut visited = 0usize;
    while next != 0 {
        // Bound the walk so a corrupt cyclic chain cannot loop forever.
        visited += 1;
        if visited > MAX_IFDS {
            return Err(Error::Parse(format!(
                "LSM IFD chain exceeds {MAX_IFDS} entries (corrupt/cyclic?)"
            )));
        }

        let ifd = usize::try_from(next)
            .map_err(|_| Error::Parse("LSM IFD offset overflows usize".to_string()))?;
        let (page, props, cz, next_ifd) = parse_ifd(buf, ifd, endian)?;

        if walk.cz_tag.is_none() {
            walk.cz_tag = cz;
        }
        if !props.reduced {
            match &walk.first {
                None => {
                    walk.first = Some(FirstPage {
                        width: props.width,
                        height: props.height,
                        samples: props.samples,
                        bits: props.bits,
                        sample_format: props.sample_format,
                        planar: props.planar,
                        compression: props.compression,
                        strips: page.offsets.len(),
                    });
                }
                Some(first) => check_later_page(&props, first, &page, walk.real_pages.len())?,
            }
            walk.real_pages.push(page);
        }
        next = next_ifd;
    }
    Ok(walk)
}

/// A later real page must match page 0's geometry and strip count, or it would select the wrong strips.
fn check_later_page(
    props: &IfdProps,
    first: &FirstPage,
    page: &PageStrips,
    index: usize,
) -> Result<()> {
    // Each later page must match page 0's geometry: equal byte totals would hide a different W x H.
    if props.width != first.width
        || props.height != first.height
        || props.samples != first.samples
        || props.bits != first.bits
        || props.sample_format != first.sample_format
        || props.compression != first.compression
        || props.planar != first.planar
    {
        return Err(Error::Unsupported(format!(
            "LSM real page {index} geometry differs from page 0 \
             ({}x{} s{} b{} f{} c{} planar={} vs {}x{} s{} b{} f{} c{} planar={})",
            props.width,
            props.height,
            props.samples,
            props.bits,
            props.sample_format,
            props.compression,
            props.planar,
            first.width,
            first.height,
            first.samples,
            first.bits,
            first.sample_format,
            first.compression,
            first.planar,
        )));
    }
    // Strip count is compared too: a later page with other strips would select the wrong ones.
    if page.offsets.len() != first.strips || page.bytes.len() != first.strips {
        return Err(Error::Decode(format!(
            "LSM real page {index} has {} strip offsets / {} strip byte-counts, \
             page 0 has {} strips",
            page.offsets.len(),
            page.bytes.len(),
            first.strips,
        )));
    }
    Ok(())
}

fn read_cz_geometry(
    buf: &[u8],
    cz_tag: Option<CzTag>,
    endian: Endian,
    first: &FirstPage,
) -> Result<CzInfo> {
    let cz_tag = cz_tag.ok_or_else(|| {
        Error::Decode("LSM is missing the CZ-LSM-Info tag (34412)".to_string())
    })?;
    let geo = lsm_cz_info::parse(buf, cz_tag, endian)?;
    let (x, y) = (first.width as usize, first.height as usize);
    let samples = first.samples.max(1) as usize;
    if geo.dim_x as usize != x || geo.dim_y as usize != y {
        return Err(Error::Decode(format!(
            "LSM CZ-LSM-Info XY ({},{}) disagrees with IFD ({x},{y})",
            geo.dim_x, geo.dim_y
        )));
    }
    if geo.dim_channels as usize != samples {
        return Err(Error::Decode(format!(
            "LSM CZ-LSM-Info channels {} disagrees with SamplesPerPixel {samples}",
            geo.dim_channels
        )));
    }
    Ok(geo)
}

// Plain `%`: `is_multiple_of` needs Rust 1.87, and the crate builds on older stable toolchains.
#[allow(clippy::manual_is_multiple_of)]
fn strips_per_channel(first: &FirstPage, t: usize, channels: usize) -> Result<usize> {
    // Page order inside a scene is validated only at T=1: a multi-T file with another T/Z convention would
    // misassign planes, so it fails closed. TODO: re-enable once a multi-T .lsm is in the parity suite.
    if t > 1 {
        return Err(Error::Unsupported(
            "multi-timepoint LSM (T>1) not yet supported: page ordering unverified".to_string(),
        ));
    }

    // Channels are planar strip blocks; interleaved LSM is not seen in practice and is refused.
    if channels > 1 && !first.planar {
        return Err(Error::Unsupported(
            "interleaved (chunky) multi-channel LSM not supported".to_string(),
        ));
    }

    if first.strips == 0 || first.strips % channels != 0 {
        return Err(Error::Decode(format!(
            "LSM page has {} strips, not a multiple of {channels} channels",
            first.strips
        )));
    }
    Ok(first.strips / channels)
}

/// Each page is one Z at one T of one tile or position, with channels as planar strips.
fn check_page_count(layout: &LsmLayout, pages: usize) -> Result<()> {
    let expected_pages = layout
        .expected_pages()
        .ok_or_else(|| Error::Decode("LSM T*Z*P*M overflow".to_string()))?;
    if pages != expected_pages {
        let implied = if layout.scene_count() == 1 { "T*Z" } else { "T*Z*P*M" };
        return Err(Error::Decode(format!(
            "LSM has {pages} real pages but CZ-LSM-Info implies {implied} = {expected_pages}"
        )));
    }
    Ok(())
}

/// Upper bound on IFDs we will follow (guards against a cyclic/corrupt chain).
const MAX_IFDS: usize = 1_000_000;

/// Mirrors the TIFF reader's table: BitsPerSample and SampleFormat (1 uint, 2 int, 3 float).
pub(super) fn map_dtype(bits: u16, sample_format: u16) -> Result<DType> {
    match (sample_format, bits) {
        (1, 8) => Ok(DType::U8),
        (1, 16) => Ok(DType::U16),
        (2, 16) => Ok(DType::I16),
        (3, 32) => Ok(DType::F32),
        _ => Err(Error::Unsupported(format!(
            "LSM sample type (SampleFormat={sample_format}, {bits}-bit) not modelled"
        ))),
    }
}
