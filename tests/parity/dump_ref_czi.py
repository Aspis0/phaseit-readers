"""Dump reference regions of the owner's CZI mosaic as `.npy` (czifile truth).

Every (C,Z) plane of the owner's mosaic is a grid of tiles that OVERLAP in a
narrow band; the mosaic index says which tile wins there. This dumps three
512x512 windows per plane for two planes — one across an X overlap band, one
across a Y overlap band, one inside a single tile — plus a manifest that says
how the reference was made. The window is read by `czifile.imread(path, roi=...,
maxworkers=1, C=c, Z=z, T=t)`; the manifest's "read" note says why the plane is
selected too. A CZI need not have every dimension — the sibling sample has
only C, Y, X and S — so only the dimensions the file HAS are selected, and the
manifest's "read" call names exactly those.

WHAT THIS REFERENCE CERTIFIES: the manifest records, per window, whether the
mosaic order and the subblock-directory order pick the SAME winner for every
pixel of the window (`orders_agree`), and the manifest's `order_census` counts
the overlapping pairs of the whole file that the two orders judge differently.
The owner's mosaic has none: its directory order IS its mosaic order, so a
reference made from it cannot tell the two composition orders apart. It
certifies the geometry, the stride, the dtype and the origin of the file — not
the composition order, which the JPEG-XR reference certifies instead.

THE ORDER THE REFERENCE WAS MADE WITH: the manifest's `reference_order` is the
order `czifile` composes THIS file in — the mosaic order when some entry of the
file carries a non-negative mosaic index (czifile.py:1308-1309), the
subblock-directory order when none does. It is not a constant: a reference
made in one order does not certify a reader that composites in the other.

Run: python dump_ref_czi.py PATH_TO_CZI [--out DIR] [--size 512]
"""
from __future__ import annotations

import argparse
import hashlib
import json
import sys
import time
from pathlib import Path

import czifile
import numpy as np

REGION_SIZE = 512
MOSAIC_ORDER = "mosaic_index_ascending"
DIRECTORY_ORDER = "subblock_directory_order"
TILE_KEYS = ("mosaic_index", "dir_index", "x0", "y0", "x1", "y1")
# The dimensions a plane is keyed by, in the order the reader indexes them.
PLANE_DIMS = ("C", "Z", "T")
ORDER_NOTES = {
    MOSAIC_ORDER: ("the tiles are drawn in ascending mosaic index, so the tile "
                   "with the higher M ends up on top (czifile.py:1308-1309)"),
    DIRECTORY_ORDER: ("the file carries no non-negative mosaic index, so the "
                      "tiles keep their subblock-directory order (czifile.py:1310)"),
}


def plane_key(starts: dict) -> tuple[int, int, int]:
    """The (C, Z, T) key of a tile's plane.

    A CZI need not have every dimension — the sibling sample has only C, Y, X
    and S — so a dimension the file LACKS is 0: the plane's canonical index in
    that dimension is 0 as well, and the reader's shape carries 1 there.
    """
    return tuple(int(starts.get(dim, 0)) for dim in PLANE_DIMS)


def plane_tiles(czi) -> dict:
    """The full-resolution tiles of every plane, keyed by (C, Z, T)."""
    planes: dict[tuple[int, int, int], list[dict]] = {}
    for dir_index, entry in enumerate(czi.subblock_directory):
        if entry.is_pyramid or entry.stored_shape != entry.shape:
            continue
        starts = dict(zip(entry.dims, entry.start))
        sizes = dict(zip(entry.dims, entry.shape))
        x0, y0 = int(starts["X"]), int(starts["Y"])
        planes.setdefault(plane_key(starts), []).append(
            {"mosaic_index": int(entry.mosaic_index), "dir_index": dir_index,
             "x0": x0, "y0": y0, "x1": x0 + int(sizes["X"]), "y1": y0 + int(sizes["Y"])})
    for tiles in planes.values():
        tiles.sort(key=lambda t: (t["y0"], t["x0"], t["dir_index"]))
    return planes


def available_dims(czi) -> set:
    """The dimension names the file actually has (a CZI need not have Z or T).

    The same rule as `crosscheck_ref_czi.py`, so both scripts select the same
    dimensions of the same file.
    """
    scene = next(iter(czi.scenes.values()))
    return set(scene._raw_sizes)


def effective_order(czi) -> str:
    """The order `czifile` composes THIS file's tiles in.

    `czifile` sorts the filtered subblock directory by mosaic index when some
    entry of it carries a non-negative index (czifile.py:1308-1309); without
    one the tiles keep their subblock-directory order (czifile.py:1310).
    """
    for entry in czi.subblock_directory:
        if entry.is_pyramid or entry.stored_shape != entry.shape:
            continue
        if entry.mosaic_index >= 0:
            return MOSAIC_ORDER
    return DIRECTORY_ORDER


