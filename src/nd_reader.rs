// SPDX-License-Identifier: Apache-2.0
//! MetaMorph Multi-Dimensional Acquisition (`.nd`): a small text file beside the acquisition, listing the axes, and
//! one image file per (wavelength, stage position, time point).
//! The scenes are the stage positions, the channels the wavelengths, the time points the time axis, and the z steps
//! the z-series. A wavelength without a z-series holds ONE plane in a z-dataset: its second plane is refused, never
//! the first plane repeated.
//! The image files are opened when a plane of them is first read and kept for the next one; a file the dataset
//! directory does not hold is refused by name when its plane is read, never a zero plane.
//! Calibration comes from the dataset's first file; the z step comes from the `.nd` file's own `ZStepSize`.

mod listing;
mod names;
mod parse;
mod sources;

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::calibration::Calibration;
use crate::error::{Error, Result};
use crate::guard::MAX_ND_FILE_BYTES;
use crate::types::{ChannelMeta, Dims5, ImageMeta, ImageReader, Plane, SceneMeta};

use self::listing::Listing;
use self::parse::Dataset;
use self::sources::{Geometry, Sources};

/// A MetaMorph `.nd` dataset: the text file and the image files it lists.
pub struct NdReader {
    meta: ImageMeta,
    dataset: Dataset,
    listing: Listing,
    sources: Sources,
}

impl ImageReader for NdReader {
    fn open(path: &Path) -> Result<Self> {
        let text = decode(&read_capped(path)?);
        let dataset = parse::parse(&text, &stem(path))?;
        names::guard_names(&dataset)?;

        let directory = path.parent().unwrap_or(Path::new("")).to_path_buf();
        let listing = Listing::read(&directory)?;
        let reference = reference(&dataset, &listing)?;
        let reader = sources::open_file(&reference)?;
        let geometry = geometry(reader.as_ref())?;
        let calibration = calibration(&dataset, reader.as_ref());

        let sources = Sources::new(geometry);
        sources.insert(&reference, Arc::clone(&reader));

        Ok(NdReader {
            meta: meta(&dataset, geometry, calibration),
            dataset,
            listing,
            sources,
        })
    }

    fn meta(&self) -> &ImageMeta {
        &self.meta
    }

    fn plane(&self, scene: usize, t: usize, c: usize, z: usize) -> Result<Plane> {
        self.meta.scene(scene)?;
        if t >= self.dataset.times || c >= self.dataset.waves.len() || z >= self.dataset.z_steps {
            return Err(Error::OutOfRange { scene, t, c, z });
        }
        let wave = &self.dataset.waves[c];
        if !wave.has_z && z > 0 {
            return Err(Error::Unsupported(format!(
                "MetaMorph .nd: wavelength {} holds no z-series, so it has one plane; z = {z} is refused",
                c + 1
            )));
        }

        let name = names::file_name(&self.dataset, c + 1, scene + 1, t + 1);
        let Some(path) = self.listing.find(&name) else {
            return Err(Error::Unsupported(format!(
                "MetaMorph .nd: the image file of wavelength {}, stage position {}, time point {} ({name}) is \
                 missing from the dataset directory",
                c + 1,
                scene + 1,
                t + 1
            )));
        };
        let reader = self.sources.reader(path)?;
        if wave.has_z {
            reader.plane(0, 0, 0, z)
        } else {
            reader.plane(0, 0, 0, 0)
        }
    }
}

/// The dataset's first image file: the geometry and the calibration of the dataset are its own.
fn reference(dataset: &Dataset, listing: &Listing) -> Result<PathBuf> {
    for wave in 1..=dataset.waves.len() {
        for stage in 1..=dataset.stages.len() {
            for time in 1..=dataset.times {
                let name = names::file_name(dataset, wave, stage, time);
                if let Some(path) = listing.find(&name) {
                    return Ok(path.to_path_buf());
                }
            }
        }
    }
    Err(Error::Unsupported(
        "MetaMorph .nd: the dataset directory holds none of the image files the dataset declares"
            .to_string(),
    ))
}

/// The geometry of the dataset, as the first file states it.
fn geometry(reader: &dyn ImageReader) -> Result<Geometry> {
    let scene = reader.meta().scene(0)?;
    Ok(Geometry {
        dtype: scene.dtype,
        x: scene.dims.x,
        y: scene.dims.y,
    })
}

/// The calibration of the dataset: the first file's own, with the z step the `.nd` file states.
fn calibration(dataset: &Dataset, reader: &dyn ImageReader) -> Calibration {
    let Some(px) = reader
        .meta()
        .scene(0)
        .ok()
        .and_then(|scene| scene.px_size_um)
    else {
        return Calibration::uncalibrated(vec![NO_SCALE.to_string()]);
    };
    Calibration::sized(px.x, px.y, dataset.z_step_um)
}

/// The dataset's scenes: one per stage position, with the wavelengths as its channels.
fn meta(dataset: &Dataset, geometry: Geometry, calibration: Calibration) -> ImageMeta {
    let channels: Vec<ChannelMeta> = dataset
        .waves
        .iter()
        .map(|wave| ChannelMeta {
            name: (!wave.name.is_empty()).then(|| wave.name.clone()),
            color: None,
        })
        .collect();
    let dims = Dims5 {
        t: dataset.times,
        c: dataset.waves.len(),
        z: dataset.z_steps,
        y: geometry.y,
        x: geometry.x,
    };
    let scenes = dataset
        .stages
        .iter()
        .enumerate()
        .map(|(index, stage)| SceneMeta {
            dims,
            dtype: geometry.dtype,
            samples: 1,
            px_size_um: calibration.px_size_um,
            notes: calibration.notes.clone(),
            channels: channels.clone(),
            palette: None,
            name: scene_name(dataset, index, stage),
        })
        .collect();
    ImageMeta {
        name: dataset.base.clone(),
        scenes,
    }
}

/// The name of one scene: the stage position's own name, or its place in the dataset.
fn scene_name(dataset: &Dataset, index: usize, stage: &str) -> String {
    if stage.is_empty() {
        format!("{} (stage {})", dataset.base, index + 1)
    } else {
        stage.to_string()
    }
}

/// The `.nd` file's bytes, refused above the cap before they are read.
fn read_capped(path: &Path) -> Result<Vec<u8>> {
    let file = File::open(path)?;
    let length = file.metadata()?.len();
    if length > MAX_ND_FILE_BYTES {
        return Err(Error::OversizedAllocation {
            requested: usize::try_from(length).unwrap_or(usize::MAX),
            cap: MAX_ND_FILE_BYTES as usize,
        });
    }
    let mut bytes = Vec::new();
    file.take(MAX_ND_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(Error::Io)?;
    if bytes.len() as u64 > MAX_ND_FILE_BYTES {
        return Err(Error::OversizedAllocation {
            requested: bytes.len(),
            cap: MAX_ND_FILE_BYTES as usize,
        });
    }
    Ok(bytes)
}

/// The `.nd` text: UTF-8 when the bytes are UTF-8, Latin-1 otherwise, so a stray byte is never a panic.
fn decode(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes.iter().map(|byte| char::from(*byte)).collect(),
    }
}

/// The `.nd` file's name without its extension: the image files are named after it.
fn stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("image")
        .to_string()
}

const NO_SCALE: &str = "the dataset's first image file states no pixel size this reader can use";
