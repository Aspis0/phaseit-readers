Local patches to vendored jxrlib
================================

This is a vendored copy of Glencoe Software's jxrlib (itself forked from
Microsoft's JPEG-XR library, BSD-licensed — see the upstream headers).

    upstream:  https://github.com/glencoesoftware/jxrlib
    commit:    19cc1fa19ca6607449d210aea946b3187f808f2c  (master)

Rule: every file of this tree that has an upstream counterpart is upstream's
byte for byte at that commit, with the exceptions listed below. The
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
  definition in `strcodec.c` (lines 672-689):
  ```c
  #if (defined(WIN32) && !defined(UNDER_CE)) || (defined(UNDER_CE) && defined(_ARM_)) || defined(_MSC_VER)
  // WinCE ARM and Desktop x86
  #else
  // other platform
  #ifndef _BIG__ENDIAN_
  U32 _byteswap_ulong(U32 bits);
  #endif // _BIG__ENDIAN_
  #endif
  ```
  The definition exists only when neither `(WIN32 && !UNDER_CE)` nor
  `(UNDER_CE && _ARM_)` nor `_MSC_VER` holds, and only when `_BIG__ENDIAN_` is not
  set: on big-endian `strcodec.c` defines a macro instead (`#define _byteswap_ulong(x) (x)`),
  which a declaration would break, and where the CRT provides the function - Win32
  and MSVC alike - declaring it there would clash with the CRT's own declaration.
- **Amendment (`_MSC_VER` in the first alternative):** MSVC defines `_WIN32`, not
  `WIN32`, so MSVC used to take the "other platform" branch of this header, and of
  `strcodec.c` alike. That was wrong twice over. `strcodec.c` defined a function
  MSVC's CRT already provides, which is error C2169 at `/O2` (`/O2` implies `/Oi`,
  and the compiler then treats the name as an intrinsic that must not be defined);
  and this header declared it with a type that differs from the CRT's
  (`unsigned long` against `U32`, which is `unsigned int`), which MSVC reports as
  C4028 and C4142. Adding `defined(_MSC_VER)` to the first alternative puts MSVC in
  the same branch as a real Win32 build: the CRT provides the function, so the tree
  neither defines it nor declares it. `image/sys/strcodec.c` carries the same
  alternative, so the two files stay mirrors of each other.
- **How the delta stays checkable:** the patch is the only difference between the
  vendored `image/sys/strcodec.h` and upstream's file; deleting the 17 added lines
  reproduces upstream's bytes exactly, line endings (LF) included. Checked against
  upstream at 19cc1fa, byte for byte.
- **If upstream fixes it:** drop those 17 lines and the vendored file is upstream's
  bytes again; the byte accounting below goes back to 44 identical files and 4
  patched files.

### 3. MSVC must not define `_byteswap_ulong`: the release build does not compile

- **File:** `image/sys/strcodec.c`, lines 667-672 of the vendored file — the comment
  and the `#if` that guard the definition of `_byteswap_ulong`.
- **Upstream bug:** the definition sits behind
  `#if (defined(WIN32) && !defined(UNDER_CE)) || (defined(UNDER_CE) && defined(_ARM_))`.
  MSVC defines `_WIN32`, not `WIN32`, so MSVC fell into the `#else` branch and
  defined the function. MSVC's CRT already declares `_byteswap_ulong` (in
  `ucrt/stdlib.h`), and at `/O2` - which implies `/Oi` - the compiler treats the
  name as an intrinsic that must not be defined. The Windows release build of this
  crate therefore does not compile, while the debug build does:
  ```
  vendor/jxrlib/image/sys/strcodec.c(674): error C2169: '_byteswap_ulong': intrinsic function, cannot be defined
  ```
- **Fix:** add `|| defined(_MSC_VER)` to the first alternative, so MSVC takes the
  same branch as a real Win32 build and uses the CRT's function. The definition is
  still there for every compiler that has no CRT function to borrow: the `#else`
  branch, i.e. the ANSI build with gcc or clang, is untouched. The Win32 build
  (`WIN32` defined, as the `.vcxproj` projects define it) is untouched too.
