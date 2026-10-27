"""Dump Bio-Formats' own reference hashes for the planes of an LSM file.

Bio-Formats is the reference here: the script never reads phasit-imread's
output, so the hashes it records cannot certify the Rust reader against
itself. It drives the Bio-Formats harness (a small Java probe that writes one
plane as raw little-endian samples) and records the SHA-256 of every series'
first plane.

The hashes of a PRIVATE acquisition file are a fingerprint of that file, so
the dump is never committed: it lands in the gitignored folder
`tests/parity/_ref/<stem>/manifest.json`, and the test reads it through the
shared real-data gate (`PHASIT_REAL_DATA_REQUIRED`).

The harness is named on the command line, so this script carries no local
path of its own:

    python dump_ref_lsm.py PATH_TO_LSM --java JAVA --jar BIOFORMATS_JAR \
        --classes HARNESS_CLASSES [--out DIR]

`--out` defaults to this script's `_ref/` folder; the manifest goes into a
folder named after the file, like the other dumpers do.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

# The harness answers on stdout, one JSON object per line, prefixed with this.
BFJSON = "BFJSON "


class Harness:
    """The Bio-Formats harness: one JVM, one plane on disk at a time."""

    def __init__(self, java: str, jar: str, classes: str, sample: str) -> None:
        classpath = jar + os.pathsep + classes
        self.proc = subprocess.Popen(
            [java, "-cp", classpath, "BfDump", "serve", sample],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            encoding="utf-8",
            errors="replace",
            bufsize=1,
        )
        self.info = self._read()

    def _read(self) -> dict:
        while True:
            line = self.proc.stdout.readline()
            if not line:
                raise RuntimeError("the Bio-Formats harness produced no answer")
            if line.startswith(BFJSON):
                return json.loads(line[len(BFJSON):])

    def dump(self, series: int, c: int, z: int, t: int, out_path: str) -> dict:
        self.proc.stdin.write("dump %d %d %d %d %s\n" % (series, c, z, t, out_path))
        self.proc.stdin.flush()
        answer = self._read()
        if "error" in answer:
            raise RuntimeError("the harness refused series %d: %s" % (series, answer["error"]))
        return answer

    def close(self) -> None:
        try:
            self.proc.stdin.write("quit\n")
            self.proc.stdin.flush()
            self.proc.wait(timeout=30)
        except Exception:
            self.proc.kill()


def sha256_file(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def dump_planes(harness: Harness, series_count: int) -> list[dict]:
    """The first plane of every series, hashed as the harness wrote it."""
    planes: list[dict] = []
    with tempfile.TemporaryDirectory(prefix="dump_ref_lsm_") as work:
        raw = os.path.join(work, "plane.bin")
        # The harness splits its commands on whitespace, so the path of the
        # plane it writes cannot contain one.
        if any(character.isspace() for character in raw):
            raise RuntimeError("the harness cannot take a path with spaces in it: %s" % raw)
        for series in range(series_count):
            answer = harness.dump(series, 0, 0, 0, raw)
            planes.append(
                {
                    "series": series,
                    "c": 0,
                    "z": 0,
                    "t": 0,
                    "shape": [answer["sizeY"], answer["sizeX"]],
                    "pixelType": answer["pixelType"],
                    "bytes": answer["bytesWritten"],
                    "sha256": sha256_file(raw),
                }
            )
    return planes


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Dump Bio-Formats' reference hashes for an LSM file (no default paths)."
    )
    parser.add_argument("path", help="the LSM file to dump the reference planes of")
    parser.add_argument("--java", required=True, help="the java executable that runs the harness")
    parser.add_argument("--jar", required=True, help="the Bio-Formats jar the harness runs against")
    parser.add_argument("--classes", required=True, help="the folder holding the harness classes")
    parser.add_argument(
        "--out",
        default=None,
        help="the folder the manifest goes into (default: this script's _ref/)",
    )
    args = parser.parse_args(argv)

    sample = Path(args.path)
    if not sample.is_file():
        print("no such file: %s" % sample, file=sys.stderr)
        return 2

    harness = Harness(args.java, args.jar, args.classes, str(sample))
    try:
        series_count = harness.info["seriesCount"]
        planes = dump_planes(harness, series_count)
    finally:
        harness.close()

    here = Path(__file__).resolve().parent
    out_dir = Path(args.out) if args.out else here / "_ref"
    sample_dir = out_dir / sample.stem
    sample_dir.mkdir(parents=True, exist_ok=True)

    manifest = {
        "generator": "dump_ref_lsm.py",
        "reference": "bio-formats",
        "sample": {"file": sample.name},
        "planes": planes,
    }
    manifest_path = sample_dir / "manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2), encoding="utf-8")
    print("Wrote %s (%d plane(s))" % (manifest_path, len(planes)))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
