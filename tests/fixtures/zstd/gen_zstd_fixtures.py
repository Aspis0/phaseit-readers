#!/usr/bin/env python
"""Generate Zstd0 / Zstd1(+HiLo) fixtures for the CZI reader parity test.

Uses imagecodecs (BSD) — the SAME codec czifile uses to decode CZI zstd subblocks —
to produce byte-exact ZSTD1/ZSTDHDR streams (with and without the Hi/Lo byte shuffle)
plus a plain Zstd0 stream. For each case we dump:
  <name>.bin  = the compressed bytes (what a CZI subblock would store)
  <name>.raw  = the expected decoded pixels as little-endian bytes (ground truth)
and record shape/dtype/itemsize/hilo in manifest.json.

The Rust side (tests) decodes <name>.bin with phasit-imread's zstd path and asserts
byte-equality with <name>.raw. This tests the Hi/Lo de-interleave that no real sample
on disk currently exercises (both real CZI files are Uncompressed Gray16).

Run:  python gen_zstd_fixtures.py    (writes fixtures next to this script)
"""
import json, os, numpy as np, imagecodecs

HERE = os.path.dirname(os.path.abspath(__file__))

def arrays():
    # u16 microscopy-ish content; HiLo shuffle is data-agnostic but vary it anyway.
    yield "const_u16",   np.full((48, 64), 0x0ABC, np.uint16)
    yield "gradient_u16", (np.arange(64*64, dtype=np.uint16).reshape(64, 64) & 0x0FFF)  # 12-bit ramp
    rng = np.random.default_rng(12345)
    yield "random12_u16", rng.integers(0, 4096, size=(96, 80), dtype=np.uint16)          # 12-bit noise
    yield "extremes_u16", np.array([[0, 0xFFFF, 0x00FF, 0xFF00]*8]*8, np.uint16)
    yield "u8_plane",     rng.integers(0, 256, size=(40, 40), dtype=np.uint8)             # itemsize 1

def main():
    manifest = []
    for name, arr in arrays():
        arr = np.ascontiguousarray(arr)
        itemsize = arr.dtype.itemsize
        raw = arr.tobytes()  # C-order little-endian (host is LE)
        cases = []
        # Zstd0: plain zstd stream (CZI compression id 5)
        cases.append(("zstd0", imagecodecs.zstd_encode(raw), False))
        # Zstd1 without HiLo (id 6, hilo flag off)
        cases.append(("zstd1", imagecodecs.zstd1_encode(arr, itemsize=itemsize, hilo=False), False))
        # Zstd1 with HiLo (id 6, hilo flag on) — the risky de-interleave path
        if itemsize > 1:
            cases.append(("zstd1hilo", imagecodecs.zstd1_encode(arr, itemsize=itemsize, hilo=True), True))
        for variant, enc, hilo in cases:
            # sanity: round-trip in Python with the reference decoder
            dec = imagecodecs.zstd1_decode(enc, itemsize=itemsize)
            assert bytes(dec) == raw, f"{name}/{variant} python round-trip FAILED"
            base = f"{name}__{variant}"
            open(os.path.join(HERE, base + ".bin"), "wb").write(bytes(enc))
            open(os.path.join(HERE, base + ".raw"), "wb").write(raw)
            manifest.append({
                "name": base, "variant": variant, "hilo": hilo,
                "shape": list(arr.shape), "itemsize": itemsize,
                "bin": base + ".bin", "raw": base + ".raw",
                "bin_size": len(bytes(enc)), "raw_size": len(raw),
            })
            print(f"  {base:28s} enc={len(bytes(enc)):6d}  raw={len(raw):6d}  hilo={hilo}")
    doc = {
        "generator": "gen_zstd_fixtures.py",
        "note": "Every fixture is an imagecodecs Zstd0/Zstd1 stream; the .raw file is the expected decode.",
        "fixtures": manifest,
    }
    with open(os.path.join(HERE, "manifest.json"), "w", encoding="utf-8") as fh:
        json.dump(doc, fh, indent=2)
    print(f"wrote {len(manifest)} fixtures + manifest.json to {HERE}")

if __name__ == "__main__":
    main()
