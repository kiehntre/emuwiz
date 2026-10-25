# Classic Game Genie / encoded console cheat family

## Scope

This feature is a local, clean-room decoder and normalisation foundation. It
does not patch ROMs, download cheat databases, or infer revision safety from a
title alone.

The implementation covers:

- NES six-character and eight-character Game Genie codes.
- SNES eight-character Game Genie codes.
- Mega Drive / Genesis eight-character Game Genie codes.
- Game Boy six-character and nine-character hexadecimal Game Genie codes.
- Master System and Game Gear six/nine-character hexadecimal Game Genie codes.

The algorithms were independently written from public interoperability
descriptions. The implementation does not copy a proprietary database or
translate a decoder implementation line-for-line.

## Evidence and format boundaries

Unscoped code shapes intentionally remain ambiguous where platforms overlap:
an eight-character code can be NES, SNES, or Genesis; a six-character
hexadecimal code can be NES or Master System/Game Gear. Callers must supply the
platform before decoding.

The decoded result retains:

- original input;
- normalized encoded input;
- platform and format;
- address, replacement value, width, and optional compare byte;
- neutral CheatOperation projection where it is an unconditional direct write;
- typed issues and revision-safety state;
- clean-room provenance.

Compare bytes are never discarded or treated as unconditional writes. They are
represented by the conditional `CheatOperation::ConditionalWrite8` IR variant,
so compatibility analysis can report conditional overlap explicitly.

## Game Boy format

The Game Boy implementation is based on [Jeff Frohwein’s Game Boy Game Genie
technical page](https://www.devrs.com/gb/files/gg.html), cross-checked against
the public [Game-Genie-Good-Guy decoder source](https://github.com/Mte90/Game-Genie-Good-Guy/blob/master/decode.c)
without copying its implementation.

Six- and nine-character hexadecimal codes are accepted with optional display
separators. The first two nibbles are the replacement byte; the next three
are the low address nibbles and the complemented high nibble. Addresses are
restricted to `$0002..$7FFF`. Six-character codes have no compare byte and
therefore apply across ROM banks. Nine-character codes decode the compare byte
by rotating the encoded pair right by two and XORing `$BA`; the documented
check-nibble rule rejects `A XOR C` values `1..7`.

The normalized nine-character form is conditional and retains the compare
byte. Invalid alphabet, length, check, address, and non-ASCII input fail
closed.

## Revision safety

Decoded does not mean safe to apply. Evidence is ranked as:

1. exact verified ROM SHA-256;
2. verified ROM identity;
3. verified region/revision evidence;
4. title-only warning;
5. unverified.

This phase has no apply path. The result is therefore suitable for preview and
for a later adapter-specific planner, but it cannot silently modify a ROM or
promote a title match to verified identity.

## Platform notes

NES uses the documented APZLGITYEOXUKSVN alphabet and supports the six-character
replacement form plus the eight-character compare form. SNES uses its distinct
DF4709156BC8A23E alphabet. Genesis uses the distinct 32-symbol alphabet and
16-bit replacement semantics. Master System and Game Gear use the documented
hexadecimal six/nine-character form with an optional compare byte.

Game Boy uses the hexadecimal family described above and is selected
explicitly because six/nine-character hexadecimal shapes overlap with SMS
and Game Gear.

## GUI and emulator projection

The Cheat Sources page now provides a local Game Genie preview: one code or
one code per line, explicit platform selection, normalized code, decoded
fields, compare explanation, identity warning, issues, and provenance. No
database downloader, ROM patcher, or new emulator-native adapter was added.
Runtime projection remains limited to existing adapters that can represent the
decoded semantics exactly; compare-bearing codes are otherwise preview-only.

## Public research references

- NES format and mapping: [GameHacking.org NES documentation](https://wiki.gamehacking.org/Hacking_NES)
- NES emulator-facing fields: [FCEUX documentation](https://documentation.help/FCEUX/documentation.pdf)
- SNES and Genesis reference implementations used only as behavioural
  comparison: [MAME simple cheat plugin](https://github.com/mamedev/mame/blob/master/plugins/cheat/cheat_simple.lua)
- Cross-platform public format overview:
  [RetroMultiTools cheat-code reference](https://github.com/SvenGDK/RetroMultiTools/blob/main/docs/reference/cheat-codes.md)
- Master System/Game Gear background:
  [SMS Power! Game Genie discussion](https://www.smspower.org/forums/8070-GameGearGameGenieCodeFormat)

## Tests

Synthetic tests cover NES six/eight-character decoding, Game Boy public
vectors, compare retention, SNES and Genesis vectors, Master System/Game Gear
optional compare handling, ambiguous unscoped shapes, malformed/unsupported
input, deterministic decoding, title-only warnings, conditional IR and
compatibility projection. No network or ROM mutation is involved.
