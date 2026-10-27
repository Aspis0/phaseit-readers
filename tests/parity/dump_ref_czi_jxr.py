"""Dump reference regions of a real Zeiss JPEG-XR CZI as `.npy` (czifile truth).

Reads the subblock directory with `czifile`, computes the level-0 scene
bounding box the way `phasit-imread` does (the bounding box of the
full-resolution tiles, each placed at its `X.Start`/`Y.Start`), and picks the
windows that exercise the mosaic:

  * four single-tile interiors (one corner, the centre, and the tile bottom
    right corners) — covered by exactly ONE tile, so the comparison cannot
    depend on the order in which overlapping tiles are blitted;
  * two windows straddling the seam of two side-by-side tiles, inside the
    band the two tiles share;
  * one window straddling a corner where four tiles meet;
  * one window at the bottom-right corner of a tile's interior, far from the
    top-left corner every other single-tile window sits in;
  * one window hugging the scene bounding box, partly outside every tile, so
    the fill value of the gap shows.

Only the tiles that intersect a window are decoded; the WHOLE mosaic is never
composed.

COMPOSITION ORDER: `czifile` composites the tiles of a mosaic in ASCENDING
MOSAIC INDEX, so the tile with the higher M ends up on top (`czifile.py:1308`
and `:1309`, applied at `:2810` and `:2811`). `phasit-imread` composites them
in the same order (`src/czi_reader.rs`, `CziReader::open` — the per-plane
mosaic sort), so the reference and the reader agree. The manifest records, per
region, the order the region was ACTUALLY composited in: when the sort does not
run — a plane whose tiles all carry a negative mosaic index is not sorted — the
region was drawn in the subblock-directory order, and the manifest says that
instead of the mosaic order. The manifest's TOP level names that same effective
order, with the order the run ASKED for beside it, so the two can never be
confused. The manifest still records
the SUBBLOCK-DIRECTORY order next to it,
because the two orders are not the same in every file: in the Axioscan sample
993 of the 7350 overlapping tile pairs pick a different winner under the two
orders, so the seam and corner regions are exactly where the order decides the
bytes.

CHANNEL ORDER: the container stores B,G,R (`PixelType = Bgr24`), but `czifile`
returns the JPEG-XR tiles as R,G,B (it does not swap BGR->RGB for compression
4, because the decoder already returns R,G,B), and that is the order
`phasit-imread` publishes. The `.npy` files therefore hold R,G,B.

Run: python dump_ref_czi_jxr.py PATH_TO_CZI [--out DIR] [--size 1024]
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

from czi_jxr_geometry import Mosaic, Tile, orders_agree

REGION_SIZE = 1024

# czifile's default fill value for the pixels no subblock covers
# (`czifile.py:556`, `:1386`, `:2509`); `phasit-imread` fills them with zero
# too (`src/czi_reader.rs`, `CziReader::plane` and `CziReader::region` — the
# composed buffer starts zeroed, so the uncovered pixels stay zero).
FILL_VALUE = 0

# `czifile.py:4396` — the BGR->RGB swap is applied for every compression
# EXCEPT 1 (JpgFile), 4 (JpegXR), 5 and 6 (Zstd): for those the decoder
# already returns the samples in the published order.
NO_SWAP_COMPRESSIONS = frozenset({1, 4, 5, 6})

MOSAIC_ORDER = "mosaic_index_ascending"
DIRECTORY_ORDER = "subblock_directory_order"

# What each composition order MEANS, recorded in the manifest next to the order
# itself. The manifest records the order the dump was ACTUALLY made with, so a
# dump made the other way round says so, and the parity test fails loudly
# instead of certifying it.
ORDER_NOTES = {
    MOSAIC_ORDER: ("czifile composites the tiles of a mosaic in ascending mosaic "
                   "index, so the tile with the higher M ends up on top "
                   "(czifile.py:1308, czifile.py:1309, applied at czifile.py:2810 "
                   "and czifile.py:2811)."),
    DIRECTORY_ORDER: ("the tiles were composited in SUBBLOCK-DIRECTORY order, "
                       "which is NOT czifile's order: czifile composites a mosaic "
                       "in ascending mosaic index (czifile.py:1308, czifile.py:1309, "
                       "applied at czifile.py:2810 and czifile.py:2811). A reference "
                       "made this way does not certify the reader's composition."),
}


def level0_tiles(czi) -> tuple[list[Tile], dict[int, object], dict[tuple, list[Tile]]]:
    """The full-resolution tiles in subblock directory order, with their entries.

    The directory order is the order the tiles are read in; the manifest
    records it beside the mosaic-index order, because the two are not the same
    in every file. The tiles are also grouped per PLANE, because the sort that
    decides the composition order runs per plane.
    """
    tiles = []
    entries = {}
    planes: dict[tuple, list[Tile]] = {}
    for dir_index, entry in enumerate(czi.subblock_directory):
        if entry.is_pyramid or entry.stored_shape != entry.shape:
            continue
        starts = dict(zip(entry.dims, entry.start))
        x0 = int(starts["X"])
        y0 = int(starts["Y"])
        tile = Tile(
            x0,
            y0,
            x0 + entry.shape[entry.dims.index("X")],
            y0 + entry.shape[entry.dims.index("Y")],
            entry.mosaic_index,
            dir_index,
        )
        tiles.append(tile)
        entries[dir_index] = entry
        key = tuple(int(starts[d]) for d in ("C", "Z", "T") if d in starts)
        planes.setdefault(key, []).append(tile)
    return tiles, entries, planes


def pixel_layout(entry) -> tuple[int, np.dtype, bool]:
    """The samples, the dtype and whether `czifile` swaps the channels.

    All three come from the subblock itself — its pixel type and its
    compression — so a subblock of another layout is never mistaken for this
    one.
    """
    samples = entry.pixel_type.samples
    dtype = entry.pixel_type.dtype
    swaps = int(entry.compression) not in NO_SWAP_COMPRESSIONS and samples in {3, 4}
    return samples, dtype, swaps


def decode_tile(czi, entry, tile: Tile) -> np.ndarray:
    """The tile's pixels, in the order `czifile` publishes them.

    `czifile` decodes JPEG-XR with WIC on Windows and with imagecodecs
    elsewhere; both must agree with imagecodecs' own jxrlib binding, else the
    ground truth would depend on the machine that produced it.
    """
    subblock = entry.read_segment_data(czi)
    pixels = np.asarray(subblock.data())
    decoded = imagecodecs.jpegxr_decode(bytes(subblock.data(raw=True)))
    if pixels.size != decoded.size or not np.array_equal(
        pixels.reshape(decoded.shape), decoded
    ):
        raise SystemExit(
            f"czifile and imagecodecs disagree on the tile at {tile.x0},{tile.y0}"
        )
    return pixels.reshape(decoded.shape)


def compose(czi, entries, mosaic: Mosaic, x, y, w, h, order, plane_tiles):
    """The window's pixels, composed in the given order.

    `czifile` composites a mosaic in ascending mosaic index and fills the
    pixels no tile covers with its default fill value 0. The order that was
    ACTUALLY used is returned with the pixels: when the sort does not run, the
    tiles are drawn in the subblock-directory order, and the manifest has to
    say that.
    """
    used = mosaic.intersecting(x, y, w, h)
    effective = order
    if order == MOSAIC_ORDER:
        # `czifile` sorts the tiles of a mosaic by mosaic index ONLY when some
        # entry of the file carries a non-negative index (`czifile.py:1308`).
        # The reader evaluates that condition per PLANE, and so does this
        # producer: a plane whose tiles all carry a negative index is not
        # sorted, and the directory order stands. (czifile evaluates it over
        # the whole file's filtered directory — a documented divergence.)
        if any(t.mosaic_index >= 0 for t in plane_tiles):
            # The sort is stable, so tiles that share an index keep their
            # directory order — which is what the `(mosaic_index, dir_index)`
            # key spells out.
            used.sort(key=lambda t: (t.mosaic_index, t.dir_index))
        else:
            effective = DIRECTORY_ORDER
    else:
        used.sort(key=lambda t: t.dir_index)
    samples, dtype, _ = pixel_layout(entries[used[0].dir_index]) if used else (1, np.uint8, False)
    out = np.full((h, w, samples), FILL_VALUE, dtype=dtype)
    for tile in used:
        pixels = decode_tile(czi, entries[tile.dir_index], tile)
        if pixels.ndim == 2:
            pixels = pixels[:, :, None]
        ix0, ix1 = max(x, tile.x0), min(x + w, tile.x1)
        iy0, iy1 = max(y, tile.y0), min(y + h, tile.y1)
        out[iy0 - y : iy1 - y, ix0 - x : ix1 - x] = pixels[
            iy0 - tile.y0 : iy1 - tile.y0, ix0 - tile.x0 : ix1 - tile.x0
        ]
    return out, used, effective


def region_record(name, kind, x, y, w, h, used, mosaic: Mosaic, order) -> dict:
    """The manifest entry of one region.

    `order` is the order the region was ACTUALLY composited in — it is passed
    in, never assumed, so the manifest cannot claim the mosaic order for a
    region that was drawn in the directory order.
    """
    mosaic_order = [
        t.dir_index for t in sorted(used, key=lambda t: (t.mosaic_index, t.dir_index))
    ]
    directory_order = [t.dir_index for t in sorted(used, key=lambda t: t.dir_index)]
    covered = mosaic.covered_pixels(x, y, w, h)
    return {
        "name": name,
        "kind": kind,
        "x": x,
        "y": y,
        "w": w,
        "h": h,
        "subblocks": [
            {
                "mosaic_index": t.mosaic_index,
                "dir_index": t.dir_index,
                "x0": t.x0,
                "y0": t.y0,
                "x1": t.x1,
                "y1": t.y1,
            }
            for t in sorted(used, key=lambda t: t.dir_index)
        ],
        "tile_count": len(used),
        "composition_order": order,
        "mosaic_order": mosaic_order,
        "directory_order": directory_order,
        # The two orders are compared PER PIXEL, from the tile rectangles: the
        # orders can list the tiles differently and still win the same pixel,
        # and the winner is what decides the bytes. This is the same method
        # `dump_ref_czi.py` uses, so the `orders_agree` of the two manifests
        # means the same thing.
        "orders_agree": orders_agree(used, x, y, w, h),
        "covered_pixels": covered,
        "uncovered_pixels": w * h - covered,
    }


def library_versions() -> dict:
    """The versions of the libraries the reference depends on."""
    return {
        "czifile": getattr(czifile, "__version__", "unknown"),
        "imagecodecs": imagecodecs.__version__,
        "numpy": np.__version__,
        "python": platform.python_version(),
    }


def sample_identity(path: str) -> dict:
    """The sample's name, size and md5, so a reference is tied to its input."""
    digest = hashlib.md5()
    size = 0
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
            size += len(chunk)
    return {"file": Path(path).name, "size": size, "md5": digest.hexdigest()}


