# Flycast / Dreamcast native cheats

## Sources and scope

This work is based on the current upstream Flycast source, not on a bundled
cheat database:

- [Flycast `core/cheats.h`](https://github.com/flyinghead/flycast/blob/master/core/cheats.h)
- [Flycast `core/cheats.cpp`](https://github.com/flyinghead/flycast/blob/master/core/cheats.cpp)
- [Flycast issue #279](https://github.com/flyinghead/flycast/issues/279), documenting the distinction between Flycast `.cht` files and CodeBreaker/GameShark media

Flycast is GPL-licensed. EmuWiz does not bundle Flycast source, commercial
cheat databases, CodeBreaker discs, or Action Replay/GameShark lists.

## Flycast format

The native standalone Flycast format is a sectionless INI-like file, normally
named `<game-id>.cht` below Flycast's cheat directory. Upstream loads a bounded
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

Apply requires a verified Dreamcast product code and an exact Flycast cheat
destination named for that product code. Title-only evidence is rejected.
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
| Direct native writes | Parsed and normalized for 8/16/32-bit writes |
| Native conditional/increment/copy operations | Preserved opaque; no invented semantics |
| Persistent per-cheat enable state | Implemented via `enable` |
| CodeBreaker/GameShark external decoding | Research-only; not duplicated |
| Exact product-code identity | Required |
| Disc-specific binding | Disc 1 supported; later discs require explicit shared evidence |
| Deterministic merge/write | Implemented |
| Transactional apply/rollback | Implemented |
| Global Flycast configuration mutation | Refused |
| Disc/media mutation | Refused |

## GUI and routing

The native routing capability for selected Flycast is upgraded from
inventory-only to supported, with the native format reported as Flycast `.cht`.
The shared preview surface identifies the Dreamcast product-code evidence and
explains that direct writes are interpreted while other native operations stay
opaque. The existing generic Cheats workflow remains intentionally conservative
about automatic provider/database browsing; the adapter API is ready for a
future per-entry editor using the same typed request.
