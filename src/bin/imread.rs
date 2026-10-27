// SPDX-License-Identifier: Apache-2.0
//! `imread` — CLI front-end for phasit-imread.
//!
//! Built solely on phasit-imread's public API (`phasit_imread::{open,
//! detect_format}` + the `ImageReader` trait). Four subcommands:
//!
//! ```text
//! imread info <file>
//! imread dump <file> <scene> <t> <c> <z> <out>
//! imread json <file>
//! imread sweep --json <listfile>
//! ```
//!
//! The two JSON modes report one file as one line of JSON — the format the
//! reader detected, the outcome of the open, every scene's metadata, the
//! scenes the reader refused, and the statistics and hash of the planes the
//! sweep decodes. `imread json` reports one file; `imread sweep` reports every
//! file of a list file.

use std::path::{Path, PathBuf};

use phasit_imread::Result as ImreadResult;

// The bin's own modules live in `src/bin/imread/`, one responsibility each.
// Rust resolves a module of `src/bin/imread.rs` next to that file, so the path
// of each module is named here.
#[path = "imread/batch.rs"]
mod batch;
#[path = "imread/json.rs"]
mod json;
#[path = "imread/plane.rs"]
mod plane;
#[path = "imread/report.rs"]
mod report;
#[path = "imread/sha256.rs"]
mod sha256;

/// A parsed, ready-to-run CLI command.
#[derive(Debug)]
enum Command {
    Info {
        file: PathBuf,
    },
    Dump {
        file: PathBuf,
        scene: usize,
        t: usize,
        c: usize,
        z: usize,
        out: PathBuf,
    },
    Json {
        file: PathBuf,
    },
    Sweep {
        list: PathBuf,
    },
}

/// Short usage/help text, printed on any argument error.
fn usage_text() -> String {
    format!(
        "imread (phasit-imread {})\n\nusage:\n  imread info <file>\n  \
         imread dump <file> <scene> <t> <c> <z> <out>\n  \
         imread json <file>\n  imread sweep --json <listfile>\n",
        env!("CARGO_PKG_VERSION")
    )
}

/// Parse one numeric CLI argument, reporting a clear error (plus usage) on failure.
fn parse_usize(name: &str, raw: &str) -> Result<usize, String> {
    raw.parse::<usize>()
        .map_err(|e| format!("invalid {name} value {raw:?}: {e}\n\n{}", usage_text()))
}

/// Parse the process argument vector (WITHOUT the program name) into a
/// [`Command`], or an error message (already including usage text) to print
/// to stderr with exit code 2.
fn parse_args(args: &[String]) -> Result<Command, String> {
    match args {
        [sub, file] if sub == "info" => Ok(Command::Info { file: PathBuf::from(file) }),
        [sub, file] if sub == "json" => Ok(Command::Json { file: PathBuf::from(file) }),
        [sub, flag, list] if sub == "sweep" && flag == "--json" => {
            Ok(Command::Sweep {
                list: PathBuf::from(list),
            })
        }
        [sub, file, scene, t, c, z, out] if sub == "dump" => {
            let scene = parse_usize("scene", scene)?;
            let t = parse_usize("t", t)?;
            let c = parse_usize("c", c)?;
            let z = parse_usize("z", z)?;
            Ok(Command::Dump {
                file: PathBuf::from(file),
                scene,
                t,
                c,
                z,
                out: PathBuf::from(out),
            })
        }
        _ => Err(usage_text()),
    }
}

/// Run a parsed command; any `Err` here is a `phasit_imread::Error` from
/// `open`/`plane`, printed as `error: {e}` with exit code 1.
fn run(cmd: Command) -> ImreadResult<()> {
    match cmd {
        Command::Info { file } => run_info(&file),
        Command::Dump { file, scene, t, c, z, out } => run_dump(&file, scene, t, c, z, &out),
        Command::Json { file } => {
            println!("{}", batch::guarded_report(&file));
            Ok(())
        }
        Command::Sweep { list } => {
            batch::run(&list)?;
            Ok(())
        }
    }
}

