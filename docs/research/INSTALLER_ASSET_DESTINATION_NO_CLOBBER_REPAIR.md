# Installer asset destination no-clobber repair

Review candidate on `fix/installer-asset-noclobber`, based on authoritative main
`8ce27bebb5d3924158300396abd5d0b815253ba5`. No promotion or push is part of this task.

## Base and scope

- Main belongs to `/home/davedap/emuwiz-main-release-fix`.
- Main already contains `fix(installer): bind ownership validation across publication`.
- The tracked main tree was clean before work and remains clean.
- At preflight, local `origin/main` was `7aabe00badb4dda9eca4ab31c59a374ab3f040f6`:
  main was one commit ahead, zero behind. During this task that local remote
  tracking ref advanced to the recorded main SHA; final local parity is exact.
  This task did not fetch, push, or write either ref.
- Untracked research files and caches on main were observed and left untouched.
- Dedicated worktree: `/home/davedap/emuwiz-installer-asset-noclobber`.
- Allowed files: `install.sh`, `tests/test_install.sh`,
  `tests/test_installer_asset_races.py`, `.github/workflows/ci.yml`, and this report.
- No changes to manifest parsing, accepted slot names, fingerprints, old-content
  ownership decisions, uninstall policy, release layout, application code, or Cargo/package dependencies.

## Complete managed publication inventory

This table describes the original publication operations on the authoritative base.
`B` = resolved binary prefix; `D` = XDG data home;
`ID` = `io.github.kiehntre.emuwiz`. Every asset also checks the manifest/bookkeeping
binding before publication. Sources support the existing canonical `bin/`, flat
release, legacy-name, and workspace `target/release/` layouts.

| Slot/site | Source/staged object | Destination | Ownership gate | Original final operation | Can overwrite? | Atomic? | Gate-to-publication race? |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `bin-emuwiz-cli` | `src_cli` copied into `B/.emuwiz-binary.XXXXXX`, executable | `B/emuwiz-cli` | `gate_binary`, old manifest SHA-256 | `mv -f binary_tmp dest` | Yes | Same-filesystem rename | Yes |
| `bin-emuwiz` | `src_gui` copied into the same staging pattern, executable | `B/emuwiz` | `gate_binary`, old manifest SHA-256 | `mv -f binary_tmp dest` | Yes | Same-filesystem rename | Yes |
| `alias-archivefs-cli` | Literal target `emuwiz-cli` | `B/archivefs-cli` | `gate_alias`, recorded/raw fixed target | `ln -sf target dest` | Yes; can also follow a destination directory symlink | Atomic replacement on tested GNU coreutils 9.4; no portable atomicity guarantee | Yes |
| `alias-emuwiz-gui` | Literal target `emuwiz` | `B/emuwiz-gui` | `gate_alias`, recorded/raw fixed target | `ln -sf target dest` | Yes; same directory issue | Same as above | Yes |
| `alias-archivefs-gui` | Literal target `emuwiz` | `B/archivefs-gui` | `gate_alias`, recorded/raw fixed target | `ln -sf target dest` | Yes; same directory issue | Same as above | Yes |
| `desktop` | Rendered desktop template in `D/applications/.ID.XXXXXX.desktop` | `D/applications/ID.desktop` | `gate_content`, old SHA-256 or pre-manifest comparison ignoring `Exec=` | `mv -f desktop_tmp desktop_file` | Yes; a directory collision becomes a move into that directory | Same-filesystem rename | Yes |
| `icon-32` | Approved `emuwiz-logo-32.png` copied into `D/icons/hicolor/32x32/apps/.ID.XXXXXX.png` | `D/icons/hicolor/32x32/apps/ID.png` | `gate_content`, old SHA-256 or exact approved source | `mv -f icon_tmp icon_dest` | Yes; same directory issue | Same-filesystem rename | Yes |
| `icon-64` | Approved `emuwiz-logo-64.png`, same staging pattern under `64x64/apps` | `D/icons/hicolor/64x64/apps/ID.png` | Same icon gate | Same icon move | Yes | Same-filesystem rename | Yes |
| `icon-128` | Approved `emuwiz-logo-128.png`, staging under `128x128/apps` | `D/icons/hicolor/128x128/apps/ID.png` | Same icon gate | Same icon move | Yes | Same-filesystem rename | Yes |
| `icon-256` | Approved `emuwiz-logo-256.png`, staging under `256x256/apps` | `D/icons/hicolor/256x256/apps/ID.png` | Same icon gate | Same icon move | Yes | Same-filesystem rename | Yes |
| `icon-512` | Approved `emuwiz-logo-512.png`, staging under `512x512/apps` | `D/icons/hicolor/512x512/apps/ID.png` | Same icon gate | Same icon move | Yes | Same-filesystem rename | Yes |
| Manifest, absent | Complete `.manifest.XXXXXX` in pinned bookkeeping directory | Pinned fd 8 + `manifest` | Strict parser/preflight, absence and directory binding | `ln -T manifest_tmp /proc/self/fd/8/manifest` | No | Exclusive link creation | Already repaired |
| Manifest, owned | Complete `.manifest.XXXXXX` in pinned bookkeeping directory | Validated open manifest inode, fd 9 | Identity, SHA-256 and directory binding | `cat manifest_tmp > /proc/self/fd/9` | Only writes the held owned inode | No | Replacement pathname cannot redirect the write; late changes abort |

