#!/usr/bin/env python
"""Extract real Zeiss JPEG-XR tiles from a CZI into `tests/fixtures/jpegxr_real/`.

Walks the subblock directory with `czifile` and samples every k-th subblock, so
the sample spreads over the whole file, and writes for each one:

  <name>.jxr  the raw JPEG-XR codestream, the bytes the CZI subblock holds
  <name>.raw  the expected decode, in the container's NATIVE channel order

The layout of every fixture is DERIVED from the subblock itself — its pixel
type and its compression — and never hard-coded:

* the samples per pixel and the sample dtype come from the subblock's pixel
  type (`czifile`'s `CziPixelType`);
* only JPEG-XR subblocks (compression 4) are decoded at all;
* a colour subblock is reversed to the container's native B,G,R order, a
  greyscale one is not reversed at all.

`imagecodecs` decodes the codestream to R,G,B. The container declares
`PixelType = Bgr24`, so its native order is B,G,R and the ground truth for
`decode_jpegxr` (which asks jxrlib for 24bppBGR and returns the native order
unchanged) is the decode with its channels reversed -- the same convention
`tests/fixtures/jpegxr/gen_jpegxr_fixtures.py` documents.

`manifest_real.json` lists the fixtures in the format the Rust verifier in
`src/jpegxr_ffi.rs` reads: `name`, `dtype`, `samples`, `stored_x`, `stored_y`.
`stored_x` and `stored_y` are the size the CONTAINER declares for the
subblock (`entry.stored_shape`), not the size the decoder happened to return,
so the verifier's geometry check is not self-referential. The script refuses to
write a fixture whose codestream does not decode to the declared size.

MEMORY: only the sampled subblocks are decoded, one at a time; the mosaic is
never materialized.

Run: python extract_jxr_tiles.py PATH_TO_CZI [--out DIR] [--count 24]
"""
from __future__ import annotations

import argparse
import hashlib
import json
import platform
import sys
import time
from pathlib import Path

import czifile
import imagecodecs
import numpy as np

DEFAULT_COUNT = 24

# The CZI compression id of JPEG-XR (czifile's `CziCompressionType.JPEGXR`).
JPEG_XR = 4

# The dtype names the Rust verifier parses (`src/jpegxr_ffi.rs`).
DTYPE_NAMES = {"uint8": "u8", "uint16": "u16", "float32": "f32"}


def select_indices(total: int, count: int) -> list[int]:
    """Every k-th subblock index, spread deterministically over the file."""
    if count >= total:
        return list(range(total))
    step = total // count
    return [i * step for i in range(count)]


def pixel_layout(entry) -> tuple[int, str, bool]:
    """The samples, the dtype name and whether the channels are reversed.

    All three come from the subblock's own pixel type, so a subblock of
    another layout is never written as if it were this one.
    """
    samples = entry.pixel_type.samples
    dtype_name = DTYPE_NAMES.get(str(entry.pixel_type.dtype))
    if dtype_name is None:
        raise ValueError(f"unsupported sample dtype {entry.pixel_type.dtype}")
    return samples, dtype_name, samples == 3


def decode_tile(czi: czifile.CziFile, entry) -> tuple[bytes, bytes, int, int]:
    """The raw codestream of one subblock and its decode in native order.

    Returns the codestream, the expected decode, the declared size and the
    size the codestream actually decodes to.
    """
    if int(entry.compression) != JPEG_XR:
        raise ValueError(f"subblock {entry.file_position} is not JPEG-XR")
    subblock = entry.read_segment_data(czi)
    raw = bytes(subblock.data(raw=True))
    decoded = imagecodecs.jpegxr_decode(raw)
    # `czifile` decodes JPEG-XR with WIC on Windows and with imagecodecs
    # elsewhere; both must agree with imagecodecs' own jxrlib binding, else the
    # ground truth would depend on the machine that produced it. `czifile`
    # keeps the singleton C axis, imagecodecs drops it.
    pixels = np.asarray(subblock.data())
    if pixels.size != decoded.size or not np.array_equal(
        pixels.reshape(decoded.shape), decoded
    ):
        raise SystemExit(
            f"czifile and imagecodecs disagree on the subblock at {entry.file_position}"
        )
    samples, _dtype, reverse = pixel_layout(entry)
    native = decoded[..., ::-1] if reverse else decoded
    stored_h = entry.stored_shape[entry.dims.index("Y")]
    stored_w = entry.stored_shape[entry.dims.index("X")]
    return raw, np.ascontiguousarray(native).tobytes(), stored_w, stored_h