fn run_info(path: &Path) -> ImreadResult<()> {
    let format = phasit_imread::detect_format(path)?;
    let reader = phasit_imread::open(path)?;
    let meta = reader.meta();

    let file_label = path
        .file_name()
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| path.display().to_string());

    println!("file:   {file_label}");
    println!("format: {format:?}");
    println!("scenes: {}", meta.scene_count());

    for (i, scene) in meta.scenes.iter().enumerate() {
        if i > 0 {
            println!();
        }
        let file_index = reader.scene_file_index(i);
        if file_index == i {
            println!("  scene {i}  \"{}\"", scene.name);
        } else {
            println!("  scene {i}  \"{}\"  <Image> {file_index}", scene.name);
        }

        let d = scene.dims;
        println!("    dims:    T={} C={} Z={} Y={} X={}", d.t, d.c, d.z, d.y, d.x);
        // Show samples-per-pixel for color/multi-sample scenes (e.g. Bgr24 ->
        // "U8 (RGB, 3 spp)"); grayscale prints the plain dtype as before.
        match scene.samples {
            1 => println!("    dtype:   {:?}", scene.dtype),
            3 => println!("    dtype:   {:?} (RGB, 3 spp)", scene.dtype),
            n => println!("    dtype:   {:?} ({n} spp)", scene.dtype),
        }
        println!("    channels: {}", scene.channels.len());

        // Only print per-channel detail when the format actually recorded
        // names/colors for at least one channel.
        if scene.channels.iter().any(|ch| ch.name.is_some() || ch.color.is_some()) {
            for (ci, ch) in scene.channels.iter().enumerate() {
                let name = ch.name.as_deref().unwrap_or("(unnamed)");
                match ch.color {
                    Some([r, g, b]) => println!("      [{ci}] {name}  color=({r}, {g}, {b})"),
                    None => println!("      [{ci}] {name}"),
                }
            }
        }

        match scene.px_size_um {
            Some(px) => {
                println!("    px size: z={:.4} y={:.4} x={:.4} um", px.z, px.y, px.x)
            }
            None => println!("    px size: (none)"),
        }
    }

    for refused in reader.refused_scenes() {
        println!("refused: {refused}");
    }

    Ok(())
}

fn run_dump(path: &Path, scene: usize, t: usize, c: usize, z: usize, out: &Path) -> ImreadResult<()> {
    let reader = phasit_imread::open(path)?;
    let plane = reader.plane(scene, t, c, z)?;

    eprintln!(
        "shape: [{}, {}]  dtype: {:?}  spp: {}",
        plane.shape[0],
        plane.shape[1],
        plane.dtype(),
        plane.samples()
    );

    std::fs::write(out, plane::to_ne_bytes(&plane.data))?;
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_args(&args) {
        Ok(cmd) => {
            if let Err(e) = run(cmd) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Err(usage) => {
            eprint!("{usage}");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_args_is_usage_error() {
        assert!(parse_args(&args(&[])).is_err());
    }

    #[test]
    fn unknown_subcommand_is_usage_error() {
        assert!(parse_args(&args(&["frobnicate", "x.tif"])).is_err());
    }

    #[test]
    fn info_wrong_arg_count_is_usage_error() {
        assert!(parse_args(&args(&["info"])).is_err());
        assert!(parse_args(&args(&["info", "a.tif", "extra"])).is_err());
    }

    #[test]
    fn info_parses() {
        let cmd = parse_args(&args(&["info", "a.tif"])).unwrap();
        match cmd {
            Command::Info { file } => assert_eq!(file, PathBuf::from("a.tif")),
            _ => panic!("expected Info"),
        }
    }

    #[test]
    fn dump_wrong_arg_count_is_usage_error() {
        assert!(parse_args(&args(&["dump", "a.tif", "0", "0", "0", "0"])).is_err());
        assert!(
            parse_args(&args(&["dump", "a.tif", "0", "0", "0", "0", "out.bin", "extra"]))
                .is_err()
        );
    }

    #[test]
    fn dump_parses() {
        let cmd = parse_args(&args(&["dump", "a.tif", "1", "2", "3", "4", "out.bin"])).unwrap();
        match cmd {
            Command::Dump { file, scene, t, c, z, out } => {
                assert_eq!(file, PathBuf::from("a.tif"));
                assert_eq!((scene, t, c, z), (1, 2, 3, 4));
                assert_eq!(out, PathBuf::from("out.bin"));
            }
            _ => panic!("expected Dump"),
        }
    }

    #[test]
    fn dump_rejects_non_numeric_args() {
        let e =
            parse_args(&args(&["dump", "a.tif", "x", "0", "0", "0", "out.bin"])).unwrap_err();
        assert!(e.contains("scene"));
    }

    #[test]
    fn json_parses() {
        let cmd = parse_args(&args(&["json", "a.tif"])).unwrap();
        match cmd {
            Command::Json { file } => assert_eq!(file, PathBuf::from("a.tif")),
            _ => panic!("expected Json"),
        }
    }

    #[test]
    fn json_wrong_arg_count_is_usage_error() {
        assert!(parse_args(&args(&["json"])).is_err());
        assert!(parse_args(&args(&["json", "a.tif", "extra"])).is_err());
    }

    #[test]
    fn sweep_parses_with_its_json_flag() {
        let cmd = parse_args(&args(&["sweep", "--json", "list.txt"])).unwrap();
        match cmd {
            Command::Sweep { list } => assert_eq!(list, PathBuf::from("list.txt")),
            _ => panic!("expected Sweep"),
        }
    }

    #[test]
    fn sweep_without_the_json_flag_is_a_usage_error() {
        assert!(parse_args(&args(&["sweep", "list.txt"])).is_err());
        assert!(parse_args(&args(&["sweep", "--yaml", "list.txt"])).is_err());
    }
}
