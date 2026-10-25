# Game Slimmer profiles

EmuWiz Game Slimmer profiles are local, versioned JSON knowledge records. They
describe evidence-backed rules for a specific verified game identity and do not
execute operations. Schema version `1` is represented by
`archivefs_core::game_slimmer_profiles::ProfileBundle`.

The profile chain is:

`verified identity -> matching profile -> bounded asset rule -> evidence -> permitted operation -> required verification`

Identity is bound to structured evidence, not filenames. Supported bindings are
PSP Disc ID, PS2 serial with optional executable CRC, Xbox Title ID, GameCube
Game ID, Wii Game ID, and ScummVM game ID/engine/variant. An unverified identity
cannot match a profile.

Rules use a closed operation enum: `retain`, `remove`,
`replace_with_validated_dummy`, `zero_payload_preserve_layout`, `drop_partition`,
or `recompress_only`. There are no executable commands or scripts in the
format. Selectors are bounded relative exact paths or path prefixes; absolute,
parent-traversal, Windows-drive, and backslash paths are rejected.

`to_json` validates and emits deterministic pretty JSON. `from_json` enforces a
bounded document size, strict unknown-field rejection, schema version, duplicate
profile rejection, and all nested validation. Imported community profiles remain
untrusted; parsing cannot turn one into a trusted profile. No network import or
automatic profile download exists.

The PSP integration accepts an existing read-only `PspShrinkInspection` plus
caller-supplied verified Disc ID/revision evidence. It retains source SHA-256
provenance and performs matching only; it does not write, delete, rebuild, or
slim an image.
