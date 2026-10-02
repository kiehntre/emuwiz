# CUE INDEX 00 / PREGAP timeline (backend)

Base `3b166b523907514120639e51106118b414e3e0f6`, branch `feature/cue-index00-pregap-correctness`.
Builds on `CUE_BIN_PREGAP_CORRECTNESS.md`. Reference semantics (read, not copied):
libyal libodraw "CUE sheet format", GNU ccd2cue "CUE sheet format", libcdio pregap notes
(links in the earlier note). No network was used for this change.

## What was reused
`ingestion::cue_bin` stays the one parser. It gained per-track `file_ordinal`, `file_type` and
`extra_indexes` (INDEX 02-99), duplicate/out-of-range INDEX numbers are now errors, and MSF fields
must be plain digits (`+1:00:00` and blanks were accepted before). `optical_preservation` and
`repair::optical_conversion` are **not edited** (dirty in `emuwiz-cue-chd-current-main`); their
existing single-track MODE1/2048 gate already refuses INDEX 00, PREGAP and extra indexes.

## New: `ingestion/cue_timeline.rs`
`build_timeline(&CueLayout)` (lengths from metadata only, no file reads) returns per track:

- `pregap`: `None | SourceBacked(SourceRange{start_frame, frames, byte_offset, byte_len})` (INDEX 00)
  `| Synthetic{frames}` (PREGAP). Never merged or converted into each other.
- `program`: source-backed INDEX 01 -> boundary, where the boundary is the next track's INDEX 00 if
  present, else its INDEX 01, else the file end.
- `extra_indexes`, `synthetic_postgap_frames`, and `cue_frame_start / index01 / end`.
  Positions are cue-timeline frames, **not LBAs**. `lba_index01` is relative to track 1's INDEX 01.
- Multi-file: INDEX values are per FILE; each file starts where the previous one (plus any synthetic
  frames) ended. Synthetic PREGAP/POSTGAP inside a shared file push later frames out; file offsets
  never move.
- First track: INDEX 01 is not assumed to be disc zero. Audio track 1 with INDEX 00 longer than
  the 150-frame lead-in reports `hidden_track_one_frames`.

Refusals are typed (`Invalid`, `Unsupported`, `ReviewRequired`): INDEX 00 + PREGAP on one track
(ReviewRequired, never summed), INDEX 00 not before INDEX 01, decreasing or overlapping indexes,
out-of-order INDEX 02+, index beyond the file, tracks without program frames, non-consecutive track
numbers, non-BINARY FILE types, mixed sector sizes in one file, empty/partial-sector (truncated) or
missing sources, and any u64 overflow (checked arithmetic throughout, no floats). Unsupported modes
(for example MODE2/2336) stay refused by the parser.

`LayoutEvidence` records source-backed pregap tracks, synthetic pregap/postgap tracks, hidden track
one, multi-file, and `redump_equivalence_verified: false` so a later DAT comparison can tell the
structures apart. No Redump equivalence is claimed.

## Rewrite safety
`render_canonical_cue` normalises text (spacing, order, drops REM/TITLE) but keeps INDEX 00 as INDEX 00
and PREGAP as PREGAP. `semantic_signature` proves it: parse -> render -> parse yields an identical
signature, rendering is idempotent, and the INDEX 00 and PREGAP signatures differ. No other code
rewrites CUE text today.

## Conversion
`chd_conversion_preservation(&DiscTimeline)` allows only the layout the converter is proven to keep
(one BINARY file, one MODE1/2048 track, INDEX 01 at frame 0, no gaps/extra indexes) and otherwise
returns specific `ConversionBlocker`s (source-backed pregap, synthetic pregap/postgap, hidden track
one, extra indexes, multi-file, multi-track, audio, unverified mode) with readable reasons.
Wiring these reasons into the converter preview needs `optical_conversion.rs` /
`optical_preservation.rs`; that is deferred because of the collision above.

## Still unsupported
Multi-track, audio, MODE1/2352 and MODE2 CHD conversion; materialising synthetic pregap;
INDEX 00 + PREGAP; non-BINARY FILE types; MODE2/2336.
