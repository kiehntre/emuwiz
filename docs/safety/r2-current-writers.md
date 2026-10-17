# Current-version writers and the R2 boundary

This assessment starts at accepted `426bf2d9a56a1c5ea47e1a424a008aa5b4ce8a6f`.
Freshly fetched `origin/main` on 2026-10-15 was
`ce8815d63c6d2797fdc480c9d1f7d840035ca6f1`. The owned integration worktree is
`/home/davedap/emuwiz-r2-real-integration-20261015`.

**Whole-application shared admission is not implemented. Production R2 remains
blocked and disabled.** (Default-OFF; the release contract is
`r2-practical-safety-contract.md`.) Replacing executables alone does not change this fact.
The meaningful production changes in this delta correct failed-Undo reporting
and receipt retention. No new mutation path, authority or recovery format was
introduced. The accepted regular-file enrolled integration is preserved.

## What updating can and cannot solve

A single supported build is useful operationally. It reduces incompatible
callers only if every relevant entry point actually joins the same authority,
and previous executables/processes cease writing. Today ordinary current GUI,
CLI and worker calls do not join the test-private R2 authority. Several separate
engines and pre/post-publication writers bypass the rename executor entirely.

A future cooperative contract can cover all explicitly enrolled writers without
a privileged service. That contract does not cover any unregistered writer. It must
not promise that an unregistered writer cannot change the protected files.
Replacing installations, a version check in the new binary, an advisory lock,
or a PID scan cannot enforce that stronger promise. Retained source worktrees
and build caches can also supply runnable older copies.

## Exact source inventory

[r2-writer-inventory.txt](r2-writer-inventory.txt) preserves the independent
baseline source inventory: 21 writer/resource families, entry points, mutation
helpers, GUI/CLI/background callers, exported API-only writers, and explicit
refusal paths. It distinguishes game payloads, generated resources, saves and
cards, emulator installations, and metadata. Its line numbers refer to the
baseline above; its search index and audit limitations are explicit.

The main groups are:

| Writer family | Current route | R2 admission |
| --- | --- | --- |
| DAT rename; repair; organisation; playing libraries; publisher profiles | `rename_apply`, with wrapper directory/sidecar work | Only supported test-enrolled regular RenameMove calls |
| Exact resume; quarantine | Separate loops calling `apply_mutation_in` | No ordinary application admission |
| MAME internal repair / merged reconstruction | Staging plus main executor | No ordinary application admission |
| MAME normalizer CLI | Separate ZIP/directory rebuild and reversal | None |
| CHD / Wii U | Converters, staging, publication through repair; separate cleanup | None across complete operation |
| PSP ISO to CSO | Direct output writer | None |
| 7z/RAR extraction | Exported staging/publish API | None; no normal app caller found |
| Public ZIP conversion | Explicit refusal stubs | Remains refused |
| Standalone/optical patches and patch recovery | Existing separate receipts and publication | None |
| Shared mods/textures/cheats; older cheat installer | Two other transaction families | No common R2 admission |
| Library views; BIOS links; ES-DE metadata | Separate publishers/reversals | None |
| Launch workspaces; saves/cards; emulator installs | Separate resource lifecycles | None; old unguarded card restore remains refused |

This is a source audit, not a dynamic proof that every possible write is known.
Spawned external tools/emulators do not inherit R2 participation by being
launched by EmuWiz. CLI `rom-organise` only changes configuration; the CLI MAME
normalizer really mutates despite its stale read-only description.

## Implemented corrections

* Generic linked-library, RomM and RetroDECK Undo inspect the nested
  `RollbackResult`. Partial/failed attempts now set the error consumed by GUI
  background completion, so it cannot emit its Undo success banner.
* Failed RetroDECK publication compensation preserves an unresolved transaction
  and reports nested rollback failure. A fully successful compensating Undo
  preserves the established no-applied-receipt behavior.
* RomM/RetroDECK receipts show a recovery warning for incomplete transaction
  states instead of a green library-created label, including after state reload.
  The existing `is_fully_rolled_back()` consistency check also prevents a green
  Undo receipt when a RolledBack summary contradicts unresolved entries.
* PPSSPP texture Undo checks its structured result, retains the installed receipt
  on failure, clears stale rollback preview and shows an error. Only success
  clears the receipt.

Synthetic regressions reproduced the concealed failed Undo, lost texture
receipt and misleading rendered receipts before correction. Existing normal
GUI flows remain part of validation. These changes improve truthful outcomes;
they do not remove the underlying legacy wrong-object race.

## Actual race and interruption evidence

The new Rust test
`current_legacy_process_bypasses_enrolled_ownership_during_undo` starts a second
process running this candidate's real `executor::apply_transaction`. At the
existing deterministic boundary after Undo validation, that process performs
two ordinary legacy transactions: moving the original destination aside and
moving a replacement into its name. R2 Undo then moves the replacement and
reports `RollbackFailed` after the identity check. The original inode is retained
at the other path; compact state agrees with the result; release refuses and
claims remain. **Current unenrolled EmuWiz interference is NOT PREVENTED.**

This process is a real Rust executor caller, not an actual GUI or CLI frontend.
Do not relabel it as a GUI-versus-CLI acceptance test.

