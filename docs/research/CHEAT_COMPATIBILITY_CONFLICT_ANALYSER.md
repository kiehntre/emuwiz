# Cheat Compatibility and Conflict Analyser

## Existing support audited

EmuWiz already has a format-neutral `CheatOperation` IR for direct 8-, 16-,
and 32-bit writes, Dolphin on-frame writes, and explicitly opaque raw
operations. Existing reconciliation groups exact semantic/raw duplicates and
same-title/different-code entries. Existing preview/apply paths retain staged
files, use shared destination checks and transactions, and already fail closed
for unsupported or unverified sources. None of those layers compared memory
ranges or master/revision requirements, so this feature adds a separate pure
analysis seam rather than duplicating parsing or installation.

## Normalized memory model

`patch_manager::cheat_compatibility` accepts `CheatCompatibilityEntry` values
with provider/source provenance, platform, revision evidence, master-code
requirements and normalized operations. `CheatCompatibilityOperation` supports
direct writes, arithmetic mutations, pointer writes and opaque unknown
operations. `CheatMemoryRange` carries address, width, value, condition,
continuous-write state and pointer uncertainty. Existing `CheatOperation`
values can be converted with `CheatCompatibilityOperation::from_ir` without
inventing semantics.

## Conflict rules

The analyser reports deterministic pairwise evidence for same-address same-
value writes, same-address different-value writes, partial range overlap,
conditional overlap, duplicate normalized cheats, master-code mismatch,
platform mismatch, revision mismatch, order sensitivity, always-on/toggle
interaction, unknown operations and possible pointer aliases. Every finding
retains both IDs/providers and address evidence where it exists.

Same-value duplicates are informational. Different values, unsafe partial
overlap, platform mismatch, incompatible master codes and strong revision
mismatch are blocking. Unknown and pointer behaviour produces warnings or an
unsupported readiness state; it is never silently treated as compatible.

## Revision and master-code policy

Revision evidence has explicit strength: exact hash, verified identity,
provider-declared revision, title-only, and unknown. Strong conflicting
evidence blocks a stack. Title-only evidence remains ambiguous/weak and never
overrides exact identity. Master/enabler codes are retained as explicit
requirements; different required codes are a blocking mismatch and EmuWiz
does not choose one automatically.

## Readiness and apply boundary

The report exposes `Compatible`, `CompatibleWithWarnings`, `OrderSensitive`,
`Conflicting`, `WrongRevision`, `Ambiguous`, and `Unsupported` states plus
`can_apply()`. The analyser is read-only and does not reorder, write files,
activate emulator codes, or change game state. Existing emulator-specific
transactions remain authoritative; callers must pass an approved report before
enabling a future stack apply. No installer was rewritten in this task.

## GUI-v2 boundary

GUI-v2 already routes cheat preview/apply through the established native
Cheats & Mods workflow and shared preview transaction. The new core report is
available at that seam for the next UI wiring increment; existing GUI controls
remain unchanged in this task so no adapter silently receives an incomplete
analysis. Advanced address/provider details belong in an expandable preview
panel, while the primary surface should use plain language such as “Both
cheats write different values to the same address.”

## Tests

Synthetic vectors cover same/different values, partial overlap, duplicate
selection, master/platform mismatch, unknown and pointer uncertainty, strong
revision mismatch, deterministic readiness and apply blocking. No external
provider is contacted and no source or emulator state is mutated.

## Known limitations

The existing IR does not yet carry full conditional, pointer-resolution,
increment/decrement, or platform-specific master-code semantics for every
adapter. Such operations remain explicit uncertainty. GUI display and
adapter-specific apply gating should be wired only where the corresponding
parser already supplies an approved normalized operation set; raw CHT text must
not be guessed into addresses.
