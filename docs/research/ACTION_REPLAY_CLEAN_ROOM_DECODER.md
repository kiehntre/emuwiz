# Action Replay clean-room decoder foundation

This feature adds a bounded, local-only interoperability decoder. It is
independently implemented and does not contain a Datel database, proprietary
source, provider credentials, or network retrieval.

## Pipeline

The detect_action_replay_format function performs shape-only detection.
Eight-digit address/value records are classified as the conservative GBA
candidate. Sixteen-digit records are intentionally reported as ambiguous
between PS2, GameCube, and Nintendo DS until the caller supplies an explicit
platform.

The platform decoder then parses the supported direct-write subset into the
existing neutral CheatOperation IR. Unsupported control, conditional, pointer,
master, loop, and other stateful operations remain raw or receive a typed
issue. Malformed input is never silently rewritten.

## Formats

- GBA Action Replay/GameShark: raw direct writes with a bounded address and
  16/32-bit value; encrypted formats are not claimed.
- PS2 Action Replay v1/v2/MAX: only the already-decoded direct-write shape is
  accepted; encrypted/MAX-specific records remain outside the supported subset.
- GameCube Action Replay: Dolphin-compatible direct-write opcode families are
  projected; control families remain UnsupportedRaw.
- Nintendo DS Action Replay: the existing conservative DS classifier is reused,
  preserving conditionals, pointers, activators, loops, and master records as
  unsupported raw evidence.

No checksum recovery or proprietary decryption is attempted.

## Provenance and adapters

Every result records an independent implementation method, public
documentation/emulator behavior as its reference basis, and local-only /
database-free flags. Projection is a read-only CheatAdapterProjection; it does
not install or mutate emulator files. Existing native adapters remain the
authority for any later write workflow.

The GUI decoder card accepts pasted local code, shows detected shape, explicit
target, decoded instructions, warnings, and provenance. It has no provider,
upload, or apply action.

## Limitations

Encrypted Action Replay/CodeJunkies variants, undocumented opcodes, checksums
not exposed by the current record format, and ambiguous platform shapes require
additional independently verifiable documentation and are refused or retained
as raw input.
