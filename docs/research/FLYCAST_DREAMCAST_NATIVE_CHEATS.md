# Flycast / Dreamcast native cheats

## Current capability correction

Blue now provides local native-file inspection and supported-entry preview.
Installation remains disabled: the original product-code destination assumption
below does not establish Flycast consumption. See
[the integration boundary and blockers](FLYCAST_BLUE_CHEATS_INTEGRATION.md).
The shared file writer/rollback API is not a proven Flycast installation route.
No version/profile layout or in-game effect is verified by this work.

## Sources and scope

This work is based on the current upstream Flycast source, not on a bundled
cheat database:

- [Flycast `core/cheats.h`](https://github.com/flyinghead/flycast/blob/master/core/cheats.h)
- [Flycast `core/cheats.cpp`](https://github.com/flyinghead/flycast/blob/master/core/cheats.cpp)
- [Flycast issue #279](https://github.com/flyinghead/flycast/issues/279), documenting the distinction between Flycast `.cht` files and CodeBreaker/GameShark media

Flycast is GPL-licensed. EmuWiz does not bundle Flycast source, commercial
cheat databases, CodeBreaker discs, or Action Replay/GameShark lists.

## Flycast format

The native standalone Flycast format is a sectionless INI-like file, not automatically associated by `<game-id>.cht`. Flycast loads the `[cheats]`
mapping in `emu.cfg` or uses a content-filename search in `Dreamcast.CheatPath`. Upstream loads a bounded
file, reads an optional `cheats` count, and then reads `cheatN_` fields such as:

`desc`, `address`, `cheat_type`, `memory_search_size`, `value`, `enable`,
`dest_address`, `address_bit_position`, `repeat_count`,
`repeat_add_to_value`, and `repeat_add_to_address`.

Persistent enable/disable is represented by `cheatN_enable`. Flycast reads the
file when the game identity changes; a running game therefore requires a game
restart/reload to consume changed cheats.

The adapter bounds file size, lines, line length, and entry count. It preserves
comments, unrelated keys, unknown native fields, and opaque operation types.
Rendering is deterministic and duplicate insertion is refused.

## Code types and interoperability

Upstream defines native operation types for disabled, direct set, increment,
decrement, conditional comparisons, and copy operations. EmuWiz normalizes
only direct 1/2/4-byte writes. Conditional, increment/decrement, copy, and
unknown operations remain opaque and are not fabricated as memory ranges.

Flycast's source also exposes a GameShark-code import path in its UI. The
result is converted into Flycast-native entries before persistence. The
adapter deliberately does not reimplement a Dreamcast CodeBreaker/GameShark
decoder: source documentation does not establish a stable, complete external
decoder contract, and the existing Action Replay decoder is not assumed to be
Dreamcast-compatible. User-imported/native entries can still be preserved and
previewed as opaque.

## Identity and multi-disc policy

The legacy file planner requires a verified Dreamcast product code and a
destination named for that product code. That destination is not a proven
Flycast-consumed path, so Blue does not expose Apply. Title-only evidence is rejected.
Existing EmuWiz Dreamcast/IP.BIN/product-code evidence is reused; this feature
does not alter GDI, CHD, IP.BIN, or DCP logic.

The Flycast cheat file itself has no disc-number field. Therefore EmuWiz
refuses Disc 2+ unless the caller explicitly supplies evidence that the cheat
is shared across discs. This prevents a Disc 1-specific code from silently
being installed for another disc. A content/data-track hash may be retained as
provenance, but the current native Flycast file format does not consume it.

## Transaction and preservation model

The adapter uses the shared preview, destination precondition, bounded staged
output, atomic publication, backup, journal, verification, and exact rollback
primitives. It writes only the per-game Flycast `.cht` file. It never modifies
Dreamcast media, IP.BIN, GDI/CHD files, global Flycast defaults, BIOS, or VMU
data. An externally modified destination blocks destructive rollback.

## Support matrix

| Capability | Result |
|---|---|
| Native Flycast `.cht` parse | Implemented |
| Direct native writes | Parsed and normalized for 8/16/32-bit writes (native shifts 3/4/5) |
| Native conditional/increment/copy operations | Preserved opaque; no invented semantics |
| Persistent per-cheat enable state | Inspected via `enable`; Blue does not install |
| CodeBreaker/GameShark external decoding | Research-only; not duplicated |
| Exact product-code identity | Required |
| Disc-specific binding | Disc 1 supported; later discs require explicit shared evidence |
| Deterministic merge/write | Legacy file primitive; unavailable in Blue |
| Transactional apply/rollback | File primitive tested; Flycast installation not proven |
| Global Flycast configuration mutation | Refused |
| Disc/media mutation | Refused |

## GUI and routing

Selected standalone Flycast reports inventory-only capability, with native
format inspection and a dedicated Blue entry preview. Apply is disabled with
an explicit mapping/validation blocker. The source file can be reviewed without
changing Launch, configuration, game media, BIOS or VMU data. Runtime cheat
effectiveness remains unverified.