All eleven managed assets were vulnerable. The manifest paths were already
repaired and retain their original policy and publication operations. Hidden
staging and record files are temporary bookkeeping, not additional asset slots.
Initial user config creation/source registration and uninstall removal are
outside managed asset publication; the manifest never claims ownership of user config.

## Deterministic reproduction

Private bundle copies insert mutations immediately before the original final
operation, after the ownership gate and last manifest assertion. Before the fix,
all six initial checks failed: binary regular-file and symlink collisions, alias,
desktop, and icon regular-file collisions were overwritten; a colliding desktop
directory acquired the staged desktop file. The foreign canary remained the
expected assertion target in each fixture. No sleeps or probabilistic timing races
were used. The pre-fix transcript is `/tmp/emuwiz-asset-baseline.log`.

A disposable syscall trace on GNU coreutils 9.4 also verified that `mv -f`
first attempts `renameat2(RENAME_NOREPLACE)` but falls back to overwriting
`renameat` after `EEXIST`. `ln -sf` creates a temporary symlink and uses overwriting
`renameat`. Their atomic replacement behavior does not protect foreign occupants.

## Mechanism and ownership preservation

One shared `publish_asset` function routes all eleven slots through a narrowly
scoped Python standard-library helper embedded in `install.sh`. Embedding it
preserves every existing bundle layout without a companion executable, compiler,
or new package dependency. Python is a new installer runtime requirement.

Before the gate, fd 7 pins the asset parent. Inspection opens the destination
with `O_PATH | O_NOFOLLOW`, snapshots its state, and fingerprints regular content
through that held object. The existing shell ownership gates still decide
absent/owned/foreign using old manifest SHA-256 or existing recognition policy.
Snapshots detect replacement; they never grant ownership.

For an absent destination, publication uses
`linkat(AT_FDCWD, "/proc/self/fd/<source-fd>", 7, basename, AT_SYMLINK_FOLLOW)`.
The staged inode is held open; aliases are staged symlinks held with
`O_PATH | O_NOFOLLOW`. The follow flag applies only to the trusted proc descriptor.
An occupied destination returns `EEXIST`, including a regular file, symlink,
dangling symlink, or directory. It neither follows nor replaces that destination.
Fresh publication is atomic. Direct syscall tracing confirmed this exact operation
for every repaired slot.

For verified regular files, the helper opens the destination using
descriptor-relative `openat` with `O_NOFOLLOW`, rechecks state and the pre-gate
content digest, then opens that held inode for writing through procfs. It rechecks
immediately before writing. Replacement after that check cannot redirect the write
to a foreign file: the output descriptor still points to the verified inode.
Copies stream in bounded chunks, truncate shorter updates correctly, and verify
the final held bytes and destination binding. Unchanged bytes require no write.
This deliberately reuses the manifest repair's non-atomic descriptor-write model.

Existing fixed aliases retain their verified symlink inode and target. A differing
target refuses publication rather than unlinking/replacing a name. Ordinary
reinstall and release upgrades keep the fixed alias targets and work unchanged.

The record returned by the helper describes the bytes/target actually published
through held objects. It never re-fingerprints an arbitrary replacement pathname
to grant new ownership. Manifest binding is asserted immediately before and after
the helper. Failed publication never appends that slot's new record or publishes
a new manifest; an existing manifest remains unchanged.

`--replace-foreign` still explicitly moves a gate-reported foreign object into a
fresh, private backup directory beside it. Asset backups use the pinned parent and
report a durable physical recovery path, never an expired `/proc/self/fd/7` path.
Unreadable foreign files remain eligible for explicit backup. Once backed up, the
destination must be absent: a later collision refuses even with the flag. There
is no added authority to overwrite newly appeared objects.

## Parent binding, runtime and failure limits

Parent checks mirror the bookkeeping model: directory inode/device plus physical
location. Canonical parent components are walked with `O_DIRECTORY | O_NOFOLLOW`;
final name operations use only a basename relative to held fd 7. Pre-existing
symlinked roots remain supported. Retargeted parent symlinks and replaced parents
cannot redirect final publication. A replacement in the final syscall window can
leave a new asset only in the old held directory; the postcheck refuses metadata
publication and preserves occupants of the replacement directory.

Runtime requirements: Linux 2.6.39+ (`O_PATH`, empty-name `readlinkat`), Python 3.6+
with descriptor-relative filesystem support, procfs, and hardlink support on the
installation filesystem. Platform/stdlib capabilities are checked; syscall and
filesystem failures propagate. There is no unsafe `mv -f`, forced-link, rename,
or compatibility fallback for managed asset publication.

Installation remains nontransactional. Owned regular file updates are also
non-atomic: interruption/I/O failure may leave an incomplete owned inode that no
longer matches the old manifest. Earlier publications are not rolled back. Missing
or stale ownership records fail closed on later runs; late partial failures remain
possible. Errors state `destination changed during installation`, refuse further
publication, and explicitly warn that earlier assets may remain unrecorded.