| Requested scenario | Evidence / limitation |
| --- | --- |
| Two enrolled current processes | Existing admission tests refuse conflicts before mutation: different IDs, hardlink aliases, both slots and held-operation/checkpoint boundaries |
| Current enrolled versus current legacy process | New counterexample above: NOT PREVENTED, detected after wrong mutation |
| Current GUI versus current CLI | Source paths audited; GUI and real CLI Undo caller regressions run separately. No deterministic cross-frontend race test executed; NOT ACCEPTED as protected |
| Current versus an exact old executable | No exact old-binary race run. Old/current legacy routes remain outside admission; old installs are not cleared by this audit |
| Ordinary outside writer | Existing subprocess name-swap and retained-writable-FD cases explicitly NOT PREVENTED; latter changes the protected inode itself |
| Crash during Apply/Undo and fresh recovery | Existing enrolled suite kills at mutation/checkpoint/acknowledgment/approval/release boundaries and verifies fresh-process signed recovery and exact identities |
| Corrupt/rolled-back authority | Corruption/partial rewind refuses; restoring the complete authority snapshot can replay a consumed genuine grant, explicitly NOT PREVENTED |

The passing counterexample assertions prove a limitation, not a protection.
Process interruption and injected I/O errors do not establish hardware power-loss
durability. See [the accepted integration contract](r2-in-tree-enrolled-recovery.md).

## Why there is no safe executor-only integration

The existing authority can protect its supported enrolled operations. Moving
its test-private configuration into normal bootstrap needs a trusted common
selection mechanism across GUI, CLI, profiles and restart. Ownership also needs
the complete resource set and lifecycle, including directory creation, staging,
backups, sidecars, nested compensation and cleanup. Other writer families have
other recovery semantics; the compact Bridge deliberately refuses those kinds.

Installing only an executor lock would leave direct loops and other engines
unprotected. Claiming that it covers the application would be false. Creating a
second legacy authority or broadening compact semantics implicitly would violate
the accepted contract. No such partial interception or silent fallback was added.
The whole-current-version rollout remains an implementation requirement; it is
not completed or declared inherently impossible by this audit.

## Which guarantee needs protected custody

A cooperative-only guarantee does not inherently need a privileged Linux
service. **Excluding old, unregistered or external writers from protected
objects and names needs an enforceable filesystem boundary.** The present
same-UID application has none. The two counterexamples show both kinds of bypass:
name substitution after validation and writing through an already-open handle.

On the proposed Linux deployment, a separately administered service identity
could own the selected inodes and both name parents, with all ordinary clients
lacking write/rename/unlink access, including aliases. Setup requires operator
permission to provision that identity and custody. A daemon need not run as root
for every operation; the boundary is enforced ownership and access, not the name
or privilege level of the process. Permissions changed while old writable handles
survive are insufficient. An approved offline handover/reboot or migration to
new private inodes must eliminate those handles and mappings. No such host
changes were performed here.

Independent signing-key custody, genuine approval, authority anti-rollback,
crash-safe custody handover, supported filesystem/device power-loss behavior,
capacity/retry bounds and deployment recovery still need implementation and
validation for an adversarial deployment. Updating the app cannot supply those
guarantees. Under the owner's practical contract
(`r2-practical-safety-contract.md`) they are documented limitations, not release
prerequisites. *Updated 2026-10-17:* the gate is no longer `#[cfg(test)]`. The
explicit `emuwiz-r2` operator executable is the only caller, and each
enrollment/execution needs `--enable-contract-1-i`. The GUI and `emuwiz-cli`
expose only compact readers/refusals.

## Safe upgrade and retirement procedure (not executed)

1. Inventory GUI/CLI launchers, desktop Exec/TryExec targets, PATH wrappers,
   portable/AppImage copies, user and system packages, scheduled/background
   jobs, other user sessions and mounted/removable copies. Record exact binary
   hashes and source/build provenance; a shared version string is insufficient.
   Include aliases and caller-specific authority/config roots.
2. Stop new operations through the supported UI, let safe work finish and retain
   every unresolved journal/claim. Review and recover outstanding work through
   its supported version. Back up config, catalogue, journals and authority
   consistently. Never restore authority records independently or clear claims
   merely because a process disappeared.
3. Reuse `scripts/qa/upgrade_preflight.py` and its documented read-only report for
   config/schema/root/journal compatibility. Its SAFE verdict is not writer
   custody or process quiescence. Resolve mixed ArchiveFS/EmuWiz roots and unknown
   journals before migration; do not let older builds reopen migrated authority.
4. After explicit operator approval, replace launchers/packages with one verified
   build and retire old runnable installations and scheduled invocations. Stop
   old processes under a reviewed handover. Replacing an on-disk executable does
   not terminate an old process or revoke an open writable descriptor. Retained
   worktrees, history, audit evidence and support caches are not deleted by this
   procedure; keep their binaries away from production access through approved
   custody if old-code exclusion is required.
5. Validate real GUI/CLI/background participation, supported/refused operation
   routes, restart/restore policy and custody using disposable data before any
   production enablement. Obtain approval for the actual bounded contract. A
   version check is diagnostic only and must not be the exclusion mechanism.

Read-only host observations for this task are in the audit ledger's
`installations.json`. Bounded standard install/AppImage/desktop roots found two
helper scripts, no positively identified installed EmuWiz GUI/CLI there. A
separate shallow cache search found 15 runnable GUI/CLI artifacts with unverified
build provenance, all preserved and unexecuted. This is not an exhaustive host
inventory: process visibility is the execution sandbox's namespace, not all host
sessions; renamed/deeper/offline copies remain possible. No host-wide clearance
or effective old-version retirement is claimed.

## Validation and handoff

Exact commands, exit statuses, counts, red regressions and independent review
are retained under
`/home/davedap/emuwiz-audit-ledger/2026-10-15-r2-single-version/`.
All Cargo commands use this worktree's `scripts/cargo-iso --low-debug` target
and the unchanged 8 GiB floor. Existing caches, worktrees and evidence are kept.
No push, production activation or real-library mutation is authorized by this
candidate. Whole-app participation and the GUI/CLI/old-binary race acceptance
cells above remain outstanding even if the focused fixes and existing R2 tests
pass.
