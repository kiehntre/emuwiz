# Tape Identity historical comparison — 2026-10-18

Compared fresh `origin/main` **29654c7ad540112291ce730b068d84ef8624533d** with the exact
preserved ref `refs/archive/emuwiz/2026-10-16/feature/tape-identity-phase2`,
**c362cc13da4ce2d72eccde63d3dd459b645106c0**. The preserved tip is **not an ancestor** of main: the merge base is
`e8f30bfde7af3f5c8b54f7ecefc8d4ad2bbbfa8e`, its parent. Capability coverage below is
established by direct file/function/test comparison, not a claim that this exact tip merged.
No historical ref or recovery evidence was changed.

The separate integration commit on main is
**10be8f0dbb98c2323c588be39d122c9df4982878** (`integrate bounded ZX tape evidence`).
Its `tape_identity.rs` matches the preserved tip except for the Commodore parser call
adapting to the fixed-prefix/file-length API. Later commits add timing and loader facts.
This is evidence of integrated functionality, not ancestry of the preserved branch tip.

## Decision

The historical parser capabilities are already present or superseded. Do not resurrect or
cherry-pick the branch, add a decoder, or treat cassette blocks as separate verified games.
One later regression merits a small correction: the deep analyzer and Tape Inspector label
**every valid shared TZX/CDT container as ZX Spectrum**, even a pause-only container and even
when the caller supplies CPC context. The historical `observe_tzx_evidence` correctly kept
container structure separate from canonical platform identity. The correction restores that
boundary in descriptive analysis and user-facing wording, without changing DAT verification,
launch identity, parsing, imports or catalogue state.

## Historical capability and present evidence

| Area | Preserved branch | Current implementation and boundary |
| --- | --- | --- |
| ZX TAP | LE block lengths, EOF framing, XOR checksum on every block; bounded standard header metadata | Same `tape_identity::parse_zx_tap` and all ten historical module tests retained unchanged. Names, type, declared length and parameters describe blocks; no title/release verification. |
| Shared `.tap` | Distinguish `C64-TAPE-RAW` from a validated headerless ZX stream; reject ambiguous interpretations | `inspect_tap` retains dispatch; Commodore parser now uses fixed-prefix/file-length adapters. Oric leader/block inspection is additional, not inferred from `.tap`. |
| TZX/CDT | Shared signed/versioned linear parser; metadata and bounded block inventory | Same parser plus timing/length facts, loader classification and fingerprints. `parse_cdt` still delegates to `parse_tzx`. CDT filename or readable metadata does not prove CPC, Spectrum or a game. |
| Commodore TAP/T64 | TAP header and T64 directory facts; neither extracts nor identifies programs | Expanded safe-read adapters and eleven focused tests replace two older Commodore tests. Valid-header/strong-format evidence and non-Commodore refusal remain covered; counts, ranges, overlap, reserved fields and size checks are stronger. |
| Other existing cassette inspection | Not implemented by this branch | Current `tape_analysis` dispatches Oric TAP, raw/gzip UEF and Dragon/CoCo CAS. Existing WAV signal/recovery modules serve Spectrum, Commodore, CPC, BBC, MSX and Atari with explicit GUI platform context. These are existing bounded interpreters, not new work recovered here. |
| PZX/standalone CSW/VOC | Not implemented | Do not claim a complete decoder from an extension or file-picker entry. PZX/CSW candidate filters are not parser support. TZX-embedded CSW/generalized blocks are framed without expansion by this structural inspector. VOC is not added by this task. |

The two renamed historical Commodore tests are covered by
`valid_minimal_tap_header_parses_without_decoding_payload` and
`tap_rejects_wrong_magic_truncation_and_empty_payload`, including the two strong evidence
facts and non-Commodore refusal. Historical TAP/TZX tests still cover malformed/truncated
input, XOR mismatch, absurd block counts, shared TAP dispatch, control-flow records,
non-expanding CSW/generalized framing, unknown framing, metadata limits and conservative CPC
context. No historical missing behavior justifies another decoder.

## Container validation is not release verification

See `crates/archivefs-core/src/tape_identity.rs`, `commodore_tape.rs`, `tape_analysis.rs`,
`content_evidence.rs` and `dat/library_identity_summary.rs`.

