# PCSX2 approved temporary activation

This completes the activation stage of existing PR #167. The initial
[evidence-only probe](PCSX2_TEMP_CHEAT_PROBE_20261026.md) remains historical.
No second PNACH converter or persistent cheat manager was added.

## Supported boundary

Linux Flatpak `net.pcsx2.PCSX2`, user or system installation, exact tested
v2.8.2 deployment
`f8162e3d6b0ce75f54d0e10605d368b3678fda969bb1e806277500156c85fa10`.
The read-only preview queries the live installation; launch pins that commit
and verifies the running sandbox's application, commit and filesystem grants.
Updates, native/AppImage/portable installs, ambiguous settings or folder
routing, unresolved firmware, unsupported media and achievement hardcore
mode are refused with a reason. Ordinary launch remains independently available.

A direct verified PS2 ISO must have a serial and executable CRC. The existing
strict local PNACH parser checks its filename against both and validates every
patch line; the user explicitly selects the whole file for one launch.
Named-section source files unsupported by that parser are refused. CodeBreaker
conversion and permanent install/Undo remain their existing workflows.

## Blue journey

Library → select game → Play → Launch page → the matching PCSX2 profile →
**Temporary PCSX2 cheats — explicit approval**:

1. Choose the local PNACH. Selection and approval initially remain off.
2. Select its validated patch lines, then request the read-only preview.
3. Review serial/CRC, installation profile, PNACH SHA-256, tested deployment
   and preservation paths. Planning installs and activates nothing.
4. Approve one temporary launch, then click its separate launch action.
   Approval is consumed. Changed sources, game, profile, installation or
   deployment require a new preview; there is no ordinary-launch fallback.
5. The canonical runtime creates a private owner-only workspace. PCSX2 runs
   an offscreen configuration check there before the game can boot. Evidence
   and approval are revalidated again between that check and game launch.
6. Blue distinguishes private staging, game handoff, PNACH-load and selected
   group log observations, unexpected exit, preservation violations and
   cleanup. Even observed enablement says **In-game effect NOT CONFIRMED**.
7. Normal sandbox exit triggers canonical cleanup. If EmuWiz is interrupted,
   a running/unknown sandbox retains its workspace. The existing recovery
   control, available in the same panel, allows explicit cleanup only after
   the canonical ownership, process, sandbox and stale-age checks pass.

## Preservation and lifecycle

`launch::cheat_runtime_pcsx2` uses `CheatRuntimePlan`, immutable source hashes,
create-new materialization, owned-file rollback, lifecycle markers, receipts
and existing stale cleanup. A bounded source-backed `VerifiedResource` file
kind admits BIOS bytes without increasing configuration/cheat file limits.

Global and matching per-game INIs are copied; only the private cheat routing,
selection and enablement are overridden. Ordinary patch/widescreen settings
are retained. Per-game BIOS/folder overrides are refused. Real cards, states,
snapshots and videos retain their approved paths; read-only resources and
protected inputs cannot overlap writable grants. Traversal and symlink
ancestors are refused. Mixed-case INI lookups follow PCSX2's case-insensitive
semantics, retaining duplicate/ambiguous-setting refusal.

BIOS and optional console-state companions are verified and copied privately.
The first disposable boot exposed PCSX2 attempting to create a missing `.mec`
next to a read-only original BIOS. Private copying fixes that defect without
changing original firmware or granting writes to it.

Both probe and game use `--nofilesystem=host:reset` plus exact approved grants
and private child HOME/XDG/data-root settings. The configuration probe uses
`--die-with-parent`; the game may survive EmuWiz interruption. Its exact
instance ID comes from the inherited private descriptor. Cleanup waits for
both host exit and absence from Flatpak's process list and runtime metadata;
missing/invalid lifetime evidence conservatively retains the workspace.
No production INI or cheat directory is a write destination.

## Disposable acceptance evidence

Original generated MIPS branch-loop ELF in a generated ISO; verified serial
`SLUS-00001`, executable CRC `76E7FB84`; an authorized existing QA BIOS copy;
synthetic erased cards; a selected PNACH write to unused memory; an existing
original cheat and independently enabled per-game `PreservedBaseline` patch.
No BIOS/game bytes are included in the repository.

Local evidence root: `/tmp/emuwiz-pcsx2-journey-5cdp2zve/evidence`.

| Observation | Result |
| --- | --- |
| Exact private data root and approved folder grants | checked at runtime before boot |
| Selected PNACH loading and `EmuWiz Selected` enablement | observed in PCSX2 logs |
| Existing per-game `PreservedBaseline` patch | observed enabled |
| Emulator overlay | `1 game patch is active. 1 cheat patch is active.` |
| Normal window close | process exit 0; canonical receipt cleanup completed |
| Forced termination of the exact owned sandbox | exit 137 reported; cleanup completed; workspace removed |
| Launch owner interruption | sandbox survived; recovery retained live workspace |
| Explicit recovery after sandbox exit | canonical stale cleanup removed workspace |
| Original INIs, BIOS, synthetic cards, patches and cheat files | before/after SHA-256 maps identical |
| Rebuilt Blue selection → preview → approval → launch → normal close | log-observed load/enablement, consumed approval, normal exit and cleanup shown |
| Blue source changed after preview | refused before staging; no ordinary-launch fallback |
| Gameplay save writes | not exercised by the branch-loop fixture |
| In-game cheat effects | **NOT CONFIRMED**; fixture has no gameplay |

The manual Core acceptance example requires a private fixture HOME/XDG/TMPDIR
and an explicit disposable-activation flag. Its recovery mode advances only
that fixture's observation clock past the existing grace period. Production
Blue uses the real clock and normal grace.

Independent review identified deployment-update and INI-case bypasses. The
owner added commit-pinned execution/live metadata checks and centralized
case-insensitive lookup plus filename validation and focused regressions.
A follow-up review found whitespace section aliases; these are refused before
section storage. Independent safety review passed exact runtime-tested code
`4a1864df407ee6f403c54eb5d5c365da3a1c1ab0`. Final Clippy-only control-flow
simplifications preserve those checks; the final reviewed head is recorded
on PR #167.

Final focused owner gates: 87 runtime tests passed (3 existing ignored),
24 readiness tests, 92 PCSX2 GUI tests, 11 recovery GUI tests and 8 Python
probe tests passed. Final runtime logs include `adapter-activate-final.log`,
`adapter-crash.log`, and `adapter-interrupt-final.log`; Blue evidence includes
`blue-final-report.json`, `blue-final-11.png` (emulator overlay and honest
status) and `blue-final-12.png` (normal exit, removal and consumed approval).
Final test totals, exact reviewed head and CI belong to the PR evidence.
