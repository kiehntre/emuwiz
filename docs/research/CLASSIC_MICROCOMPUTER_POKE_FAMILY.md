# Classic microcomputer POKE cheat family

> **Consolidated:** `PokeIdentityState` and the static emulator table described below were superseded by `CheatApplicabilityMatch` and the runtime capability model; see [`../cheats/CHEAT_CONSOLIDATION.md`](../cheats/CHEAT_CONSOLIDATION.md).

This feature adds a neutral, read-only normalization model. It does not write
emulated memory, modify media, execute BASIC, execute trainer scripts, or
bundle a newly researched database.

## Model and identity

`PokeCheat` contains platform, ordered `PokeOperation` values, address, width,
value, optional original-value guard, memory space, bank/page and provenance.
Locations compare by `(memory_space, bank, address)`, so identical numeric
addresses in different banks are not conflicts.

Apply eligibility is deliberately separate from parsing:

1. exact verified media hash;
2. verified game/release identity;
3. title-only or unverified identity is preview-only;
4. missing identity is blocked.

## Existing ZX support

The existing bounded `.pok` parser remains unchanged. The new
`normalize_zx_pok_family` projection preserves its bank and original-value
fields, including multiple writes. This is an additional neutral projection;
the existing `CheatDocument` projection is not changed.

Fuse’s documented POKE behavior confirms bank values `0`–`7`, current mapping
`8`, address limits, byte values, and activation until reset:
[Fuse manual, POKE memory](https://manpages.org/fuse).

## Platform boundaries

| Platform | Implemented | Safe semantics |
| --- | --- | --- |
| ZX Spectrum | `.pok` projection | 16-bit address, 8-bit value, explicit bank and original guard |
| Amstrad CPC | local/simple import | `POKE address,value`; banked forms refused |
| Commodore 64 | local/simple import | 16-bit CPU address; VICE bank is never inferred |
| Atari ST | manual/simple import | explicit 24-bit address; no stable bundled trainer format claimed |
| MSX | local/simple import | explicit mapper slot/page accepted; bare address has no bank claim |
| BBC Micro / Acorn 8-bit | manual/simple import | no automatic machine-map or database claim |

The generic parser accepts only `POKE address,value[,original]` lines and
comments. It does not interpret BASIC expressions, loops, scripts or trainer
commands. Address/value overflow, malformed lines, unsupported banking and
empty entries fail closed.

## Emulator projection audit

The capability model is descriptive, not an installer:

- Fuse / RetroArch Fuse: native ZX `.pok` path, subject to exact target review.
- Caprice32: runtime/manual memory projection only.
- VICE: runtime monitor/memory-write projection; VICE’s binary monitor exposes
  memory-set address, memory-space and bank fields, so bank context must not be
  discarded ([VICE binary monitor](https://vice-emu.sourceforge.io/vice_13.html)).
- Hatari: runtime debugger `memwrite`; no stable native cheat-file contract is
  claimed ([Hatari debugger manual](https://www.hatari-emu.org/doc/debugger.html)).
- openMSX: runtime `poke`/`poke16` and trainer-oriented tooling; mapper context
  remains explicit ([openMSX command reference](https://openmsx.org/manual/commands.html)).
- BeebEm / b-em: manual runtime action only in this phase.

No native writers are added here. RetroArch/libretro routing may consume the
neutral writes where an existing adapter proves the target format; this module
does not invent a writer for other emulators.

## Runtime projection continuation

The runtime capability model is intentionally more specific than “supported”:

| Emulator | Classification | Projection |
| --- | --- | --- |
| Fuse | `SupportedNativeRuntime` | deterministic `.pok` file with bank, address, value and original guard |
| VICE | `SupportedMonitorCommand` | deterministic `>` monitor commands for explicit unbanked byte writes |
| Hatari | `PreviewOnly` | `memwrite` exists, but exact scripted argument semantics and guards are not proven |
| Caprice32 | `PreviewOnly` | `--autocmd` exists, but a memory-write command contract is not documented |
| openMSX | `PreviewOnly` | interactive `poke` exists, but mapper projection is not proven |
| BeebEm / b-em | `PreviewOnly` | debugger inspection is documented; safe scripted writes are not proven |

Fuse’s manual documents `.pok` loading and activation. VICE documents both the
`>` memory-write monitor command and playback files in its
[monitor manual](https://vice-emu.sourceforge.io/vice_12.html). Hatari documents
`memwrite` and `--parse` but its command argument grammar remains an adapter
follow-up ([debugger manual](https://www.hatari-emu.org/doc/debugger.html)).
Caprice32 documents `--autocmd` but not an equivalent memory-write command
([manual](https://github.com/ColinPitrat/caprice32/blob/master/doc/man.html)).
BeebEm documents debugger inspection commands but not a safe scripted write
path ([README](https://github.com/AndyA/beebem/blob/master/doc/README.txt)).

Runtime projection is blocked unless identity is verified. Source media is
never changed. Generated output is returned as inspectable preview text; no
emulator is launched by this adapter.

## Trainer expression grammar

The accepted grammar is deliberately tiny:

```text
POKE integer, integer [, integer]
POKE integer, integer : POKE integer, integer
```

Decimal, `$hex`, and `0xhex` integers are accepted. The optional third integer
is an original-value guard. Lines, operation count, expression bytes and
integer widths are bounded. `FOR`, `NEXT`, `DATA`, `READ`, `SYS`, `CALL`, `USR`,
`RANDOMIZE`, assignments, arithmetic, variables and arbitrary expressions are
classified as unsafe/unsupported rather than simplified.

## Source expansion

The existing ZXDB/.pok provider remains the only non-local classic source
integration. CPC, MSX and BBC/Acorn descriptors are now explicit local-import
provider entries, matching the existing C64 and Atari ST legal boundary. No
public source with both clear redistribution rights and sufficiently stable,
identity-bearing classic POKE data was established in this pass. Community
manuals and emulator documentation are research evidence, not cheat payload
providers. No ROM, disk, tape, trainer executable or opaque patch archive is
downloaded or bundled.

## Legal/source boundary

ZXDB and the existing `.pok` provider remain governed by the already-audited
provider metadata. C64 and Atari ST remain local/user-import only. MSX and BBC /
Acorn remain local/manual only. No random web scraping, remote upload or
provider payload bundling was added.

## GUI

The Cheat Sources page now exposes a Classic POKE preview section with platform,
address width, value width and banking semantics. It explains that manual and
local imports are non-executing and that title-only identity is not Apply-safe.
The existing source/provider controls remain unchanged; no destructive Apply
control is introduced by this feature.

## Remaining limits

There is no generic proof that a BASIC POKE address means the same thing across
revisions, memory maps, cartridges or mapper configurations. Native emulator
writers, per-emulator runtime launch integration, and exact game identity
matching remain separate follow-up work.