Native EmuWiz processes must be closed before changed-byte upgrades: Linux refuses
writes to running executable inodes with `ETXTBSY`. The installer finds this out
**before publishing anything**. For each EmuWiz-owned native binary whose incoming
bytes differ, it opens the already-verified inode (through the pinned parent
directory, `O_NOFOLLOW`, identity and digest rechecked) for non-truncating write
access and closes it again. That is the very operation the in-place update needs,
the kernel refuses it with `ETXTBSY` exactly while the inode is executing, and it
changes no byte and no inode field. Process names, `pgrep` and PIDs are not used.
Both binaries are checked before the first publication, so a running GUI cannot
leave the CLI published but unrecorded. The installer then stops with "EmuWiz is
currently running and needs to be closed before it can be updated", the manifest
and every asset untouched, and a plain re-run after closing EmuWiz succeeds
without `--replace-foreign`. A binary whose bytes are unchanged is never probed,
so reinstalling the same release while EmuWiz runs still works. A process started
in the short window after the preflight gets the same plain message from the
publication step, but earlier files may already have been published; installation
remains nontransactional. Changed-byte updates to
hardlinked owned files also refuse, preserving neighbouring links. Existing
cooperating installer/uninstaller locking remains unchanged. As with the manifest
repair, this lock does not prevent another same-user process from editing held
inodes or creating hardlinks; it is not a general isolation boundary against that
user. Pathname substitutions cannot make these publication operations overwrite
their replacement objects.

Known limitations deliberately left as they are (recorded, not addressed here):
the installer is not transactional and late-failure messages stay generic;
trailing data after the manifest `end` line; uninstall reads by pathname;
staging temporaries can be left behind by an interrupted run; the Python 3
dependency; filesystems without hardlink support; and installer performance.

References: [Linux link/linkat documentation](https://man7.org/linux/man-pages/man2/link.2.html),
[Linux open/openat documentation](https://man7.org/linux/man-pages/man2/open.2.html),
[Linux rename documentation](https://man7.org/linux/man-pages/man2/rename.2.html).

## Regression coverage and validation

The new suite contains 32 test methods, including four direct helper tests and
52 explicit subcases. It covers all eleven slots, all four asset classes with
regular/symlink/directory collisions, both fresh and previously owned destinations,
and collisions immediately inside the final link/open/write windows. It also
covers fresh install, legitimate reinstall, upgrades of every regular asset class,
shorter binary updates, explicit backups (including unreadable files), refusal of
a post-backup collision, parent replacement, symlinked parent retargeting, a real
concurrent installer held at a deterministic pipe barrier, unmanaged neighbours,
unchanged/absent ownership records on refusal, hardlinks, running native binaries,
wrong owned digests, and missing runtime/syscall support. All mutations affect
private installer copies; production has no test hooks. CI runs the new suite.

Validation uses temporary install roots and
`CARGO_TARGET_DIR=/tmp/emuwiz-asset-cargo-target`:

| Validation | Result |
| --- | --- |
| Full installer suite, including real-terminal rerun | 269 shell checks passed |
| Existing ownership adversarial suite | 13 passed, including repeated execution by the full installer suite |
| New asset race/direct helper suite | 32 methods passed, including 52 explicit subcases |
| Release packaging | 21 passed |
| SBOM generation | 13 passed |
| Packager/SBOM integration | 16 passed |
| Signing | 3 passed with disposable keys and GPG agent runtime access |
| RC acceptance self-test | Passed |
| Release smoke self-test | Passed |
| Upgrade preflight self-test | Passed |
| Shell syntax checks | Passed for installer, installer suite and validation wrappers |
| `cargo check --offline --locked --workspace` | Passed |
| `cargo fmt --all -- --check` | Passed |
| `git diff --check` | Passed |

Totals: 367 reported checks/test methods (269 shell, 13 ownership, 32 new asset,
21 packaging, 13 SBOM, 16 packager/SBOM, 3 signing), excluding duplicate terminal
and repeated ownership runs; all three acceptance/smoke/preflight self-tests passed.
Final installer/race transcripts are `/tmp/emuwiz-asset-installer-final-pass.log`
and `/tmp/emuwiz-asset-races-final-pass.log`. Other validation transcripts use
the `/tmp/emuwiz-asset-` prefix. The working-tree scope and GUI boundary guards
also passed, with no application code changes.

Warnings: cargo emitted five existing GUI warnings (unused `LaunchWarningKind`,
`show_with_playing_library_plan`, `dat_health_label`, `Informational`, and
`portability_label`). No unrelated warning fixes were made. The combined 53-test
release run passed 50 and initially skipped three signing tests because the sandbox
could not start GPG agent; the separate signing run passed all three. The old
180-second terminal watchdog timed out after the shell checks during ownership
tests; it is now 300 seconds, and a complete run passed without changing coverage.

Candidate SHA is supplied with the final review handoff after the local commit.
Main modified: **NO**. Pushed: **NO**. Promoted: **NO**.
