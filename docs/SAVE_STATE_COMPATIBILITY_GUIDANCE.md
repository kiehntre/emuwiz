# Save-state compatibility guidance

Recovered from historical `feature/savestate-restore-readiness-wiring`
(`6cc0563877b4d0178d4483522ed8e5eb07b96109`), starting at fetched main
`e66477861027eafa8c1d64891c84f1feede589ed`.

## Existing and recovered functionality

- Already implemented: provider-neutral Saves & States inventory, game identity
  evidence, emulator/profile discovery and resolver policy, Launch planning and
  readiness, native Blue Saves & States, and PS2 Save Vault.
- Missing and recovered: focused compatibility assessment and resume-readiness
  projection, plus explanations in Blue inventory and Game Details.
- Superseded historical assumptions: matching metadata is not a proven format
  contract; unknown validation is not corruption; version differences alone do
  not prove incompatibility. No guessed portability or probably-compatible status.
- Evidence limitation: current inventory's emulator/version/profile fields describe
  the installation owning the observed location. They do not prove capture-time
  emulator/build/core identity. No current inventory adapter supplies a verified
  state-format/version contract. Consequently **no production emulator/state
  combination is advertised as Supported**.

## Evidence rules

`SaveStateCompatibility` is Supported, Incompatible or Unknown.

Supported requires fresh state bytes tied to explicit capture provenance,
unambiguous corroborating game evidence, matching known emulator/version/profile
(and RetroArch core/version), and an adapter's explicit format/version contract
bound to exactly that capture and selected target. The synthetic tests supply
contracts for same-version DuckStation and same-core/build RetroArch fixtures;
these are test data, not claims about real emulator formats. No new parser,
provenance persistence, state storage or restoration framework is introduced.

Known different emulator/core or an explicitly unsupported format is Incompatible
when the capture evidence is fresh. Missing versions/cores/profiles, changed
versions/profiles, missing/unreadable/unchecked files, conflicting/filename-only
identity, stale provenance, absent selection or absent format contracts remain
Unknown, with reasons. No cross-version support contract is currently implemented.
Platform, title text and revision alone never establish snapshot game ownership.

The API takes the current hash from a caller that actually checked the file.
Rendering performs no file reads: production guidance explicitly reports freshness
as unchecked, never reuses the inventory hash as a fresh hash, and remains Unknown.
Refresh save locations to renew the inventory; it still cannot establish missing
capture provenance or format support. A previously assessed result is never cached
as permission to load a state. Any changed selection must be reassessed.

`SaveStateResumeVerdict` retains Launch blockers and always reports
`resume_available = false`. Even Supported means manual review only; no actual
restoration has been verified. Ordinary SRAM/save-file and memory-card portability
remain governed by their existing workflows.

## Blue journey

Open **Saves & States → Inspect save locations → Savestates**. Each state shows
Unknown and explains capture/freshness evidence missing from the inventory. Game
Details includes **Save-state compatibility guidance**, showing only snapshots
with an unambiguous title/content identity matching the game. Unassigned or
ambiguous states remain in the full inventory. The existing game-scoped save view
shows the same guidance. Current selection comes from the same Launch plan and
candidate selection policy as Game Details; no emulator is selected by this feature.
Launch target metadata lacks verified versions for most adapters, so these stay unknown. DuckStation uses only version metadata belonging to the shared resolver’s authoritative executable; missing/refused/ambiguous resolver results never become a selected target.

No assessment starts an emulator, loads a state, changes saves/configuration,
changes Launch readiness, or changes PS2 Save Vault transactions/Undo. R2 and
shared recovery journals are untouched. Historical branches and unique commits
must be retained until the existing retirement process authorizes removal.

## Validation

Synthetic fixtures cover same-version emulator and same-core contracts, different
emulator/core, missing/different versions, changed selection, ambiguous and
filename-only identity, stale provenance, missing files, unsupported formats,
malformed hashes, retained Launch blockers and no save/configuration writes.

The independent reviewer executed all 15 final core module tests through an
isolated Rust harness. Focused existing core suites passed: inventory 6, resolver
31 (one existing ignored test), Launch planner 68, Launch integration 66 and
readiness 7. Focused GUI suites passed: guidance 3, Saves & States 6, save inventory
4, Save Vault 6, readiness summary 40, shared resolver 37 and Game Details 7.
GUI and focused existing core tests used private HOME/XDG/TMP roots and synthetic
fixtures. No real emulator was executed. Scope, GUI boundary, targeted formatting
and whitespace checks passed. Existing unrelated compiler warnings were retained.

Independent review found and corrected two issues before promotion: snapshot
identity rules initially affected ordinary-save association, and whitespace-only
RetroArch core names could pass the synthetic contract. Regressions preserve
ordinary save matching while keeping ambiguous snapshots unassigned, and reject
blank core metadata. Filename confidence and malformed digest checks were also
added. The review/validation evidence is retained outside the source tree.