def reader_order(planes: dict) -> str:
    """The order the reader composes the file's planes in.

    The reader sorts a plane's tiles by mosaic index when one of them carries
    a non-negative index (`CziReader::open`, the per-plane mosaic sort), so a
    file whose every plane sorts is composited in the mosaic order — and a
    file with a plane that does not sort keeps the directory order there.
    """
    if all(any(t["mosaic_index"] >= 0 for t in tiles) for tiles in planes.values()):
        return MOSAIC_ORDER
    return DIRECTORY_ORDER


def selection_call(dims: set) -> str:
    """The `czifile.imread` call the reference was made with.

    Only the dimensions the file HAS are selected: a CZI without Z or T has no
    such plane to select.
    """
    chosen = ", ".join(f"{dim}={dim.lower()}" for dim in PLANE_DIMS if dim in dims)
    call = "czifile.imread(path, roi=(x,y,w,h), maxworkers=1"
    return f"{call}, {chosen})" if chosen else f"{call})"


def window_name(name: str, values: tuple[int, int, int], dims: set) -> str:
    """The file name of a window: its name and the plane it was cut from.

    Only the dimensions the file HAS are named: a CZI without Z or T has no
    such plane to name.
    """
    parts = [name] + [f"{dim.lower()}{value}"
                       for dim, value in zip(PLANE_DIMS, values) if dim in dims]
    return "_".join(parts)