def library_versions() -> dict:
    """The versions of the libraries the fixtures depend on."""
    return {
        "czifile": getattr(czifile, "__version__", "unknown"),
        "imagecodecs": imagecodecs.__version__,
        "numpy": np.__version__,
        "python": platform.python_version(),
    }


def sample_identity(path: str) -> dict:
    """The sample's name, size and md5, so the fixtures are tied to it."""
    digest = hashlib.md5()
    size = 0
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
            size += len(chunk)
    return {"file": Path(path).name, "size": size, "md5": digest.hexdigest()}


def parse_args(argv):
    parser = argparse.ArgumentParser(
        description="Extract real JPEG-XR tiles from a Zeiss CZI (no default paths)."
    )
    parser.add_argument("path", help="the .czi file to read")
    parser.add_argument(
        "--out",
        default=None,
        help="the folder the tiles are written to (default: "
        "tests/fixtures/jpegxr_real next to this script)",
    )
    parser.add_argument(
        "--count",
        type=int,
        default=DEFAULT_COUNT,
        help=f"how many tiles to sample (default: {DEFAULT_COUNT})",
    )
    args = parser.parse_args(argv)
    if args.count < 1:
        parser.error("--count must be at least 1")
    return args


def extract(czi, entries, out_dir: Path, count: int) -> list[dict]:
    """Sample the subblocks, write the fixtures and return their records."""
    indices = select_indices(len(entries), count)
    print(f"{len(entries)} subblocks in the file; sampling {len(indices)}")
    records = []
    for n, index in enumerate(indices):
        entry = entries[index]
        raw, native, stored_w, stored_h = decode_tile(czi, entry)
        samples, dtype_name, _reverse = pixel_layout(entry)
        expected = stored_w * stored_h * samples * np.dtype(
            entry.pixel_type.dtype
        ).itemsize
        if len(native) != expected:
            raise SystemExit(
                f"tile {n}: the codestream decodes to {len(native)} bytes, but the "
                f"container declares {stored_w}x{stored_h} ({expected} bytes)"
            )
        name = f"tile_{n:02d}"
        (out_dir / f"{name}.jxr").write_bytes(raw)
        (out_dir / f"{name}.raw").write_bytes(native)
        records.append(
            {
                "name": name,
                "dtype": dtype_name,
                "samples": samples,
                "stored_x": stored_w,
                "stored_y": stored_h,
            }
        )
        print(
            f"  {name}: {stored_w}x{stored_h} {dtype_name}x{samples} "
            f"jxr={len(raw):8d} raw={len(native):9d} "
            f"sha256={hashlib.sha256(raw).hexdigest()[:16]}"
        )
    return records


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    here = Path(__file__).resolve().parent
    out_dir = Path(args.out) if args.out else here.parent / "fixtures" / "jpegxr_real"
    out_dir.mkdir(parents=True, exist_ok=True)

    started = time.perf_counter()
    with czifile.CziFile(args.path) as czi:
        fixtures = extract(czi, czi.subblock_directory, out_dir, args.count)

    doc = {
        "generator": "extract_jxr_tiles.py",
        "sample": sample_identity(args.path),
        "libraries": library_versions(),
        "fixtures": fixtures,
    }
    (out_dir / "manifest_real.json").write_text(
        json.dumps(doc, indent=2), encoding="utf-8"
    )
    elapsed = time.perf_counter() - started
    print(
        f"wrote {len(fixtures)} tiles + manifest_real.json to {out_dir} "
        f"in {elapsed:.1f} s"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
