# PlayStation 4 identity — Phase 1 (bounded layout + PARAM.SFO)

EmuWiz can read trustworthy, bounded PlayStation 4 game identity from an
**extracted** PS4 game directory. The evidence target is a single metadata
file: `sce_sys/param.sfo`. The result is a typed evidence report, not a
guessed game name.

This is identity/backend work only. There is **no** shadPS4 launch, **no**
PKG installation or extraction, **no** decryption, **no** executable
execution, **no** GUI change, and **no** filesystem mutation.

## Supported input shape

One shape only:

* a directory (the selected library entry itself, or a shadPS4-style
  extracted/installed game root) that contains, as **regular files with no
  symlink anywhere on the path**:
  * `sce_sys/` — a real directory, and
  * `sce_sys/param.sfo` — a real file.

A loose `param.sfo` selected on its own is **not** a supported input in
Phase 1 (`inspect_game_identity` dispatches PS4 folder inspection only for a
directory). A `.pkg` file assigned to the PS4 platform is reported
`Unsupported` with a diagnostic that says to extract the game — the `.pkg`
extension alone never becomes PS4 identity.

Not supported, by design: encrypted retail `.pkg` parsing, PKG/disc
decryption, any PS4 filesystem decryption, recursive crawling of the game
directory.

## PARAM.SFO parsing

Parsed **only** through the pre-existing shared bounded parser
`crate::param_sfo::parse_param_sfo` — the same one PSP and PS3 identity use.
No second SFO parser was added. The reader in
`crate::game_identity::inspect_ps4_directory_identity`:

* rejects a `param.sfo` whose file size exceeds `MAX_SFO_BYTES`
  (1 MiB) before reading it → `Resource limit reached`;
* reads the whole (bounded) file with `std::fs::read` and hands the bytes
  to `parse_param_sfo`, which itself enforces:
  * total size ≤ `MAX_SFO_BYTES` (1 MiB),
  * index-table entry count ≤ `MAX_SFO_ENTRIES` (4096),
  * each value length ≤ `MAX_SFO_VALUE_BYTES` (64 KiB),
  * every key/value offset and length inside the file (checked
    arithmetic, `Option`-returning helpers), fail-closed to `None` on any
    violation — never a partial or guessed result;
* on `None` from the parser → `Invalid`.

### Fields read

| Key | Use | Confidence |
|-----|-----|------------|
| `TITLE_ID`   | verified PS4 title ID (see validation below) | `Verified` / `StructuredMetadata` |
| `CONTENT_ID` | verified PS4 Content ID, a **separate** fact | `Verified` / `StructuredMetadata` |
| `TITLE`      | descriptive display name | report warning only — never identity |
| `APP_VER`    | descriptive app version | report warning only |
| `VERSION`    | descriptive master version | report warning only |
| `CATEGORY`   | descriptive application category (`gd`, `gp`, …) | report warning only |

Only `TITLE_ID` and `CONTENT_ID` become verified evidence. `TITLE`,
`APP_VER`, `VERSION`, and `CATEGORY` are retained as
`PS4 PARAM.SFO <KEY>: <value>` strings in `report.warnings`, so a display
can surface them without any of them being treated as exact game identity.
No field is assumed present.

## PS4 title-ID validation

`crate::ps4_layout_evidence::normalize_ps4_title_id` trims, upper-cases, and
requires the **`CUSA` application-ID family**: the literal four letters
`CUSA` followed by exactly five digits (`CUSA00001`). This is deliberately
stricter than the loose "four letters + five digits" shape:

* PS3 (`BLUS30000`, `NPEB00342`) — rejected;
* PS Vita region codes (`PCSE00001`, `PCSB00001`) — rejected;
* wrong length / non-digit tail — rejected.

A missing `TITLE_ID` → `Missing`. A present but non-`CUSA` `TITLE_ID` →
`Invalid` with a diagnostic noting a PS3/Vita SFO is not PS4.

## Content-ID behaviour

`CONTENT_ID`, when present, is parsed by
`crate::ps4_layout_evidence::parse_ps4_content_id` by bounded, shape-only
grammar matching of the shared Sony Content ID form
`<label(2)><dist(4)>-<title-id(9)>_<type(2)>-<content-label>`
(e.g. `UP0001-CUSA00001_00-BLOODBORNE000000`):

* exactly three `-`-delimited segments;
* a 6-character alphanumeric `<label><dist>` prefix;
* a middle segment split once on `_` into a `CUSA`-family title ID and a
  2-digit content type;
* a 1–16 character alphanumeric content label;
* total length ≤ 64 bytes (a real Content ID is 36–37).

Anything else → `None` (not exposed). The parsed Content ID is exposed as
its own `Ps4ContentId` fact / `IdentityKind::Ps4ContentId` verified value —
**never merged into** the title ID.

### Disagreement handling

`title_id_agreement(param_sfo_title_id, content_id)` compares the PARAM.SFO
`TITLE_ID` against the title-ID component embedded in `CONTENT_ID`:

* `NotComparable` — no usable `CONTENT_ID`;
* `Agrees` — identical;
* `Disagrees { … }` — different.

