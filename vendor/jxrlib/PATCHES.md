Local patches to vendored jxrlib
================================

This is a vendored copy of Glencoe Software's jxrlib (itself forked from
Microsoft's JPEG-XR library, BSD-licensed — see the upstream headers).

    upstream:  https://github.com/glencoesoftware/jxrlib
    commit:    19cc1fa19ca6607449d210aea946b3187f808f2c  (master)

Rule: every file of this tree that has an upstream counterpart is upstream's
byte for byte at that commit, with the single exception listed below. The
upstream files were fetched from raw.githubusercontent.com at that commit and
compared file by file with sha256sum; "Byte accounting" at the end gives the
result.

Patches
-------

### 1. Free `ctxSC` on `ImageStrDecDecode` failure (native heap leak on untrusted input)

- **File:** `jxrgluelib/JXRGlueJxr.c`, function `PKImageDecode_Copy_WMP`
  (non-`REENTRANT_MODE` branch), lines 2055-2066 of the vendored file.
- **Upstream bug:** After `ImageStrDecInit` allocates the codec context into
  `pID->WMP.ctxSC` (single `malloc` in `image/decode/strdec.c`), the only thing that
  frees it is `ImageStrDecTerm` (`free(pSC)` in `strdec.c`). The original sequence was:
  ```c
  FailIf(ICERR_OK != ImageStrDecInit(...), WMP_errFail);
  FailIf(ICERR_OK != ImageStrDecDecode(...), WMP_errFail);  /* on failure: goto Cleanup */
  FailIf(ICERR_OK != ImageStrDecTerm(...),  WMP_errFail);   /* skipped when Decode fails */
  ```
  If `ImageStrDecDecode` fails partway (truncated / corrupt / crafted JPEG-XR bitstream),
  `FailIf` jumps to `Cleanup` and skips `ImageStrDecTerm`, leaking `ctxSC`. Nothing in
  `PKImageDecode_Release_WMP` frees it either. This is a per-failed-decode native heap
  leak driven by untrusted input (DoS).
- **Fix:** Once `ImageStrDecInit` has succeeded, always run `ImageStrDecTerm` exactly once
  (it is NULL-safe and only tears down + frees; it does not require decode to have
  completed), then propagate the original `WMP_errFail` on decode failure. On the success
  path behaviour is unchanged.
- **Confirmed semantics** (`image/decode/strdec.c`): `ImageStrDecInit` sets `*pctxSC = NULL`
  at entry and only assigns a non-NULL value after all allocation succeeds, so on Init
  failure `ctxSC` is NULL (nothing to free). `ImageStrDecTerm` returns early on NULL and
  must not be called twice (its `free(pSC)` would double-free).
- **Scope note:** The symmetric planar-alpha branch a few lines below has the same
  structural pattern for `ctxSC_Alpha`, but that path is only reached for alpha-bearing
  images (not the CZI microscopy inputs this crate decodes) and was left untouched to keep
  this patch minimal and local.
- **How the delta stays checkable:** the patch is the only difference between the
  vendored `jxrgluelib/JXRGlueJxr.c` and upstream's file; replacing lines 2055-2066
  with upstream's two lines (`ImageStrDecDecode` then `ImageStrDecTerm`) reproduces
  upstream's bytes exactly, line endings (CRLF) included. It is marked
  inline with a `/* phasit: ... */` comment, so it is greppable (`rg "phasit:"`).

Vendoring additions (no upstream counterpart)
---------------------------------------------

- **`LICENSE`** — added by the vendoring. Upstream ships no licence file of its own;
  this is the BSD-2 text taken from the header of `jxrgluelib/JXRGlue.h`.
- **`PATCHES.md`** — this file, added by the vendoring.

File map
--------

- **`Makefile.reference`** — upstream's root `Makefile`, renamed, byte for byte. Its
  comment header carries the three U+FFFD replacement characters that upstream's own
  blob carries (upstream lost the copyright sign and the two bullets there); the
  vendored copy keeps upstream's bytes unchanged. The file is kept for reference only:
  the crate's `build.rs` mirrors the flags it documents and never reads it.
- **`README.md`** — upstream's `README.md`, byte for byte. It describes upstream's own
  Java/SWIG/Maven build, which this vendored copy does not contain: the crate compiles
  the C sources with its own `build.rs` when the `jpegxr` feature is on.

Byte accounting
---------------

50 files in this tree:

- 47 are byte-identical to upstream at 19cc1fa (sha256 compared file by file against
  the files fetched from raw.githubusercontent.com at that commit);
- 1 (`jxrgluelib/JXRGlueJxr.c`) differs from upstream only by the patch above;
- 2 (`LICENSE`, `PATCHES.md`) are the vendoring's own files and have no upstream
  counterpart.

Every file of this tree that is not upstream's bytes is listed in this file.
