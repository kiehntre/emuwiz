# VICE C64 cheats on the launch command (`-moncommands`)

Status: **L5 (launch wiring) plus L6-for-the-command-file. L7 is not proven.**

## What this adds

- `build_vice_command_plan_with_cheats` / `preflight_vice_launch_with_cheats` add `-moncommands <file>` right after `+saveres`, leaving the content as the last argument. With no cheats the command is byte-identical to the baseline. Cheats for another game, or a non-absolute file, are refused. The canonical preflight (content and executable identity, profile rediscovery, identity match) still runs first and its refusals still apply; canonical spawn is unchanged.
- `ViceMonitorScript::create` writes the command file from a launch-reviewed `ViceCheatProjection`: only `ReadyForLaunchReview` projections, and only normal RAM and colour RAM writes (I/O, ROM-mapped and cartridge-banked targets are never applied automatically). The text is regenerated from the typed commands, not copied. It lives in a private (0700) temporary directory as a 0600 file, is hashed when written and re-hashed at preflight, and a symlinked, missing or changed file is refused.
- `ViceCheatSession` holds the process and the file together. The file is removed after VICE exits; a session dropped while VICE is still running leaves the file for the OS temp cleaner rather than removing it under the emulator.
- No VICE resource file, media image or global configuration is touched; the media stays read-only.

## Real VICE evidence (2026-10-04, `x64sc` from the Debian/Ubuntu package, disposable HOME, Xvfb)

- `x64sc +saveres -moncommands cmds.txt -limitcycles N` with a file of `radix H`, `> C000 AA`, `> C001 BB`, `save "<dump>" 0 c000 c001`, `x` logged "Opening monitor command playback file" and wrote a dump containing `AA BB` at `$C000`. **VICE read and executed the generated command file.** (L6 for the file.)
- On this host VICE could not load the supplied KERNAL ROM (`Couldn't load kernal ROM`), so no program could be autostarted. A real game, and therefore the gameplay effect, was **not** observed.

## Known limit: startup order

VICE's documentation says monitor commands run before the kernel reset sequence. An autostart or cartridge launch resets the machine, so a RAM write made at that moment may be overwritten by program startup. This candidate does not hide that: `ViceCheatIssue::LaunchTimingWarning` remains on every projection, and no persistent in-game trainer claim is made. Proving an effect needs a working ROM set and a game, or a checkpoint-based script (a monitor `command` attached to a checkpoint after startup), neither of which was available here.

## Capability level

L1–L3 existed (projection). This adds L5 and shows L6 for the command file. L7: no.