- Headerless ZX TAP evidence is **Corroborated** after full block framing and XOR validation.
  A 19-byte header supplies descriptive fields, not proof that a paired data block exists or
  that a complete commercial tape/release was recovered.
- TZX/CDT evidence is **Strong for the container**, not a canonical machine or game. Its
  structural pass does not certify all embedded program checksums: deep analysis reports
  `NotApplicable`, not `Valid`, for the overall TZX checksum. Timing/loader confidence and
  release/DAT confidence are different domains.
- The structural pass caps input at 8 MiB, TAP/TZX blocks at 4,096, TAP block size at 65,535,
  retained metadata at 256 records / 64 KiB, text at 4 KiB and group depth at 64. Arithmetic
  and slicing are checked; jump/loop/call/select records are never executed and compressed
  blocks are not expanded. Other existing audio/UEF modules have their own explicit limits.
- `LibraryDatIdentitySummary` keeps filename-only matches unverified. Exact DAT identity
  follows actual hash evidence and current catalogue evidence, not a parsed header, a title,
  structural validity or a loader fingerprint. This change grants no new verification.
- `game_identity` may call a validated **format fact** Verified (for example a T64 directory).
  That typed fact must not be read as a verified tape program or game release.

## Parts, sides and projections

`media_set/adapters.rs::attach_tape_analysis` attaches entries and loader details as notes.
It assigns no release key, canonical platform, side or ordinal from an embedded program name.
`media_set` already supports explicit tape ordinals, missing tapes, sides and loader/program
relationships. Filename-only grouping remains unverified; TAP/TZX alternatives require
proven equivalence; unrelated titles/platforms are not fuzzy-joined. TZX group nesting and
`logical_segments` are descriptive, not proof of physical sides or a complete multi-part
release.

Blue Collection Discovery uses `ingestion/content_registry`, `discovery` and
`structural_probe`: recognized tape content is distinct from game identity. Unknown-extension
TZX probes retain `platform_hint: None`; their explanation must say **TZX/CDT container**,
not Spectrum detection. Recognized CDT without identity remains an explained candidate.
The file remains the catalogue item; inspecting blocks creates no child game records.

Blue selected-file/Game Details evidence goes through `selected_evidence_page`, keeping tape
analysis separate from structural and DAT identity facts. The native Tape Inspector reuses
`tape_analysis_page`. For a shared TZX/CDT result it must show an unidentified platform and
a named shared container, with an explicit platform-unconfirmed warning; a CPC/Spectrum hint
alone must not silently become detected platform identity. The supported-format description
must likewise acknowledge the shared family.

Launch uses existing resolved catalogue identity and adapter-specific rules, not
`TapeAnalysis.platform`: Fuse supports direct TAP/TZX plus its existing snapshot formats;
Caprice32 admits bounded verified CDT/DSK through its fresh identity/hash and isolation gates.
This change neither selects an emulator nor relaxes launch readiness. No emulator or running
GUI is launched or replaced by this task.

## Focused validation

The regression fixture is a 13-byte valid shared TZX/CDT containing only a pause block. It
has no machine or game evidence. Core analysis, discovery wording, and GUI analysis with
no hint / CPC hint / Spectrum hint must all preserve that distinction. Pre-fix execution
and final bounded regression results are retained in the external audit ledger:
`/home/davedap/emuwiz-audit-ledger/2026-10-18-tape-identity/`.

Validation also exercises retained tape parser tests, tape topology/equivalence/filename
boundaries, filename-only DAT refusal, neutral CDT discovery and native Blue tape routing.
Fixtures use private application/HOME/XDG/TMP roots; no production collection, DAT audit,
5,000-game import, decoder expansion or real emulator session is involved.

Result: the three new regressions failed on the uncorrected main implementation (two core,
one headless GUI). After the correction, **91 focused tests passed** across 18 bounded
filters: retained TAP/TZX and Commodore checks, UEF/Dragon-CoCo checks, CPC waveform checks,
shared analysis, discovery, tape/side relationships, DAT filename refusal, content-evidence
confidence, Tape Inspector rendering and native Blue routing. Targeted `rustfmt --check`,
`git diff --check` and the explicit four-file scope guard passed. No unrelated full test
suite was run. Only core/GUI test binaries were compiled through `scripts/cargo-iso`, with
an isolated target, low-priority two-job build, sccache and the 100 GiB SSD reserve guarded.
