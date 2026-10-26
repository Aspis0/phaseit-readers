Local patches to vendored jxrlib
================================

This is a vendored copy of Glencoe Software's jxrlib (itself forked from
Microsoft's JPEG-XR library, BSD-licensed — see the upstream headers).

    upstream:  https://github.com/glencoesoftware/jxrlib
    commit:    19cc1fa19ca6607449d210aea946b3187f808f2c  (master)

Rule: every file of this tree that has an upstream counterpart is upstream's
byte for byte at that commit, with the two exceptions listed below. The
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

### 2. Declare `_byteswap_ulong` where it is defined, so clang can build the tree

- **File:** `image/sys/strcodec.h`, lines 41-56 of the vendored file — the header
  that both `image/sys/strcodec.c` (which defines the function) and
  `image/decode/segdec.c` (which calls it) include.
- **Upstream bug:** no header declares `_byteswap_ulong`, so `segdec.c` calls it
  without a prototype in scope. The macOS CI job (clang) does not compile:
  ```
  vendor/jxrlib/image/decode/segdec.c:66:12: error: call to undeclared function '_byteswap_ulong'; ISO C99 and later do not support implicit function declarations [-Wimplicit-function-declaration]
  ```
  GCC only warns about it, and MSVC never notices, because the CRT declares
  `_byteswap_ulong` for it.
- **Fix:** declare the function in `strcodec.h` under exactly the condition of its
  definition in `strcodec.c` (lines 667-684):
  ```c
  #if (defined(WIN32) && !defined(UNDER_CE)) || (defined(UNDER_CE) && defined(_ARM_))
  // WinCE ARM and Desktop x86
  #else
  // other platform
  #ifndef _BIG__ENDIAN_
  U32 _byteswap_ulong(U32 bits);
  #endif // _BIG__ENDIAN_
  #endif
  ```
  The definition exists only when neither `(WIN32 && !UNDER_CE)` nor
  `(UNDER_CE && _ARM_)` holds, and only when `_BIG__ENDIAN_` is not set: on
  big-endian `strcodec.c` defines a macro instead (`#define _byteswap_ulong(x) (x)`),
  which a declaration would break, and on Win32 the CRT provides the function, so
  declaring it there would clash with the MSVC intrinsic (C2169).
- **How the delta stays checkable:** the patch is the only difference between the
  vendored `image/sys/strcodec.h` and upstream's file; deleting the 16 added lines
  reproduces upstream's bytes exactly, line endings (LF) included.
- **If upstream fixes it:** drop those 16 lines and the vendored file is upstream's
  bytes again; the byte accounting below goes back to 47 identical files and 1
  patched file.

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

- 46 are byte-identical to upstream at 19cc1fa (sha256 compared file by file against
  the files fetched from raw.githubusercontent.com at that commit);
- 2 (`jxrgluelib/JXRGlueJxr.c`, `image/sys/strcodec.h`) differ from upstream only by
  the patches above;
- 2 (`LICENSE`, `PATCHES.md`) are the vendoring's own files and have no upstream
  counterpart.

Every file of this tree that is not upstream's bytes is listed in this file.
