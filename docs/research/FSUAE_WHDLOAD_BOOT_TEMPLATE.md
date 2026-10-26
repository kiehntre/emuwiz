# Explicit offline FS-UAE WHDLoad boot templates

Job #38 adds a bounded Linux adapter for ordinary WHDLoad execution through the
existing Gold planner and Blue launch state. Real gameplay has not been proven:
no real game, boot template or firmware has yet been authorized for this job.
Synthetic tests cannot establish game compatibility or input response.

The adapter supports one explicitly selected LHA/LZH installed-game package,
exactly one valid slave, WHDLoad major version 18, and an explicitly acknowledged
A1200 configuration with 2 MiB Chip RAM and 8 MiB Fast RAM. It does not run an
installer, infer identity from a filename, mount an LHA as a hard disk, integrate
FS-UAE Launcher, or download dependencies. Other layouts and requirements refuse.
Existing trainer activation remains disabled. Guest icon tooltypes and user
WHDLoad preferences are never forwarded.

## Manual setup and authorization

Supply a prepared template containing authorized guest executables/support files
and firmware. EmuWiz does not establish licensing from file presence or hashes:
the authorization fields below are explicit user attestations. Record acquisition
and permission details in `provenance`; do not mark files authorized without that
basis. An installer archive alone does not supply the installed game's data.

Place a manifest named `fs-uae.whdload.json` beside the selected profile's
`fs-uae.conf`. This is an explicit per-profile, single-game opt-in; no manifest
is created automatically. Multiple candidate FS-UAE profiles remain ambiguous.
Use absolute paths with real directory ancestors, not symlinked aliases.

```json
{
  "schema": 1,
  "machine": "A1200",
  "chip_memory_kib": 2048,
  "fast_memory_kib": 8192,
  "authorized_game": true,
  "authorized_template": true,
  "authorized_firmware": true,
  "disposable_saves": true,
  "provenance": "REPLACE with the authorization and source of each supplied input",
  "game_key": "REPLACE with the exact verified slave SHA-256 (64 lowercase hex characters)",
  "package_sha256": "REPLACE with the complete package SHA-256",
  "slave_member": "GameDirectory/Game.slave",
  "template_root": "/absolute/path/to/authorized-template",
  "whdload_version": 18,
  "files": [
    { "path": "Firmware/boot.rom", "sha256": "REPLACE" },
    { "path": "Boot/C/WHDLoad", "sha256": "REPLACE" },
    { "path": "Boot/C/SetPatch", "sha256": "REPLACE" },
    { "path": "Boot/C/uae-configuration", "sha256": "REPLACE" }
  ]
}
```

This is a schema illustration, not a usable approval. All hashes must be 64
lowercase hexadecimal characters. Every template file must be explicitly listed;
unlisted files, missing files and unknown JSON fields refuse. Additional support
files may be declared only below `Boot/C`, `Boot/Devs`, `Boot/Libs`, `Boot/L` or
`Boot/Fonts`. The machine boot ROM must be 512 KiB. Required guest tools must have
an Amiga HUNK header, and WHDLoad must declare an `$VER: WHDLoad 18.` marker.
These checks and user-declared hashes do not prove that arbitrary bytes form a
working boot environment; ordinary gameplay remains the final compatibility gate.

Do not include `Boot/S`, existing startup scripts or `WHDLoad.prefs`. The adapter
creates its own startup sequence, changes to the exact selected slave's directory,
runs SetPatch and WHDLoad without CUSTOM arguments, then requests emulator exit
through the supplied guest `uae-configuration` tool. The template must supply the
AmigaDOS/library dependencies those particular tools require.

A slave requiring kickemu additionally needs its exact ROM and matching `.RTB`
under `Boot/Devs/Kickstarts`, explicitly listed with their hashes. The selected
slave's ROM name, size and ANSI CRC16 are checked. Multi-ROM tables and opaque
memory-configuration tables remain unsupported by the existing slave parser.
Encoded ROMs and unsupported machine/memory/version requirements are not silently
converted or supplemented. No files are copied from the real profile's Kickstart
folder merely because they exist there.

## Readiness, launch and preservation

Readiness validates the manifest, explicit authorization, structure, limits and
file metadata. Game and dependency payload hashes are checked at Play, off the
render thread, against the manifest; readiness is not a claim of successful boot.
Only native Linux ELF executables are supported; shell-script wrappers refuse.
The executable is checked again at spawn and pinned through exec while its original
argv[0] is preserved for FS-UAE resource lookup. Portable resource layouts remain
unvalidated. The installed distro binary passes a version-only descriptor probe;
this does not prove resource loading or gameplay.

