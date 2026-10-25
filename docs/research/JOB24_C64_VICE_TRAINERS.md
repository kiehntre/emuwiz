# Job #24, Stage 2: explicit C64 startup POKEs

Base: `3da6c88e7b09ce5bade0e8311e628b3d8f9eacdb`, the normal merge of
[Stage 1 PR #139](https://github.com/kiehntre/emuwiz/pull/139). Its expected
head `cb9492cc692c4a938677fdf0a64aef1c3fca4d39` had independent review and
passing full CI. Current-main compatibility was checked without rerunning
those tests: the merged main tree equals the clean merge-tree result
`52452584601753719a9d88833a532771f3aa5cff` for the same head/base.
Origin/main and ancestry were verified after merge. Stage 1 Cargo ownership
was released; its worktree remains because retirement reports HOLD.

Stage 2 is a separate candidate. It must not be merged without another owner
approval. [Issue #111](https://github.com/kiehntre/emuwiz/issues/111) remains
open for that decision.

Current-main integration also preserves the independently added snapshot
evidence module alongside the C64 module; the only content conflict was their
adjacent module declarations. No snapshot functionality was rewritten.

## Bounded support and existing implementations

The Stage 1 cartridge and executable contract is unchanged: exact native
x64sc 3.7.1 binary digest, current unambiguous C64 DAT identity, and a direct
standard 8 KiB ROM CRT (v1.0, hardware 0, one ROM CHIP, bank 0 at `$8000`).
The same predicate is shared through the existing Gold CRT inspector;
there is no new CRT parser, cartridge conversion or broader media support.

A same-name `.pok`/`.POK` file is parsed by Gold's existing
`parse_simple_pokes(Commodore64, ...)`. The whole file is **one bundled entry**,
including all its POKE lines/colon-separated operations. It is not split into
partial effects or assigned a new trainer grammar. Comments supported by that
parser remain comments; BASIC programs, monitor commands and opaque scripts
are rejected. ZX `.pok` semantics are retained for ZX sources and are never
reinterpreted as C64 writes.

Gold's existing `.pok.target` format and Select/Preview/Confirm pin creation
bind the exact trainer SHA-256 to the current DAT-verified game hash. A pin
asserts an association, not gameplay compatibility. Filename-only, stale,
ambiguous, mismatched or unsupported entries cannot launch. Existing pins are
never overwritten, and no new pin Undo/backup capability is claimed.

The initial selected entry must contain only unguarded, non-overlapping byte
writes to unbanked RAM at `$C000–$CFFF`. Gold's existing classic VICE projection
must approve every operation first. Restoration fields, guards, other banks,
wider values, I/O, colour RAM and ROM-mapped targets remain refused; nothing is
silently discarded or truncated. This extra range restriction avoids machine
control memory and unsupported startup assumptions.

Writes run **once at monitor startup**, using the existing `radix H / > / x`
encoding. They are not a freeze; a game's own initialization or later execution
can overwrite them. Existing emulator settings and system-ROM handling remain
the emulator's. The native configuration must already work as in Stage 1.

## Explicit approval and revalidation

The Blue chooser defaults OFF, starts with no selected entry and no approval,
and shows source, operation count, generated write preview and compatibility
refusals. Loading or rendering evidence does not create a script or start VICE.
The user must opt in, select the whole compatible entry, approve its one-shot
writes and click the distinct trainer-launch button. Ordinary Play stays
trainer-free and cancels unused trainer approval. Approval is consumed after a
trainer launch; game/backend/evidence changes clear it too.
Leaving the supported VICE lane, including ambiguous multiple-profile plans,
revokes retained approval. Gold's existing multiple-profile refusal is preserved;
this stage does not introduce a profile chooser or weaken launch warnings.
With multiple installed profiles and an existing remembered choice, trainer
controls belong only to that unique remembered request. Other Play cards cannot
reset its selection; changing the remembered choice revokes the old approval.

The selected action is sealed to the complete Stage 1 request and evidence:
game and executable paths/identities, profile, hashes and catalogue snapshot.
Before preparation and again at the last handoff, the same Stage 1 preflight
revalidates game/DAT identity and the exact executable. Gold then rereads the
trainer, pin and game, checks their file observations/hashes and the exact
selected entry, and regenerates typed supported operations. Empty, duplicate,
unknown or stale selections refuse without launching.

## One launch path and shared preservation runtime

Gold's existing monitor encoder is shared by `ViceMonitorScript::create` and
the receipt runtime. It regenerates bytes from typed commands and ignores any
projection text; imported trainer text cannot become arbitrary monitor commands.
The command file is private disposable material under the existing approved
cheat workspace root. Script preflight uses a bounded nonblocking regular-file
read and checks the generated digest.

The existing `execute_cheat_runtime` creates the workspace, records its lease
and receipt, verifies sources, and hands off to Gold's existing
`preflight_vice_launch_with_cheats` and `spawn_vice`. The resulting command must
equal the approved Stage 1 command plus its generated `-moncommands` argument.
There is no second launcher, live-memory client or trainer interpreter.

The canonical receipt records materialisation separately from emulator handoff,
PID/exit and cleanup. Monitor consumption and in-game activation remain unknown.
Failed preparation/handoff is shown as a refusal, not a running or exited game.
The same receipt refusal/error also reaches shared Activity/attempt reporting;
retaining a cleanup receipt is not treated as a successful launch.
Game, trainer, pin and executable are read-only sources with preservation checks.
The same runtime performs post-exit cleanup and detects stale, active, ambiguous
and foreign workspaces; recovery never sweeps a live/uncertain workspace.
Source ROMs and VICE configuration are not rewritten; `+saveres` is retained.

## Validation gates

Focused tests cover the existing parsers/pins/projections, strict selection and
approval, stale source/game/DAT/backend evidence, exact argv/script, ordinary
Play isolation, receipts, source preservation and disposable workspace cleanup
and recovery. Private HOME/XDG/TMP/application roots isolate test data.

```text
scripts/cargo-iso check -p archivefs-gui --lib
scripts/cargo-iso test -p archivefs-core --lib -- classic_trainer_evidence:: classic_poke_family:: launch::vice_ disk_format::tests::crt
scripts/cargo-iso test -p archivefs-gui --lib -- launch_readiness_page::vice gui_v2::classic_trainers gui_v2::vice_evidence
```

The installed native VICE was also probed with a synthetic standard cartridge
under private HOME/XDG/Xvfb and copies of the installed system ROMs. With the
generated startup write it displayed green; without the write it displayed red.
An additional probe checked both `$C000 = $2A` and `$CFFF = $FF` and displayed
green using `radix H`, the two generated byte writes, and `x`.
The cartridge, private configuration and ROM-copy before/after hashes matched.
This proves the backend behavior in the synthetic fixture, not activation in
an arbitrary game. The harness terminated only its own process after capture.

Independent review and normal CI must pass on the final candidate. They do not
authorize merging this Stage 2 PR; that requires a new owner approval.