def pick_windows(tiles: list[dict], size: int) -> list[tuple[str, str, int, int]]:
    """The three windows of a plane: the X seam, the Y seam, a tile interior.

    A SEAM is a pair of tiles that overlap in a NARROW band along one axis and a
    wide range along the other — the band the two tiles really share. The seams
    come from the DIRECTORY, not from a hard-coded grid.
    """
    seams = {"x": None, "y": None}
    for i, a in enumerate(tiles):
        for b in tiles[i + 1:]:
            for axis in ("x", "y"):
                other = "y" if axis == "x" else "x"
                lo, hi = max(a[axis + "0"], b[axis + "0"]), min(a[axis + "1"], b[axis + "1"])
                o0, o1 = max(a[other + "0"], b[other + "0"]), min(a[other + "1"], b[other + "1"])
                if hi <= lo or o1 <= o0 or hi - lo >= o1 - o0:
                    continue
                if seams[axis] is None or hi - lo > seams[axis][1] - seams[axis][0]:
                    seams[axis] = (lo, hi, o0, o1)
    if seams["x"] is None or seams["y"] is None:
        raise SystemExit("the plane has no narrow overlap seam to straddle")
    (xs0, xs1, ry0, ry1), (ys0, ys1, rx0, rx1) = seams["x"], seams["y"]
    first = tiles[0]
    return [
        ("overlap_x", "overlap_band_x", xs0 - (size - (xs1 - xs0)) // 2,
         ry0 + max(0, (ry1 - ry0 - size) // 2)),
        ("overlap_y", "overlap_band_y", rx0 + max(0, (rx1 - rx0 - size) // 2),
         ys0 - (size - (ys1 - ys0)) // 2),
        ("interior", "single_tile",
         first["x0"] + max(0, (first["x1"] - first["x0"] - size) // 2),
         first["y0"] + max(0, (first["y1"] - first["y0"] - size) // 2)),
    ]


def intersecting(tiles: list[dict], x: int, y: int, size: int) -> list[dict]:
    """The tiles that cover any part of the window."""
    return [t for t in tiles if t["x0"] < x + size and t["x1"] > x
            and t["y0"] < y + size and t["y1"] > y]


def winner_map(tiles: list[dict], x: int, y: int, size: int) -> np.ndarray:
    """Which tile wins each pixel of the window, for one draw order.

    The tiles are drawn in the given order and the LAST one that covers a
    pixel wins it — that is how a mosaic is composited. The map holds the
    directory index of the winning tile, and -1 where no tile covers the pixel.
    """
    out = np.full((size, size), -1, dtype=np.int64)
    for t in tiles:
        ix0, ix1 = max(x, t["x0"]), min(x + size, t["x1"])
        iy0, iy1 = max(y, t["y0"]), min(y + size, t["y1"])
        if ix0 >= ix1 or iy0 >= iy1:
            continue
        out[iy0 - y:iy1 - y, ix0 - x:ix1 - x] = t["dir_index"]
    return out


def orders_agree(tiles: list[dict], x: int, y: int, size: int) -> bool:
    """Whether the two composition orders pick the same winner, per pixel.

    The comparison is made from the TILE RECTANGLES: the two orders can list
    the tiles differently and still win the same pixel, and the winner is what
    decides the bytes.
    """
    by_mosaic = sorted(tiles, key=lambda t: (t["mosaic_index"], t["dir_index"]))
    by_directory = sorted(tiles, key=lambda t: t["dir_index"])
    return bool(np.array_equal(winner_map(by_mosaic, x, y, size),
                               winner_map(by_directory, x, y, size)))


def order_census(planes: dict) -> dict:
    """How many overlapping pairs of the file the two orders judge differently.

    For a pair of tiles that overlap, the mosaic order and the directory order
    pick a different winner exactly when the mosaic order reverses the pair. A
    file where no pair is reversed cannot tell the two composition orders
    apart at all — that is what the census says about the owner's mosaic.
    """
    pairs = 0
    differing = 0
    for tiles in planes.values():
        for i, a in enumerate(tiles):
            for b in tiles[i + 1:]:
                ox = min(a["x1"], b["x1"]) - max(a["x0"], b["x0"])
                oy = min(a["y1"], b["y1"]) - max(a["y0"], b["y0"])
                if ox <= 0 or oy <= 0:
                    continue
                pairs += 1
                by_mosaic = sorted((a, b), key=lambda t: (t["mosaic_index"], t["dir_index"]))
                by_directory = sorted((a, b), key=lambda t: t["dir_index"])
                if by_mosaic[-1]["dir_index"] != by_directory[-1]["dir_index"]:
                    differing += 1
    return {"overlapping_pairs": pairs, "pairs_with_a_different_winner": differing}


def dump_region(out_dir: Path, czi_path: str, window: tuple[str, str, int, int],
                plane: dict) -> dict:
    """Read one window with czifile, write it, and describe it in the manifest."""
    name, kind, x, y = window
    c, z, t = plane["values"]
    size = plane["size"]
    selection = {k: v for k, v in (("C", c), ("Z", z), ("T", t)) if k in plane["dims"]}
    # The single-tile guard runs BEFORE the file is written: a window labelled
    # "single_tile" that several tiles overlap would not be a single-tile
    # window at all, and a reference set must not hold a file the manifest
    # does not describe.
    used = intersecting(plane["tiles"], x, y, size)
    if kind == "single_tile" and len(used) != 1:
        raise SystemExit(
            f"the interior window {name} of plane c={c} z={z} t={t} is covered by "
            f"{len(used)} tiles, not by one: a window labelled 'single_tile' that "
            f"several tiles overlap would not be a single-tile window at all"
        )
    pixels = czifile.imread(czi_path, roi=(x, y, size, size), maxworkers=1, **selection)
    path = out_dir / f"{window_name(name, plane['values'], plane['dims'])}.npy"
    np.save(path, pixels)
    return {
        "name": name, "kind": kind, "dtype": str(pixels.dtype),
        "plane": {"c": plane["c_index"], "z": plane["z_index"], "t": plane["t_index"],
                  "c_value": c, "z_value": z, "t_value": t},
        "x": x, "y": y, "w": size, "h": size, "tile_count": len(used),
        "npy_path": path.name,
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        "tiles_intersecting": [{k: u[k] for k in TILE_KEYS}
                               for u in sorted(used, key=lambda u: u["dir_index"])],
        "mosaic_order": [u["dir_index"]
                         for u in sorted(used, key=lambda u: (u["mosaic_index"], u["dir_index"]))],
        "directory_order": [u["dir_index"]
                            for u in sorted(used, key=lambda u: u["dir_index"])],
        "orders_agree": orders_agree(used, x, y, size),
    }


def dump_planes(czi_path: str, out_dir: Path, size: int, planes: dict, picked: list,
                dims: set, c_values: list, z_values: list, t_values: list) -> list[dict]:
    """Dump the windows of the picked planes and return their manifest entries."""
    regions = []
    for c, z, t in picked:
        plane = {
            "values": (c, z, t), "size": size, "dims": dims,
            "tiles": planes[(c, z, t)],
            "c_index": c_values.index(c), "z_index": z_values.index(z),
            "t_index": t_values.index(t),
        }
        for window in pick_windows(plane["tiles"], size):
            record = dump_region(out_dir, czi_path, window, plane)
            regions.append(record)
            print(f"  {record['name']} c={c} z={z} t={t}: x={record['x']} y={record['y']} "
                  f"tiles={record['tile_count']} orders_agree={record['orders_agree']}")
    return regions


def prune(out_dir: Path, regions: list[dict]) -> list[str]:
    """Remove the `.npy` files the reference set does not name.

    The output folder holds the reference set and nothing else: a file left
    behind by an earlier run — a window that moved, a plane that was dropped —
    would be a file the manifest does not describe.
    """
    kept = {region["npy_path"] for region in regions}
    removed = []
    for path in sorted(out_dir.glob("*.npy")):
        if path.name not in kept:
            path.unlink()
            removed.append(path.name)
    return removed


def build_manifest(path: str, scene: dict, composition: dict, planes: list[dict],
                   regions: list[dict], dims: set) -> dict:
    """The manifest of the reference set: what it is, and how it was made."""
    return {
        "generator": "dump_ref_czi.py",
        "libraries": {"czifile": czifile.__version__, "numpy": np.__version__,
                      "python": sys.version.split()[0]},
        "sample": {"file": Path(path).name, "size": Path(path).stat().st_size},
        "scene": scene,
        "composition": composition,
        "read": {"call": selection_call(dims),
                 "note": ("the plane is selected on purpose: a bare roi= re-sorts the "
                          "tiles by X start (czifile.py:1849, :3157) and composites in "
                          "another order than czifile's own full-plane path (:2810-2811); "
                          "maxworkers=1 makes the winner deterministic (:2793-2801). "
                          "Only the dimensions the file HAS are selected.")},
        "planes": planes,
        "regions": regions,
    }


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description="Dump reference regions of a CZI mosaic.")
    parser.add_argument("path", help="the .czi file to read")
    parser.add_argument("--out", default=None, help="the output folder (default: "
                        "tests/parity/_ref/czi_mosaic here)")
    parser.add_argument("--size", type=int, default=REGION_SIZE, help="region edge in px")
    args = parser.parse_args(argv)
    if args.size <= 0:
        parser.error("--size must be positive")
    out_dir = Path(args.out) if args.out else Path(__file__).resolve().parent / "_ref" / "czi_mosaic"
    out_dir.mkdir(parents=True, exist_ok=True)
    started = time.perf_counter()
    with czifile.CziFile(args.path) as czi:
        planes = plane_tiles(czi)
        keys = sorted(planes)
        if len(keys) < 2:
            raise SystemExit("the file has fewer than two planes")
        picked = [keys[0], keys[-1]]
        dims = available_dims(czi)
        # The scene bounding box, and the canonical (C, Z, T) indices the reader
        # uses: the position of the value in the sorted distinct list.
        every = [t for tiles in planes.values() for t in tiles]
        origin_x, origin_y = min(t["x0"] for t in every), min(t["y0"] for t in every)
        c_values = sorted({k[0] for k in planes})
        z_values = sorted({k[1] for k in planes})
        t_values = sorted({k[2] for k in planes})
        scene = {"origin_x": origin_x, "origin_y": origin_y, "tiles": len(every),
                 "width": max(t["x1"] for t in every) - origin_x,
                 "height": max(t["y1"] for t in every) - origin_y}
        reference_order = effective_order(czi)
        composition = {
            "reference_order": reference_order,
            "reference_order_note": ORDER_NOTES[reference_order],
            "reader_order": reader_order(planes),
            "reader_order_note": ("the reader sorts a plane's tiles by mosaic index "
                                  "when one of them carries a non-negative index, so a "
                                  "plane that does not sort keeps the directory order "
                                  "(CziReader::open, the per-plane mosaic sort)"),
            "fill_value": {"czifile": 0, "phasit_imread": 0},
            "order_census": {
                **order_census(planes),
                "note": ("the mosaic order and the subblock-directory order "
                         "pick the SAME winner for every overlapping pair of "
                         "this file, so this reference cannot tell the two "
                         "composition orders apart: it certifies the geometry, "
                         "the stride, the dtype and the origin of the file. "
                         "The composition ORDER is certified by the JPEG-XR "
                         "reference (tests/czi_jxr_real.rs, the Axioscan "
                         "sample), where the two orders do disagree.")}}
        regions = dump_planes(args.path, out_dir, args.size, planes, picked,
                              dims, c_values, z_values, t_values)
    planes_meta = [{"c": c_values.index(c), "z": z_values.index(z), "t": t_values.index(t),
                    "c_value": c, "z_value": z, "t_value": t} for c, z, t in picked]
    doc = build_manifest(args.path, scene, composition, planes_meta, regions, dims)
    (out_dir / "manifest.json").write_text(json.dumps(doc, indent=2), encoding="utf-8")
    # The manifest names the reference set; a file it does not name is not part
    # of it.
    removed = prune(out_dir, regions)
    if removed:
        print(f"removed {len(removed)} stale file(s): {', '.join(removed)}")
    print(f"wrote {len(regions)} regions + manifest.json to {out_dir} "
          f"in {time.perf_counter() - started:.1f} s")
    return 0


if __name__ == "__main__":
    sys.exit(main())