On `Disagrees`, `inspect_ps4_directory_identity` emits a single
`IdentityKind::Ps4TitleId` fact with status **`Ambiguous`** and a
diagnostic naming both values, and resolves **nothing** — it never silently
picks one. Focused tests cover all three outcomes
(`ps4_folder_surfaces_content_id_title_disagreement_as_ambiguous`,
`agreement_reports_match_mismatch_and_not_comparable`).

## Confidence semantics

* **Strong / structured** — a valid `sce_sys/param.sfo` layout, a bounded
  PARAM.SFO that parses within every limit, and a valid `CUSA`-family
  `TITLE_ID`: `IdentityStatus::Verified`,
  `IdentityConfidence::StructuredMetadata`. Same for a well-formed
  `CONTENT_ID`.
* **Descriptive** — `TITLE`, `APP_VER`, `VERSION`, `CATEGORY`: retained as
  report warnings, never verified identity.
* **Exact bytes** — unchanged: hash / DAT evidence, gathered separately,
  remains the only authority on which canonical release a directory holds.
  A validated PS4 title ID makes no claim about region or exact release.

## Platform-evidence semantics

Platform determination for PS4 relies on **two** PS4-specific signals
together, never on the mere existence of a `param.sfo`:

1. the `sce_sys/param.sfo` relative layout (PS3 uses `PS3_GAME/PARAM.SFO`,
   PSP uses `PSP_GAME/PARAM.SFO`); **and**
2. a `CUSA`-family `TITLE_ID` (PS Vita, which also stores `param.sfo` under
   `sce_sys/`, uses `PCSx` region codes and is excluded).

`IdentityPlatform::PlayStation4` is a new identity-inspection platform and
`IdentityPlatform::from_catalogue` now maps `"ps4"` / `"playstation 4"` /
`"playstation4"` / `"sony playstation 4"` to it. A PS3 `PS3_GAME` folder
asked as PS4 has no `sce_sys/param.sfo` and is refused
(`ps3_folder_is_never_seen_as_ps4`).

`crate::ps4_layout_evidence::observe_ps4_evidence` produces neutral
`ContentEvidence` (a PS4-exclusive `BootStructure` marker plus the title /
content IDs as `ProductCode` facts) **only** when both signals are present,
for a future scanner / `platform_evidence_fusion` consumer. Wiring that
into the live scanner and the fusion-rule / coverage tables is a
deliberate follow-up and is **not** included in Phase 1.

## `.pkg` behaviour (unchanged, and deliberately weak)

A retail PS4 `.pkg` is encrypted; a trustworthy `TITLE_ID` cannot be read
without decryption support this build does not have. The existing PS3
`.pkg` header path (`inspect_direct_pkg`) is untouched and continues to
emit a PS3 fact from a PS3-shaped Content ID. A `.pkg` **assigned to the
PS4 platform** reports `IdentityKind::Ps4TitleId` with status
`Unsupported` and a diagnostic to extract the game — it never manufactures
a PS4 identity from the extension
(`ps4_pkg_extension_alone_does_not_produce_ps4_identity`).

## Resource bounds (exact)

| Bound | Value | Where |
|-------|-------|-------|
| `param.sfo` file size (pre-read) | ≤ `MAX_SFO_BYTES` = 1 MiB | `inspect_ps4_directory_identity` |
| PARAM.SFO total size | ≤ `MAX_SFO_BYTES` = 1 MiB | `param_sfo::parse_param_sfo` |
| PARAM.SFO index entries | ≤ `MAX_SFO_ENTRIES` = 4096 | `param_sfo::parse_param_sfo` |
| PARAM.SFO one value length | ≤ `MAX_SFO_VALUE_BYTES` = 64 KiB | `param_sfo::parse_param_sfo` |
| Content ID length | ≤ 64 bytes before grammar match | `parse_ps4_content_id` |
| Paths inspected | exactly `sce_sys/` and `sce_sys/param.sfo` | `ps4_directory_paths_are_regular` |
| Directory recursion | none | — |
| Symlinks | rejected on every path component from `root` down, and on `sce_sys/` and `param.sfo` themselves | `ps4_directory_paths_are_regular` |

`report.metadata_paths_inspected` is `1` and `report.bytes_read` is the
`param.sfo` size (`< MAX_SFO_BYTES`) for a successful inspection — verified
by `ps4_inspection_reads_only_sce_sys_param_sfo_no_recursive_scan`.

## No launch

Phase 1 adds no shadPS4 command planning, no Gamer View Play path, no
emulator arguments, no installation, no PKG mounting, no executable
selection, and no `VerifiedIdentityFact` variant. `IdentityKind::Ps4TitleId`
/ `Ps4ContentId` are **not** wired into `launch::evidence_bridge`; a
verified PS4 report yields `CanonicalIdentityStatus::Unknown` from that
bridge and zero launch facts (`ps4_folder_resolves_cusa_title_and_content_id`).
PS4 launch is a later phase.

## Real-media status

The configured PS4 game root `/mnt/games/roms/ps4` (the shadPS4
`install_dirs` path) does **not exist** on this machine, and no
`sce_sys/param.sfo` is present anywhere under `/mnt/games` / `/mnt/games-nvme`.
Phase 1 was therefore validated entirely with synthetic, deterministic
PARAM.SFO fixtures. **Real PS4 media acceptance is deferred** until an
extracted PS4 game directory is available.