- **How the delta stays checkable:** the patch is the only difference between the
  vendored `image/sys/strcodec.c` and upstream's file; deleting the 5 added comment
  lines and restoring the one changed `#if` line reproduces upstream's bytes
  exactly, line endings (LF) included. Checked against upstream at 19cc1fa, byte
  for byte.

### 4. `<wchar.h>` for the ANSI build: `wcslen` was called without a declaration

- **File:** `image/sys/windowsmediaphoto.h`, the block of system includes at the
  top of the file (after `#include <string.h>`).
- **Upstream bug:** `jxrgluelib/JXRGlueJxr.c` calls `wcslen`, but no header in
  scope declares it. `<string.h>` declares the narrow string functions only; the
  wide-character ones live in `<wchar.h>`. The ANSI build has no `<windows.h>` to
  bring them in, so the call has no declaration - and a call to a function that
  was never declared is an error in C99 and later. The macOS CI job (clang) does
  not compile:
  ```
  vendor/jxrlib/jxrgluelib/JXRGlueJxr.c:66:48: error: call to undeclared library function 'wcslen'; ISO C99 and later do not support implicit function declarations [-Wimplicit-function-declaration]
  ```
  GCC only warns about it.
- **Fix:** include `<wchar.h>` from the header that already carries the ANSI
  build's system includes, under `#ifdef __ANSI__`:
  ```c
  #ifdef __ANSI__
  #include <wchar.h>
  #endif
  ```
  `windowsmediaphoto.h` is the one place where the ANSI build's system headers
  come from: it is included by `image/sys/strcodec.h`, `image/sys/strTransform.h`
  and `jxrgluelib/JXRMeta.h`, and it reaches every one of the 24 translation units
  the crate compiles. `__ANSI__` is the macro that selects the ANSI platform
  everywhere in this tree (`strcodec.h` turns it into `PLATFORM_ANSI`), and it is
  what the crate's `build.rs` defines. The Win32 build does not define it - the
  `.vcxproj` projects define `WIN32` and no `__ANSI__` - and it gets `wcslen`
  from `<windows.h>` instead, so that build is unchanged.
- **How the delta stays checkable:** the patch is the only difference between the
  vendored `image/sys/windowsmediaphoto.h` and upstream's file; deleting the 7
  added lines reproduces upstream's bytes exactly, line endings (LF) included.
  Checked against upstream at 19cc1fa, byte for byte.

### 5. `<stdlib.h>` in `segdec.c`, where the CRT's `_byteswap_ulong` is declared

- **File:** `image/decode/segdec.c`, the include block at the top of the file.
- **Why:** after patch 3, MSVC no longer gets `_byteswap_ulong` from `strcodec.h`;
  it gets it from its own CRT, and that declaration lives in `<stdlib.h>`. On the
  ANSI build `strcodec.h` declares the function and `<stdlib.h>` is not needed for
  it, so the include is the cheapest way to make the dependency explicit at the
  one place that calls the function, instead of leaving it to a three-level
  transitive include (`segdec.c` -> `strcodec.h` -> `windowsmediaphoto.h` ->
  `<stdlib.h>`).
- **Fix:**
  ```c
  #include <stdlib.h>
  ```
  It is a standard header on every platform the tree targets, and the Win32 build
  already has it in scope through the same chain, so nothing changes there.
- **How the delta stays checkable:** the patch is the only difference between the
  vendored `image/decode/segdec.c` and upstream's file; deleting the 3 added lines
  reproduces upstream's bytes exactly, line endings (LF) included. Checked against
  upstream at 19cc1fa, byte for byte.

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

- 43 are byte-identical to upstream at 19cc1fa (sha256 compared file by file against
  the files fetched from raw.githubusercontent.com at that commit);
- 5 (`jxrgluelib/JXRGlueJxr.c`, `image/sys/strcodec.h`, `image/sys/strcodec.c`,
  `image/sys/windowsmediaphoto.h`, `image/decode/segdec.c`) differ from upstream
  only by the patches above;
- 2 (`LICENSE`, `PATCHES.md`) are the vendoring's own files and have no upstream
  counterpart.

Every file of this tree that is not upstream's bytes is listed in this file.