The executable, profile and manifest are bound to the approval, and fresh evidence
must match before preparation and launch. The exact full slave path, parsed
requirements and slave SHA-256 must agree with the original inspected evidence.

Only a private copy of the package is given to the existing LHA decoder. Files
are created without clobbering through descriptor-relative operations. Links,
special files, traversal, shell metacharacters, `.uaem` sidecars, case collisions
(including implied directories) and file/directory prefix collisions refuse.
Limits are 512 members, 32 MiB per member/template file, and 128 MiB aggregate
for each of game payload and template; packages are capped at 128 MiB. At least
512 MiB staging capacity must be available. Unsupported Amiga metadata/layouts
must be supplied in an explicitly supported form rather than silently rewritten.

A generated config mounts only staged Boot and Game directories, with the boot
volume read-only and the game copy writable. HOME/XDG/state/cache/temp are private.
The child receives a clean environment with only explicit graphics/audio session
variables and a fixed PATH; parent FS-UAE, loader and plugin overrides are omitted.
The original profile is inspected for binding but its mounts, scripts and state
are not copied into the runnable configuration. Its unused Kickstart path need
not be configured: only the explicitly authorized template ROM supplies firmware
authority for this isolated FS-UAE session. A present live-profile ROM never
substitutes for a missing, wrong-sized or stale template ROM.

**This is a disposable session. New saves are discarded after emulator exit.**
The manifest must acknowledge this, and Blue displays it before Play. Original
packages, template files, firmware, saves and emulator settings are never writable
mounts or write destinations. There is no automatic export, import or replacement
of real saves. The process watcher owns the workspace until child exit, even if
the GUI selection changes or its process handle is dropped. A failed spawn cleans
only its private workspace. Interrupted host processes may leave private temporary
folders; no broad automatic cleanup of unidentified directories is added.

## Verification and remaining gameplay gate

Focused tests use generated synthetic LHA/HUNK/ROM bytes and inert local test
processes. They cover terminal-state retries separately from Game Details typed
dispatch, dependency and authorization refusals, exact slave selection, stale
sources, unsafe paths and lifecycle isolation. They do not run those synthetic
bytes in an emulator or represent them as a legally distributable real game.

Before claiming gameplay PASS, obtain explicit authorization for the candidate
package, template and every firmware/support input. In an isolated HOME/XDG and
disposable graphical session, use normal Blue Game Details → Play, reach gameplay,
demonstrate keyboard/controller response, exit, and repeat without restarting
Blue. Capture missing-firmware, stale-package and unsupported-input refusals and
before/after hashes of originals. Saved trainer handoff can be verified
independently of this gameplay gate; do not claim trainer effects or gameplay
activation from a generated command alone.

## Saved trainer handoff

Blue reads the existing per-game `whdload-trainer-options/<identity digest>.options`
file using the trainer editor's storage contract. The verified game key must equal
both the slave SHA-256 and the authorized manifest game key. CUSTOM1–CUSTOM5 values
are checked against the selected slave's own `ws_config`: duplicate slots,
undeclared bits, invalid choices/booleans, unsupported declarations, non-decimal or
overflowing values, symlinks, non-regular and oversized saved files are refused.
Zero values add no arguments. Other preserved tooltypes never enter the command.

The LHA adapter re-reads saved bytes before and after staging, and requires the
extracted slave hash and parsed structure to equal the selected evidence. Only
then are nonzero CUSTOM values appended to the generated guest command:

```text
C:WHDLoad "Game.slave" PRELOAD CUSTOM1=3 CUSTOM2=2
```

No `x_whdload_args`, CUSTOM emulator switches, source-package edits, live profile
writes, or template-gate exceptions are used. Installed-folder and Amiberry trainer
activation remain unsupported. Blue's WHDLoad details show the selected values,
package/slave, guest invocation and isolated machine/mount policy. Malformed or
stale choices block launch rather than silently launching without them.

Deterministic tests verify this handoff with synthetic packages and inert tools.
No authorized manifest/boot environment was available for this integration's
runtime demonstration: **gameplay and actual trainer activation NOT CONFIRMED**.
An installed emulator or existing firmware files alone do not establish permission
or satisfy the explicit boot-template gate.