def dump_region(out_dir: Path, name: str, pixels: np.ndarray) -> tuple[str, str]:
    """Write one region and return its file name and sha256."""
    path = out_dir / f"{name}.npy"
    np.save(path, pixels)
    return path.name, hashlib.sha256(path.read_bytes()).hexdigest()


def parse_args(argv):
    parser = argparse.ArgumentParser(
        description="Dump reference regions of a real JPEG-XR CZI (no default paths)."
    )
    parser.add_argument("path", help="the .czi file to read")
    parser.add_argument("--out", default=None, help="the folder the regions are "
                        "written to (default: tests/parity/_ref/czi_jxr here)")
    parser.add_argument("--size", type=int, default=REGION_SIZE,
                        help=f"the region edge length in pixels (default: {REGION_SIZE})")
    parser.add_argument("--order", choices=[MOSAIC_ORDER, DIRECTORY_ORDER],
                        default=MOSAIC_ORDER,
                        help="the order the tiles are composited in; the default is "
                             "czifile's own order")
    args = parser.parse_args(argv)
    if args.size <= 0:
        parser.error("--size must be positive")
    return args


def build_regions(czi, entries, mosaic: Mosaic, out_dir: Path, size: int, order: str,
                  plane_tiles):
    """Dump the single-tile regions and return their manifest entries."""
    regions = []
    for name, anchor in mosaic.anchor_tiles().items():
        for x, y in mosaic.single_tile_candidates(anchor, size):
            pixels, used, effective = compose(czi, entries, mosaic, x, y, size, size,
                                              order, plane_tiles)
            if pixels.any():
                break
        else:
            raise SystemExit(f"no non-empty window inside the {name} tile")
        file_name, digest = dump_region(out_dir, name, pixels)
        record = region_record(name, "single_tile", x, y, size, size, used, mosaic, effective)
        record["npy_path"] = file_name
        record["sha256"] = digest
        regions.append(record)
    return regions


