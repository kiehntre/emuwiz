# PSP Game Slimmer Phase 2: bounded analysis and evidence

This phase adds a read-only analyzer for PSP ISO9660 images. It does not
remove files, rebuild an ISO, create a slim copy, modify the source ISO, or
expose a deletion action.

## Structures inspected

The analyzer reuses EmuWiz's bounded ISO9660 reader and records:

- the ISO9660 volume and bounded directory tree;
- `PSP_GAME/`, `PSP_GAME/SYSDIR/`, `PSP_GAME/SYSDIR/EBOOT.BIN`, and
  `UMD_DATA.BIN` layout evidence;
- bounded `PSP_GAME/PARAM.SFO` identity fields when the file parses;
- total size, source SHA-256, entry count, read budget, and largest files;
- path/name signals for language, movie/video, audio/voice, update, install,
  manual, demo, padding/dummy, and duplicate-looking resources.

`PSP_GAME`, `SYSDIR`, `USRDIR`, `PARAM.SFO`, `EBOOT.BIN`, `BOOT.BIN`, movies,
audio, modules, and resource directories are layout facts or useful search
locations. They are not removal authority. The analyzer does not pretend that
it can prove runtime references from a filename, directory name, or a few
ASCII strings.

## Evidence model

Every reported candidate contains:

- exact normalized path and size;
- candidate category and language hint, where a path provides one;
- evidence supporting why it was surfaced;
- evidence against removal;
- a safety class and plain-language reason.

The current analyzer assigns every candidate `UnknownUnsafe`. The enum also
contains `ConditionallySafe` and `ProvenSafe` for future verified,
identity-specific profiles, but no generic rule currently reaches either
class. `EN` is not assumed required, and FR/DE/IT/ES or any other language is
not assumed removable.

## What filenames cannot establish

A name such as `movie`, `update`, `manual`, `dummy`, `install`, `language`, or
`FRENCH.PAK` cannot establish that executable code, a resource manifest, an
archive index, a checksum, a hard-coded path/LBA, or a user-selected language
will never reference it. A directory may also contain shared assets. Unknown
games therefore receive useful analysis, but no destructive action is
authorized.

## Removal, dummying, and compression

Physical removal requires a filesystem rebuild and can change directory
ordering, extents/LBAs, path tables, file sizes, archive indexes, checksums,
and hard-coded offsets. Replacing data with a same-size neutral payload keeps
some layout assumptions but can still violate compression formats, checksums,
resource semantics, or executable expectations. Leaving content intact and
using CSO compression is the only already-verified generic space-saving path;
its savings are compression savings, not removal savings.

A future trim profile must be tied to verified game identity, target
configuration, and strong executable/resource evidence. It must create a
derived output only, refuse collisions, retain the original hash, record an
exact change manifest, reparse the result, and fail closed on ambiguity.

## Verification levels

- **Structural verified:** the derived ISO reparses and required PSP layout
  structures remain present.
- **Identity retained:** the identity evidence remains consistent with the
  source and the selected profile.
- **Boot check passed:** a bounded PPSSPP run reaches a defined milestone,
  if a safe scriptable harness is later established.
- **Playability verified:** not claimed by this phase. A filesystem parse or a
  short boot cannot prove full playability.

The analyzer currently provides source-side structural evidence only. It does
not run PPSSPP and does not claim a boot or playability result.

## First POC decision

No generic safe removal rule was proven in this phase. The narrow analyzer and
evidence architecture are the useful result: they make uncertainty visible
without turning heuristics into deletion authority. A future first trim POC
must begin with one verified game/profile-specific rule and a separately
reviewed derived-ISO writer.

## Bounds and safety

Analysis uses a read-only file handle, a 64 MiB aggregate logical-read budget,
a 100,000-entry scan limit, a 16-level directory-depth limit, and a 64-entry
largest-file report. Malformed ISO9660 structures, oversized/missing malformed
PARAM.SFO, and budget exhaustion fail closed. Source bytes are never opened
for writing and no output is created.
