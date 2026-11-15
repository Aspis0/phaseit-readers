// SPDX-License-Identifier: Apache-2.0
//! The `.nd` dataset at the ceilings: the scenes a crafted file makes, and the text they hold.

use std::collections::HashMap;
use std::sync::Arc;

use super::meta;
use super::parse;
use super::sources::Geometry;

use crate::calibration::Calibration;
use crate::guard::MAX_ND_AXIS_LEN;
use crate::types::{DType, SceneMeta};

/// The wavelengths of the crafted dataset: as many as the plane cap allows beside the positions.
const WAVELENGTHS: usize = 15;

/// The bytes one wavelength name holds in the bound test: long enough to show what a name costs.
const NAME_BYTES: usize = 32 * 1024;

/// The text the scenes of a dataset at the ceilings may hold: the names are shared, so they are
/// paid once (1 660 062 bytes for the dataset below), and the names of the positions are short.
const OWNED_BYTES: usize = 2 * 1024 * 1024;

/// The geometry of the crafted dataset: one grey pixel, so the scenes cost what the dataset costs.
fn geometry() -> Geometry {
    Geometry {
        dtype: DType::U16,
        x: 1,
        y: 1,
        samples: 1,
    }
}

/// A crafted `.nd` at the ceilings: as many stage positions as the reader allows, and wavelengths
/// whose names are as long as the test asks for.
fn crafted(name_bytes: usize) -> String {
    let mut text = format!(
        "\"NDInfoFile\", Version 1.0\n\"DoWave\", TRUE\n\"NWavelengths\", {WAVELENGTHS}\n\
         \"DoStage\", TRUE\n\"NStagePositions\", {MAX_ND_AXIS_LEN}\n"
    );
    for index in 1..=WAVELENGTHS {
        text.push_str(&format!(
            "\"WaveName{index}\", \"{}\"\n",
            "w".repeat(name_bytes)
        ));
    }
    text.push_str("\"EndFile\"\n");
    text
}

/// The scenes of a dataset name their channels with one text: a name is one allocation for the
/// whole dataset, not one per scene.
#[test]
fn the_scenes_of_a_dataset_share_one_channel_name() {
    let dataset = parse::parse(&crafted(16), "base").expect("a dataset the text states");
    let image = meta(&dataset, geometry(), Calibration::default());
    let first = image.scenes[0].channels[0]
        .name
        .as_ref()
        .expect("the wavelength has a name");
    assert!(
        image.scenes.iter().all(|scene| scene.channels[0]
            .name
            .as_ref()
            .is_some_and(|name| Arc::ptr_eq(name, first))),
        "every scene names its channel with the same text"
    );
}

/// The scenes of a dataset at the ceilings hold their names once: the text of a wavelength name is
/// paid for the dataset, not for every scene.
#[test]
fn the_scenes_at_the_ceilings_hold_their_names_once() {
    let dataset = parse::parse(&crafted(NAME_BYTES), "base").expect("a dataset at the ceilings");
    assert_eq!(dataset.stages.len(), MAX_ND_AXIS_LEN);
    let image = meta(&dataset, geometry(), Calibration::default());
    let scenes = &image.scenes;
    assert_eq!(scenes.len(), MAX_ND_AXIS_LEN);

    let owned = owned_bytes(scenes);
    assert!(
        owned < OWNED_BYTES,
        "the scenes hold {owned} bytes of text, where a name copied into every scene would hold {}",
        WAVELENGTHS * NAME_BYTES * MAX_ND_AXIS_LEN
    );
}

/// The string bytes the scenes own: every distinct text counted once, because the names the scenes
/// share are one allocation.
fn owned_bytes(scenes: &[SceneMeta]) -> usize {
    let mut names: HashMap<usize, usize> = HashMap::new();
    let mut bytes = 0;
    for scene in scenes {
        bytes += scene.name.len();
        for channel in &scene.channels {
            let Some(name) = channel.name.as_ref() else {
                continue;
            };
            let address = Arc::as_ptr(name) as *const u8 as usize;
            if names.insert(address, name.len()).is_none() {
                bytes += name.len();
            }
        }
    }
    bytes
}