def build_overlap_regions(czi, entries, mosaic: Mosaic, out_dir: Path, size: int, order: str,
                          plane_tiles):
    """Dump the windows that exercise the assembly of the mosaic."""
    windows = [(f"seam_{i}", "seam", w) for i, w in enumerate(mosaic.pick_seams(size, 2))]
    windows.append(("corner", "corner", mosaic.pick_corner(size)))
    windows.append(
        ("interior", "interior", mosaic.nearest_to_centre(mosaic.interior_windows(size), size))
    )
    windows.append(("edge", "scene_edge", mosaic.pick_edge(size)))
    regions = []
    for name, kind, window in windows:
        x, y = window[0], window[1]
        pixels, used, effective = compose(czi, entries, mosaic, x, y, size, size,
                                          order, plane_tiles)
        file_name, digest = dump_region(out_dir, name, pixels)
        record = region_record(name, kind, x, y, size, size, used, mosaic, effective)
        record["npy_path"] = file_name
        record["sha256"] = digest
        regions.append(record)
    return regions


def build_manifest(path: str, mosaic: Mosaic, regions: list[dict], order: str,
                   requested: str) -> dict:
    """The manifest of the reference set: what it is, and how it was made.

    `order` is the order the regions were ACTUALLY composited in — it is
    passed in, never assumed. A dump made with `--order subblock_directory_order`
    therefore records exactly that, and the parity test that reads the manifest
    fails loudly instead of certifying a reference that was made the other way
    round. `requested` is the order the run asked for, so the manifest says
    when the two differ.
    """
    return {
        "generator": "dump_ref_czi_jxr.py",
        "sample": sample_identity(path),
        "libraries": library_versions(),
        "order": "rgb",
        "order_note": ("czifile returns R,G,B for this JPEG-XR CZI (it does not swap "
                       "BGR->RGB for compression 4), which is the order phasit-imread "
                       "publishes; the container stores B,G,R."),
        "composition": {
            "reference_order": order,
            "reference_order_note": ORDER_NOTES[order],
            "requested_order": requested,
            "reader_order": MOSAIC_ORDER,
            "reader_order_note": ("phasit-imread composites the tiles of a plane "
                                   "in ascending mosaic index too, so the tile "
                                   "with the higher M ends up on top "
                                   "(src/czi_reader.rs, CziReader::open — the "
                                  "per-plane mosaic sort). Every region "
                                   "still records the subblock-directory order "
                                   "beside the mosaic order, because the two "
                                   "are not the same in every file (993 of the "
                                   "7350 overlapping pairs of the Axioscan "
                                   "sample pick a different winner under them)."),
            "fill_value": {"czifile": FILL_VALUE, "phasit_imread": FILL_VALUE},
        },
        "scene": {
            "origin_x": mosaic.origin_x,
            "origin_y": mosaic.origin_y,
            "width": mosaic.width,
            "height": mosaic.height,
            "dims": {"t": 1, "c": 1, "z": 1, "y": mosaic.height, "x": mosaic.width},
            "tiles": len(mosaic.tiles),
        },
        "regions": regions,
    }


