# RetroArch `.cht` parser hardening

> **Consolidated:** this document describes one input to the integrated cheat model. Where it names a type that was later unified (for example `CheatPackApplicability`, `CheatPackRelationship`, `CheatPackAssociation`, `PokeIdentityState`, or the launch stand-in `CheatApplicabilityState`), see [`CHEAT_CONSOLIDATION.md`](CHEAT_CONSOLIDATION.md) for the current canonical type.

Baseline: `4c980d184584dd5f1a22b5fbbf67e6c58ff204c1`.
Branch: `fix/retroarch-cheat-parser-hardening`.
Worktree: `/home/davedap/emuwiz-retroarch-parser-hardening`.

## Audit before editing

The full document reader was `patch_manager/cht_document.rs`; selection and
installation use it through `cheat_install_plan.rs`. User import also uses it
and retains the original path, filename and SHA-256. The catalogue had a
separate metadata parser in `cheat_catalogue.rs`, and the installed-artifact
inventory had a separate aggregate parser in `retroarch_inventory.rs`.
`cheat_ir.rs` contains the cross-format semantic representation; it does not
parse `.cht` files. GUI selection projects parser warnings through the install
plan's existing `selectable` and `has_blocking_warning` fields. RetroArch launch
command planning consumes inspected paths/core selection rather than parsing
cheat contents; materialization uses catalogue records and the installation
path generates a derivative through the document renderer.

| Input | Previous full-document parser | Previous metadata readers |
| --- | --- | --- |
| Missing `cheats` | Accepted observed entries, no count diagnostic | Accepted observed metadata |
| Invalid/negative count | Warning; count absent; a count-only file failed as not a cheat file | Malformed-line evidence; catalogue could retain valid neighbors |
| Absurd count | Any `u32` accepted; mismatch warning, no count-specific bound; no allocation by declared count | Catalogue ignored numeric count mismatch; inventory marked mismatch incomplete |
| Duplicate index data | Merged fields by index; repeated desc/code/enable warned but remained selectable | Merged metadata; first description, any true enable; no duplicate diagnostics |
| Sparse/out-of-order indices | Sorted by index, sparse warning; indices >= 16,384 rejected | Catalogue accepted high `u32` indices (documented real-corpus regression); inventory rejected indices >= 16,384 |
| Missing description | Nonblocking warning, `None` retained, explicit index-derived display label | Metadata description could be absent |
| Missing code | Blocking warning | Metadata-only entries retained; catalogue excluded malformed debris without any code |
| Empty code | Blocking warning, except quoted whitespace was nonempty | Code presence used only by catalogue's debris exclusion |
| Malformed enable | Warned and defaulted false, still selectable | Any true value enabled metadata; other values silently ignored |
| Malformed numeric fields | Preserved as extras, without scalar validation | Ignored |
| Duplicate keys | First desc/code/enable retained, all duplicates nonblocking; extras/globals silently dropped; last valid count won | Duplicate descriptions/enable/count could disagree without diagnostics |
| Unknown fields | Bounded extras/globals preserved; no unsupported-field warning | Ignored |
| Long lines/values | No standalone file/line bound; decoded and allocated complete value before truncating to 4 KiB; entry warnings unbounded | Files bounded (catalogue 8 MiB, inventory 2 MiB), but no line/value bounds in parsers |
| Invalid UTF-8 | Rejected; UTF-8 BOM accepted, UTF-16 BOM rejected | Inventory rejected; catalogue explicitly decoded legacy Windows-1252 and recorded an encoding diagnostic |
| Replacement-decoded text | Replacement character accepted by text API | Catalogue uses explicit byte mapping, not replacement decoding |
| Truncation | Missing fields blocked, but missing closing quote was silently accepted; escape sequences changed values | No quote/truncation validation |
| Mixed valid/invalid entries | Already isolated missing/empty code and control/interior-quote problems | Catalogue retained metadata under its compatibility rules; inventory marked aggregate incomplete |

## Implemented policy

