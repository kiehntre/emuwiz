# PS4 platform evidence fusion — Phase 2

Phase 1 (`docs/PS4_IDENTITY_PHASE1.md`) added bounded extracted-folder PS4
identity and the pure observer `ps4_layout_evidence::observe_ps4_evidence`, and
deferred feeding it into platform-evidence fusion. This phase wires that bridge.
It is backend-only: no GUI, launch, shadPS4, PKG, decryption or migration work.

## What is now wired

```
extracted PS4 folder
  -> ps4_layout_evidence::observe_ps4_directory        (bounded, symlink-safe read)
  -> ps4_layout_evidence::observe_ps4_evidence         (neutral ContentEvidence)
  -> platform_evidence_fusion::fuse_platform_evidence  (rule ps4_extracted_layout_cusa)
  -> Resolved: PS4
```

* `observe_ps4_directory` / `observe_ps4_directory_evidence` are the new
  collector entry points. The layout check is the same
  `game_identity::ps4_directory_paths_are_regular` used by the identity report
  (now `pub(crate)`), so the report and the fusion evidence cannot disagree about
  what counts as the layout.
* One fusion rule, `ps4_extracted_layout_cusa` (platform `PS4`), with a single
  Strong `BootStructure` leg: the marker `sce_sys/param.sfo+CUSA`.
* `content_evidence_scope::SCOPE_CATALOG` marks that marker
  `PlatformSpecific("PS4")`.
* `coverage_inventory::COVERAGE` gains a PS4 row, `SyntheticValidated`, noting
  extracted-folder evidence only.
* There was no directory evidence collector for any platform before this (the PS3
  folder observer is likewise not called outside tests). Callers feed the
  collector's output to fusion / `inspect_identity` exactly as for other
  platforms; no scanner change was needed or made.

## Evidence required

All of: an absolute root with no symlink on any path component; `sce_sys/` a real
directory; `sce_sys/param.sfo` a real file at most `MAX_SFO_BYTES`; the shared
bounded `param_sfo::parse_param_sfo` succeeds; `TITLE_ID` is `CUSA` + 5 digits;
and `TITLE_ID` does not disagree with the title component of `CONTENT_ID`.

## Never PS4 on its own

A `.pkg`, `eboot.bin`, a bare `sce_sys` directory, a loose `param.sfo`, a name
containing `CUSA`, a folder under `ps4`, a PS3 `PS3_GAME/PARAM.SFO`, or a Vita
`sce_sys/param.sfo` with a `PCSx` id. `ProductCode` facts never resolve a
platform by themselves.

## Disagreement

If `TITLE_ID` and the `CONTENT_ID` title component differ, `observe_ps4_evidence`
now emits nothing (fail closed), so fusion has no PS4 leg and cannot resolve PS4
from either side. The identity report keeps reporting the same case `Ambiguous`.
(Before this phase the observer would still have emitted the Strong marker in
that case; that was unreachable from any live path.) PS4 evidence that coexists
with strong evidence for a different platform produces the normal `Conflict`
outcome; there is no priority override.

## What it does not prove

A validated CUSA id proves the PS4 application/platform only. It does not prove
the exact dump or release, region, revision, hashes, ownership, launchability or
emulator compatibility. Those stay with DAT/hash evidence and the launch layer.

## Resource bounds (inherited)

PARAM.SFO read is capped at `MAX_SFO_BYTES` (read through `take(limit + 1)` so a
file that grew after the layout check is still rejected); the parser enforces its
entry-count and value-length bounds. Exactly two paths are touched (`sce_sys`,
`sce_sys/param.sfo`); there is no directory crawl.

## Unsupported

Retail `.pkg` inspection, disc-image identity, PKG extraction/installation,
decryption, executable inspection, any filesystem mutation.

## Future boundary

shadPS4 launch planning is a separate, later task. A PS4 platform result here
makes no launch claim.