def main(argv=None) -> int:
    args = parse_args(argv)
    here = Path(__file__).resolve().parent
    out_dir = Path(args.out) if args.out else here / "_ref" / "czi_jxr"
    out_dir.mkdir(parents=True, exist_ok=True)

    started = time.perf_counter()
    with czifile.CziFile(args.path) as czi:
        tiles, entries, planes = level0_tiles(czi)
        if len(planes) != 1:
            raise SystemExit(
                f"the JPEG-XR reference producer composes ONE plane at a time, but "
                f"this file holds {len(planes)}: {sorted(planes)}"
            )
        plane_tiles = next(iter(planes.values()))
        mosaic = Mosaic(tiles)
        print(f"{len(tiles)} level-0 tiles; scene bbox x={mosaic.origin_x}.."
              f"{mosaic.origin_x + mosaic.width} y={mosaic.origin_y}.."
              f"{mosaic.origin_y + mosaic.height}")
        regions = build_regions(czi, entries, mosaic, out_dir, args.size, args.order,
                                plane_tiles)
        regions += build_overlap_regions(czi, entries, mosaic, out_dir, args.size,
                                         args.order, plane_tiles)
        for record in regions:
            print(f"  {record['name']}: scene x={record['x']} y={record['y']} "
                  f"{record['w']}x{record['h']} tiles={record['tile_count']} "
                  f"composited={record['composition_order']} "
                  f"orders_agree={record['orders_agree']} "
                  f"covered={record['covered_pixels']} "
                  f"sha256={record['sha256'][:16]}")

    # The manifest names the order the regions were ACTUALLY composited in: a
    # plane whose tiles all carry a negative mosaic index is not sorted, and
    # the regions were drawn in the subblock-directory order.
    effective = {region["composition_order"] for region in regions}
    if len(effective) != 1:
        raise SystemExit(
            f"the regions were composited in {len(effective)} orders: {sorted(effective)}"
        )
    doc = build_manifest(args.path, mosaic, regions, effective.pop(), args.order)
    (out_dir / "manifest.json").write_text(json.dumps(doc, indent=2), encoding="utf-8")
    print(f"wrote {len(regions)} regions + manifest.json to {out_dir} "
          f"in {time.perf_counter() - started:.1f} s")
    return 0


if __name__ == "__main__":
    sys.exit(main())
