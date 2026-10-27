"""The geometry of a CZI mosaic, and the windows that exercise it.

Pure geometry: it works on plain tile rectangles, never opens a file and never
decodes a pixel, so the window picking can be read (and re-run) on its own.

A tile carries the two composition orders a reader can use:

* `mosaic_index` — the CZI M coordinate. `czifile` composites the tiles of a
  mosaic in ASCENDING mosaic index, so the tile with the higher M ends up on
  top (`czifile.py:1308-1309`, `czifile.py:2810-2811`). `phasit-imread`
  composites them in the same order (`src/czi_reader.rs`, `CziReader::open` — the
  per-plane mosaic sort).
* `dir_index` — the position of the tile in the subblock directory.

The two orders are not the same in every file, which is why the windows below
are recorded with the tiles that take part in them: the windows are picked
where the two orders pick a DIFFERENT WINNER, because that is where the order
decides the bytes. The comparison is made per pixel, from the tile rectangles
(`orders_agree` below): the two orders can list the tiles differently and still
win the same pixel.
"""
from __future__ import annotations

import numpy as np

# The grid the window positions inside a tile are tried on.
SEARCH_STEP = 128


class Tile:
    """One full-resolution tile of a mosaic, in scene coordinates."""

    __slots__ = ("x0", "y0", "x1", "y1", "mosaic_index", "dir_index")

    def __init__(self, x0, y0, x1, y1, mosaic_index, dir_index):
        self.x0 = int(x0)
        self.y0 = int(y0)
        self.x1 = int(x1)
        self.y1 = int(y1)
        self.mosaic_index = int(mosaic_index)
        self.dir_index = int(dir_index)

    @property
    def width(self) -> int:
        return self.x1 - self.x0

    @property
    def height(self) -> int:
        return self.y1 - self.y0

    def __repr__(self) -> str:
        return (
            f"Tile(x0={self.x0}, y0={self.y0}, x1={self.x1}, y1={self.y1}, "
            f"mi={self.mosaic_index}, dir={self.dir_index})"
        )


def _union_length(spans: list[tuple[int, int]]) -> int:
    """The total length of (start, end) spans, merged."""
    total = 0
    cur_start = cur_end = None
    for start, end in sorted(spans):
        if cur_start is None:
            cur_start, cur_end = start, end
        elif start <= cur_end:
            cur_end = max(cur_end, end)
        else:
            total += cur_end - cur_start
            cur_start, cur_end = start, end
    if cur_start is not None:
        total += cur_end - cur_start
    return total


