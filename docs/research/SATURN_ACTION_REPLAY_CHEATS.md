# Sega Saturn Action Replay cheat family

## Scope and research basis

This feature is a local/imported, read-only decoder and preview model. It does
not patch Saturn media and does not install a cheat database.

The public Saturn format reference used for the direct-write grammar is the
[Retro Multi Tools Saturn code reference](https://svengdk.github.io/RetroMultiTools/reference/cheat-codes.html),
cross-checked against the [GameHacking Saturn code-type reference](https://wiki.gamehacking.org/Code_Types_%28Sega_Saturn%29).
Both describe `TTAAAAAA VVVV`, with `16` as a 16-bit write and `36` as an
8-bit write. The latter also documents conditional and master-code forms.
Examples of real imported text use the same shape and identify the master code
as an ordered prerequisite, but are not treated as a redistribution source.

[Mednafen's cheat documentation](https://sources.debian.org/src/mednafen/1.32.1%2Bdfsg-3/Documentation/cheats.txt)
documents a different native cheat-file grammar. [Kronos' libretro
documentation](https://docs.libretro.com/library/kronos/) says RetroArch cheats
are supported but native cheats are not. EmuWiz therefore does not claim a
Saturn-specific Mednafen/Kronos writer in this feature.

## Supported opcodes

`16AAAAAA VVVV` is normalized to a 16-bit direct write and
`36AAAAAA 00VV` (or any four-digit value whose low byte is used) is normalized
to an 8-bit direct write. Original spelling is retained and normalization is
deterministic.

`D0`-`DF` conditional forms, `F6`/`B6` master/enable forms, one-time writes,
loops, copies, offsets, and unknown types remain typed opaque entries. They are
shown in preview with a refusal reason and are never flattened into an
unconditional operation. This preserves semantics until a native Saturn
implementation can be independently verified.

## Identity and discs

`SaturnCheatIdentityEvidence` accepts exact disc/data-track identity, product
number, revision, region, disc number/count, and title evidence. Callers must
gate any future runtime action in this order: exact hash, verified product and
revision, region/version evidence, then title-only warning. Title or filename
alone is never sufficient.

Disc number and total-disc fields are explicit so Disc 1 and Disc 2 are not
silently conflated. Existing Saturn manifests and System ID facts remain the
source of optical identity; this module only consumes their evidence and does
not change disc parsing or rebuild code.

## Emulator projection and legal boundary

No Apply path is added. Kronos documents RetroArch-level cheat support, but
does not establish a Saturn-native file format that EmuWiz can safely write.
Mednafen's general cheat grammar is not assumed to be interchangeable with
Action Replay. Codes are local/imported only; no commercial Action Replay
database is downloaded, bundled, scraped, or uploaded.

## Safety invariants

Parsing is ASCII-only, bounded to 16 KiB and 256 codes, rejects malformed
lengths and non-hex input, preserves opaque codes, and never reads or mutates a
disc. Direct operations feed the existing cheat IR; compatibility analysis can
therefore report conflicts without a Saturn-specific second conflict engine.