All three readers now use the same bounded document parser. Metadata reports
still omit code bodies and retain incomplete entry metadata for review; a
catalogue record is not proof that every entry can be installed. Full selection
continues to reject blocking warnings. Catalogue legacy decoding remains
explicit and informational; installation/import do not guess an encoding.

Bounds are 8 MiB per parser input, 8 KiB per line, 4 KiB per value/code,
256 `+`-separated code components, 16,384 distinct entries, 32 entry warnings,
256 document warnings, 32 extra fields per entry, 64 globals, and 32 preserved
leading comments. Existing inventory/import/installer read limits still apply.
No allocation or iteration is driven by the declared count. At the entry cap,
new indices are omitted with a diagnostic; fields of retained entries still
receive validation. Warning overflow retains an explicit marker; entry warning
or extra-field overflow blocks installation.

Identical decoded duplicate values produce a nonblocking duplicate warning.
Conflicting entry values retain the first value and both values in bounded
review evidence, preserve the later line, and block that entry. Count/global
duplicates retain first evidence and produce document diagnostics; globals are
not forwarded by the selected-entry renderer.

Sparse and out-of-order indices are retained and sorted numerically. Any `u32`
index is supported; the bound applies to distinct entries, not index magnitude,
matching the catalogue's proven high-index compatibility. Index overflow and
invalid key shapes are reported. Only generated derivatives renumber indices.

Missing, malformed, oversized and inconsistent counts are typed evidence.
Both observed-entry-count differences and an observed index outside the
header's range produce count-mismatch warnings. Valid neighboring entries
remain selectable. No entries or field values are synthesized from a header.
Missing description remains `None` with a warning and an explicit display
label; missing/empty/whitespace-only code is unusable.

Unclosed quotes, unsafe control/replacement characters, malformed booleans,
invalid known numeric fields, empty code components, oversized fields/lines,
and conflicting duplicates block the affected entry. Known numeric fields
validate unsigned 32-bit decimal or explicit hexadecimal syntax; unknown
well-formed fields remain preserved and nonblocking. Backslashes remain literal
instead of inventing an escape language. Oversized evidence retains only an
explicitly marked prefix for review.

Parsing is read-only. Import/catalogue regressions assert unchanged source
bytes and original-byte SHA-256 provenance. Existing GUI projections require
no changes, and no launch architecture is redesigned.

## Validation scope and deferrals

Synthetic tests cover all twenty requested cases, scalar validation,
count/global/extra-field duplicates, normalized duplicate indices, late unsafe
fields after entry/warning limits, multibyte bounds, every truncation point of
a malformed fixture, and 512 deterministic byte mutations. No testing dependency
was added. Affected metadata/import regressions verify diagnostic propagation
and provenance; existing selection/install tests exercise the installation gate.

Only focused cheat/parser and affected module tests, formatting checks and
whitespace checks are permitted in this lane. Full workspace/GUI-v2 tests,
release builds and live GUI smoke belong to later batch integration. Core-
specific cheat semantics, new GUI status wording, broader encoding support,
network sources and unrelated cheat architecture are deferred.

## Validation results

All commands used `--offline` and the existing local Cargo target cache.
Each test command was `cargo test -p archivefs-core --lib
patch_manager::<module> --offline`:

| Module filter | Passed | Failed |
| --- | ---: | ---: |
| `cht_document` | 46 | 0 |
| `cheat_catalogue` | 58 | 0 |
| `retroarch_inventory` | 9 | 0 |
| `user_cheat_import` | 15 | 0 |
| `cheat_install_plan` | 38 | 0 |
| `retroarch_materialization` | 7 | 0 |
| **Total** | **173** | **0** |

`cargo fmt --all -- --check`, `git diff --check` and the task scope/boundary
postcheck passed. No separate `cargo check` was required: the affected core
crate and exhaustive enum matches compiled in the focused library test build;
GUI projection types and GUI code were unchanged. No full workspace suite,
full GUI-v2 suite, release build or live GUI smoke was run.