class Mosaic:
    """The full-resolution tiles of one scene, and the geometry between them."""

    def __init__(self, tiles: list[Tile]):
        self.tiles = list(tiles)
        self.neighbours = self._index_neighbours()
        self.origin_x = min(t.x0 for t in self.tiles)
        self.origin_y = min(t.y0 for t in self.tiles)
        self.width = max(t.x1 for t in self.tiles) - self.origin_x
        self.height = max(t.y1 for t in self.tiles) - self.origin_y
        self._cell = max(1, max(t.width for t in self.tiles), max(t.height for t in self.tiles))
        self._grid = self._index_grid()

    def _index_neighbours(self) -> dict[Tile, list[tuple[Tile, int, int]]]:
        """For every tile, the tiles it overlaps, with the overlap sizes.

        The X-overlap is the size of the band the two tiles share in X, the
        Y-overlap the size of the band they share in Y. Two neighbours on the
        same row share a NARROW X band (the seam) and a wide Y band; two
        neighbours in the same column share it the other way round.
        """
        index: dict[Tile, list[tuple[Tile, int, int]]] = {t: [] for t in self.tiles}
        order = sorted(self.tiles, key=lambda t: t.x0)
        for pos, a in enumerate(order):
            for b in order[pos + 1 :]:
                if b.x0 >= a.x1:
                    break
                ox = min(a.x1, b.x1) - max(a.x0, b.x0)
                oy = min(a.y1, b.y1) - max(a.y0, b.y0)
                if ox > 0 and oy > 0:
                    index[a].append((b, ox, oy))
                    index[b].append((a, ox, oy))
        return index

    def _index_grid(self) -> dict[tuple[int, int], list[Tile]]:
        """A coarse grid of the tiles, so a window lookup stays cheap."""
        grid: dict[tuple[int, int], list[Tile]] = {}
        for tile in self.tiles:
            for cx in range(tile.x0 // self._cell, (tile.x1 - 1) // self._cell + 1):
                for cy in range(tile.y0 // self._cell, (tile.y1 - 1) // self._cell + 1):
                    grid.setdefault((cx, cy), []).append(tile)
        return grid

    def side_by_side(self, a: Tile, b: Tile, ox: int, oy: int, size: int) -> bool:
        """Neighbours on the same row: a narrow X seam, a wide Y band."""
        return ox < min(a.width, b.width) and oy >= size

    def stacked(self, a: Tile, b: Tile, ox: int, oy: int, size: int) -> bool:
        """Neighbours in the same column: a wide X band, a narrow Y seam."""
        return oy < min(a.height, b.height) and ox >= size

    def intersecting(self, x: int, y: int, w: int, h: int) -> list[Tile]:
        """The tiles that cover any part of the window, in directory order.

        The grid only narrows the candidates down to the cells the window
        touches; every candidate still has to pass the exact rectangle test,
        because a tile that shares a cell with the window need not overlap it.
        """
        found: dict[Tile, None] = {}
        for cx in range(x // self._cell, (x + w - 1) // self._cell + 1):
            for cy in range(y // self._cell, (y + h - 1) // self._cell + 1):
                for tile in self._grid.get((cx, cy), ()):
                    if (tile.x0 < x + w and tile.x1 > x
                            and tile.y0 < y + h and tile.y1 > y):
                        found[tile] = None
        return sorted(found, key=lambda t: t.dir_index)

    def covered_pixels(self, x: int, y: int, w: int, h: int) -> int:
        """How many pixels of the window some tile covers (exact union area)."""
        rects = []
        for t in self.intersecting(x, y, w, h):
            ix0, ix1 = max(x, t.x0), min(x + w, t.x1)
            iy0, iy1 = max(y, t.y0), min(y + h, t.y1)
            if ix0 < ix1 and iy0 < iy1:
                rects.append((ix0, iy0, ix1, iy1))
        if not rects:
            return 0
        edges = sorted({r[0] for r in rects} | {r[2] for r in rects})
        total = 0
        for xa, xb in zip(edges, edges[1:]):
            spans = [(r[1], r[3]) for r in rects if r[0] <= xa and r[2] >= xb]
            total += (xb - xa) * _union_length(spans)
        return total

    def inside_scene(self, x: int, y: int, w: int, h: int) -> bool:
        """Whether the window lies inside the scene bounding box."""
        return (
            x >= self.origin_x
            and y >= self.origin_y
            and x + w <= self.origin_x + self.width
            and y + h <= self.origin_y + self.height
        )

    def seam_windows(self, size: int) -> list[tuple[int, int, list[Tile]]]:
        """Windows straddling the seam of two side-by-side tiles.

        The window is centred on the band the two tiles share, so it holds the
        pixels only the left tile covers, the band where BOTH cover, and the
        pixels only the right tile covers.
        """
        found = []
        for a in self.tiles:
            for b, ox, oy in self.neighbours[a]:
                if b.dir_index < a.dir_index:
                    continue  # every pair is visited once
                if not self.side_by_side(a, b, ox, oy, size):
                    continue
                left, right = (a, b) if a.x0 <= b.x0 else (b, a)
                band_x0, band_x1 = right.x0, left.x1
                y0 = max(left.y0, right.y0)
                y1 = min(left.y1, right.y1)
                if y1 - y0 < size or band_x1 - band_x0 > size:
                    continue
                x = band_x0 - (size - (band_x1 - band_x0)) // 2
                y = y0 + (y1 - y0 - size) // 2
                if self.inside_scene(x, y, size, size):
                    found.append((x, y, [left, right]))
        return found

    def corner_windows(self, size: int) -> list[tuple[int, int, list[Tile]]]:
        """Windows straddling a corner where four tiles meet.

        A corner is a tile with a side-by-side neighbour (the `side`) and a
        stacked neighbour (the `row`) whose seams cross: the three share a
        small rectangle, and a fourth tile — side by side with the row and
        stacked with the side — closes the corner.
        """
        found = []
        for t in self.tiles:
            sides = [o for o, ox, oy in self.neighbours[t]
                     if self.side_by_side(t, o, ox, oy, size)]
            stacks = [o for o, ox, oy in self.neighbours[t]
                      if self.stacked(t, o, ox, oy, size)]
            for side in sides:
                for row in stacks:
                    core_x0 = max(t.x0, side.x0)
                    core_x1 = min(t.x1, side.x1)
                    core_y0 = max(t.y0, row.y0)
                    core_y1 = min(t.y1, row.y1)
                    if core_x1 <= core_x0 or core_y1 <= core_y0:
                        continue
                    if self._diagonal(side, row, {t, side, row}) is None:
                        continue
                    x = (core_x0 + core_x1) // 2 - size // 2
                    y = (core_y0 + core_y1) // 2 - size // 2
                    if not self.inside_scene(x, y, size, size):
                        continue
                    taken = self.intersecting(x, y, size, size)
                    if len(taken) < 4:
                        continue
                    found.append((x, y, taken))
        return found

    def _diagonal(self, side: Tile, row: Tile, exclude: set) -> Tile | None:
        """The tile that closes the corner: side by side with `row` and
        stacked with `side`."""
        for candidate, ox, oy in self.neighbours[row]:
            if candidate in exclude:
                continue
            if not self.side_by_side(row, candidate, ox, oy, 1):
                continue
            for other, ox2, oy2 in self.neighbours[side]:
                if other is not candidate:
                    continue
                if self.stacked(side, candidate, ox2, oy2, 1):
                    return candidate
        return None

    def interior_windows(self, size: int) -> list[tuple[int, int, list[Tile]]]:
        """Windows at the bottom-right of a tile's interior, covered by it
        alone.

        The window is anchored at the tile's bottom-right corner, so it holds
        the tile's bottom-right 40 % corner - far from the top-left corner
        every single-tile window of the first slice sat in.
        """
        found = []
        for t in self.tiles:
            x = t.x1 - size
            y = t.y1 - size
            if x < t.x0 or y < t.y0:
                continue
            others = [o for o, _, _ in self.neighbours[t]
                      if o.x0 < x + size and o.x1 > x and o.y0 < y + size and o.y1 > y]
            if others:
                continue
            if self.inside_scene(x, y, size, size):
                found.append((x, y, [t]))
        return found

    def edge_windows(self, size: int) -> list[tuple[int, int, list[Tile]]]:
        """Windows that hug the scene bounding box, one per position.

        A window of this list may stick out of every tile: the mosaic does not
        fill its own bounding box, and the gap is where the fill value shows.
        """
        found = []
        step = size // 8
        x0, x1 = self.origin_x, self.origin_x + self.width
        y0, y1 = self.origin_y, self.origin_y + self.height
        positions = []
        for x in range(x0, x1 - size + 1, step):
            positions.append((x, y0))
            positions.append((x, y1 - size))
        for y in range(y0, y1 - size + 1, step):
            positions.append((x0, y))
            positions.append((x1 - size, y))
        for x, y in positions:
            found.append((x, y, self.intersecting(x, y, size, size)))
        return found

    def anchor_tiles(self) -> dict[str, Tile]:
        """The tiles that carry the corners and the centre of the mosaic.

        The bounding box of a whole-slide mosaic is usually NOT covered at its
        corners -- the tissue has an irregular outline -- so the corners are
        anchored on the TILES nearest to them.
        """
        max_x = max(t.x1 for t in self.tiles)
        max_y = max(t.y1 for t in self.tiles)
        cx = self.origin_x + self.width // 2
        cy = self.origin_y + self.height // 2
        corners = {
            "top_left": lambda t: (t.x0 - self.origin_x) + (t.y0 - self.origin_y),
            "top_right": lambda t: (max_x - t.x1) + (t.y0 - self.origin_y),
            "bottom_right": lambda t: (max_x - t.x1) + (max_y - t.y1),
            "centre": lambda t: abs(t.x0 + (t.x1 - t.x0) // 2 - cx)
            + abs(t.y0 + (t.y1 - t.y0) // 2 - cy),
        }
        return {name: min(self.tiles, key=key) for name, key in corners.items()}

    def single_tile_candidates(self, anchor: Tile, size: int) -> list[tuple[int, int]]:
        """The window positions inside `anchor` that exactly one tile covers.

        The positions are tried from the anchor's own top-left corner outwards.
        """
        found = []
        for y in range(anchor.y0, anchor.y1 - size + 1, SEARCH_STEP):
            for x in range(anchor.x0, anchor.x1 - size + 1, SEARCH_STEP):
                found.append((x, y))
        found.sort(
            key=lambda p: ((p[0] - anchor.x0) ** 2 + (p[1] - anchor.y0) ** 2, p[1], p[0])
        )
        qualified = []
        for x, y in found:
            if not self.inside_scene(x, y, size, size):
                continue
            if self.intersecting(x, y, size, size) != [anchor]:
                continue
            qualified.append((x, y))
            if len(qualified) >= 8:
                break
        return qualified

    def nearest_to_centre(self, windows, size: int):
        """The window closest to the centre of the scene (ties by position)."""
        cx = self.origin_x + self.width // 2
        cy = self.origin_y + self.height // 2
        return min(
            windows,
            key=lambda w: (
                (w[0] + size // 2 - cx) ** 2 + (w[1] + size // 2 - cy) ** 2,
                w[0],
                w[1],
            ),
        )

    def pick_seams(self, size: int, count: int):
        """`count` well-separated seam windows, the one near the centre first.

        The windows whose tiles the two composition orders judge differently come
        first: there the order decides the bytes, so the window really
        exercises the composition of the mosaic and not only the clipping.

        The windows are DISTINCT: a window already chosen is never chosen
        again, and a mosaic that does not have `count` distinct seam windows
        fails here instead of emitting the same window twice (which would
        silently duplicate a region under a second name).
        """
        found = []
        for x, y, _ in self.seam_windows(size):
            used = self.intersecting(x, y, size, size)
            found.append((x, y, used, order_is_load_bearing(used, x, y, size, size)))
        if not found:
            raise SystemExit("the mosaic has no side-by-side tile pair to straddle")
        chosen = [self.nearest_to_centre(prefer_load_bearing(found), size)]
        while len(chosen) < count:
            taken = {(w[0], w[1]) for w in chosen}
            remaining = [w for w in prefer_load_bearing(found) if (w[0], w[1]) not in taken]
            if not remaining:
                raise SystemExit(
                    f"the mosaic has {len(chosen)} distinct seam window(s) of "
                    f"{size} px, {count} were asked for"
                )
            chosen.append(
                max(
                    remaining,
                    key=lambda w: (
                        (w[0] - chosen[0][0]) ** 2 + (w[1] - chosen[0][1]) ** 2,
                        -w[0],
                        -w[1],
                    ),
                )
            )
        return chosen

    def pick_corner(self, size: int):
        """The window that straddles a four-tile corner, the nearest the centre.

        The corners whose tiles the two composition orders judge differently come
        first: there the order decides the bytes.
        """
        found = [
            (x, y, used, order_is_load_bearing(used, x, y, size, size))
            for x, y, used in self.corner_windows(size)
        ]
        if not found:
            raise SystemExit("the mosaic has no four-tile corner to straddle")
        return self.nearest_to_centre(prefer_load_bearing(found), size)

    def pick_edge(self, size: int):
        """A window hugging the scene bounding box that no tile covers entirely.

        The window is chosen so that a quarter of it is covered by tiles and a
        quarter is not: both the tile content and the fill value are then
        exercised in the same region.
        """
        fallback = None
        for x, y, used in self.edge_windows(size):
            covered = self.covered_pixels(x, y, size, size)
            if covered == 0 or covered == size * size:
                continue
            if fallback is None:
                fallback = (x, y, used)
            if size * size * 0.25 <= covered <= size * size * 0.75:
                return x, y, used
        if fallback is None:
            raise SystemExit("the scene bounding box is covered by tiles everywhere")
        return fallback


def winner_map(tiles, x: int, y: int, w: int, h: int) -> np.ndarray:
    """Which tile wins each pixel of the window, for one draw order.

    The tiles are drawn in the given order and the LAST one that covers a
    pixel wins it — that is how a mosaic is composited. The map holds the
    directory index of the winning tile, and -1 where no tile covers the pixel.
    """
    out = np.full((h, w), -1, dtype=np.int64)
    for t in tiles:
        ix0, ix1 = max(x, t.x0), min(x + w, t.x1)
        iy0, iy1 = max(y, t.y0), min(y + h, t.y1)
        if ix0 >= ix1 or iy0 >= iy1:
            continue
        out[iy0 - y:iy1 - y, ix0 - x:ix1 - x] = t.dir_index
    return out


def orders_agree(tiles, x: int, y: int, w: int, h: int) -> bool:
    """Whether the two composition orders pick the same winner, per pixel.

    The comparison is made from the TILE RECTANGLES: the two orders can list
    the tiles differently and still win the same pixel, and the winner is what
    decides the bytes. This is the same method `dump_ref_czi.py` uses, so the
    `orders_agree` of the two manifests means the same thing.
    """
    by_mosaic = sorted(tiles, key=lambda t: (t.mosaic_index, t.dir_index))
    by_directory = sorted(tiles, key=lambda t: t.dir_index)
    return bool(np.array_equal(winner_map(by_mosaic, x, y, w, h),
                               winner_map(by_directory, x, y, w, h)))


def order_is_load_bearing(used, x: int, y: int, w: int, h: int) -> bool:
    """Whether the order in which the tiles are blitted decides the bytes.

    `czifile` composites in ascending mosaic index and so does `phasit-imread`
    (`src/czi_reader.rs`, `CziReader::open` — the per-plane mosaic sort), but the
    subblock-directory order is NOT the same
    order in every file. When the two orders pick a different winner for a pixel
    of the window, the window is where the composition order is load-bearing —
    the overlap windows are chosen there, because that is what makes a window
    exercise the assembly instead of only the clipping.
    """
    return not orders_agree(used, x, y, w, h)


def prefer_load_bearing(windows):
    """The windows where the composition order decides the bytes, if any."""
    bearing = [w for w in windows if w[3]]
    return bearing if bearing else windows
