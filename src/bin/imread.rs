// SPDX-License-Identifier: Apache-2.0
//! `imread` — CLI front-end for phasit-imread, built on its public API only.
//!
//! ```text
//! imread info <file>
//! imread dump <file> <scene> <t> <c> <z> <out>
//! imread json <file>
//! imread sweep --json <listfile>
//! ```

use std::path::{Path, PathBuf};

use phasit_imread::Result as ImreadResult;

// Modules live in `src/bin/imread/`; each `#[path]` names its file.
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
        /// Compare each sweep window with the same crop of the whole plane; a timing run turns it off.
        regions: bool,
    },
}

fn usage_text() -> String {
    format!(
        "imread (phasit-imread {})\n\nusage:\n  imread info <file>\n  \
         imread dump <file> <scene> <t> <c> <z> <out>\n  \
         imread json <file>\n  imread sweep --json [--no-regions] <listfile>\n",
        env!("CARGO_PKG_VERSION")
    )
}

fn parse_usize(name: &str, raw: &str) -> Result<usize, String> {
    raw.parse::<usize>()
        .map_err(|e| format!("invalid {name} value {raw:?}: {e}\n\n{}", usage_text()))
}

/// Parses the arguments (without the program name); an error already includes the usage text.
fn parse_args(args: &[String]) -> Result<Command, String> {
    match args {
        [sub, file] if sub == "info" => Ok(Command::Info { file: PathBuf::from(file) }),
        [sub, file] if sub == "json" => Ok(Command::Json { file: PathBuf::from(file) }),
        [sub, flag, list] if sub == "sweep" && flag == "--json" => {
            Ok(Command::Sweep {
                list: PathBuf::from(list),
                regions: true,
            })
        }
        [sub, flag, no_regions, list]
            if sub == "sweep" && flag == "--json" && no_regions == "--no-regions" =>
        {
            Ok(Command::Sweep {
                list: PathBuf::from(list),
                regions: false,
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

/// An `Err` is printed as `error: {e}` with exit code 1.
fn run(cmd: Command) -> ImreadResult<()> {
    match cmd {
        Command::Info { file } => run_info(&file),
        Command::Dump { file, scene, t, c, z, out } => run_dump(&file, scene, t, c, z, &out),
        Command::Json { file } => {
            println!("{}", batch::guarded_report(&file, true));
            Ok(())
        }
        Command::Sweep { list, regions } => {
            batch::run(&list, regions)?;
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
        match scene.samples {
            1 => println!("    dtype:   {:?}", scene.dtype),
            3 => println!("    dtype:   {:?} (RGB, 3 spp)", scene.dtype),
            n => println!("    dtype:   {:?} ({n} spp)", scene.dtype),
        }
        println!("    channels: {}", scene.channels.len());

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
                let z = match px.z {
                    Some(z) => format!("{z:.4}"),
                    None => "not recorded".to_string(),
                };
                println!("    px size: z={z} y={:.4} x={:.4} um", px.y, px.x)
            }
            None => println!("    px size: (none)"),
        }

        for note in &scene.notes {
            println!("    note:    {note}");
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
            Command::Sweep { list, regions } => {
                assert_eq!(list, PathBuf::from("list.txt"));
                assert!(regions, "the window comparison is on by default");
            }
            _ => panic!("expected Sweep"),
        }
    }

    #[test]
    fn sweep_can_skip_the_window_comparison() {
        // Timing runs compare two builds: they must do the same work per file, so the region comparison is off.
        let cmd = parse_args(&args(&["sweep", "--json", "--no-regions", "list.txt"])).unwrap();
        match cmd {
            Command::Sweep { list, regions } => {
                assert_eq!(list, PathBuf::from("list.txt"));
                assert!(!regions, "the window comparison is off when it is asked for");
            }
            _ => panic!("expected Sweep"),
        }
    }

    #[test]
    fn sweep_without_the_json_flag_is_a_usage_error() {
        assert!(parse_args(&args(&["sweep", "list.txt"])).is_err());
        assert!(parse_args(&args(&["sweep", "--yaml", "list.txt"])).is_err());
    }
}
