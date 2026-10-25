# CUE INDEX 00 / PREGAP preservation audit

Base: `origin/main` 56e0699b. Builds on `CUE_INDEX00_PREGAP_SEMANTICS.md` (timeline model).

## Findings
1. `fingerprint_cue_bin` hashes only the data track's INDEX 01 program range. A single-track
   CUE/BIN with a **stored INDEX 00 pregap** therefore matched a program-only CHD, and
   `optical_equivalent` offered the CUE and BIN for quarantine, losing the stored pregap sectors.
   Synthetic PREGAP, POSTGAP, INDEX 02+ and an unaccounted INDEX 01 offset were equally invisible.
   `chd_redump::compare_cue_bin_chd_logical` documented that pregap layouts were refused; they were not.
2. The converter gate (`optical_preservation::source_layout`) already refused every such layout, but
   only with a generic "outside the verified layout" message, so the preview could not say what
   would be lost. The timeline's `ConversionBlocker` reasons were never wired in.
3. The directive-prefix checks in `cue_bin.rs` sliced `line[..N]` and panicked when a multi-byte
   character straddled the boundary; the gate had been shielding the parser from such text.

## Rules
- INDEX 00 = sectors physically in the image. Never equal to PREGAP (not in the image).
- INDEX 00 + PREGAP on one track is ambiguous: review required, never summed or guessed.
- A match from the program-area fingerprint is **not** proof of disc equivalence. Equivalence
  consumers use `fingerprint_cue_bin_exact`, which refuses stored/synthetic pregap, POSTGAP,
  INDEX 02+, a non-zero INDEX 01 file offset and any audio track.
- Conversion is admitted only for one BINARY file, one MODE1/2048 track, INDEX 01 at 0. Every
  refusal now carries `[layout preservation: ...]` naming the exact loss; the GUI shows it as
  "Not preserved exactly". No path claims lossless for anything else.

## Still unsupported
Audio/mixed-mode, multi-track, multi-FILE, MODE1/2352 and MODE2 CHD conversion; materialising a
synthetic pregap; INDEX 00 + PREGAP; any CUE-to-CHD equivalence involving pregap/audio.
