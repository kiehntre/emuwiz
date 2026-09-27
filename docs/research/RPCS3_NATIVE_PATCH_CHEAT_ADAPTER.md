# RPCS3 native patch/cheat adapter

## Scope and sources

This feature is local/user-import only. It does not download patch collections,
modify PS3 game files, or rewrite RPCS3's shipped patch database.

Primary sources reviewed:

- [RPCS3 patch wiki](https://github.com/RPCS3/rpcs3/wiki/Game-Patches/b6fba89dfb545bba594f8d49379112309bdc48ce)
- [RPCS3 `Utilities/bin_patch.cpp`](https://github.com/RPCS3/rpcs3/blob/master/Utilities/bin_patch.cpp)
- [Current raw patch engine source](https://raw.githubusercontent.com/RPCS3/rpcs3/master/Utilities/bin_patch.cpp)

The source is GPL-2.0-or-later. EmuWiz does not copy RPCS3 code; it models the
documented file contract and uses an independent bounded parser. Patch files
and game content remain user-owned inputs.

## RPCS3 format

RPCS3 loads YAML from the configuration directory's `patches/` directory. Its
documented local import path is `patches/imported_patch.yml`; enablement is
stored separately in the configuration-root `patch_config.yml`. A patch file
must contain a top-level `Version`, and the current RPCS3 source requires the
value `1.2` before loading the file.

The patch document is organised as:

```yaml
Version: 1.2
PPU-HASH:
  "Patch description":
    Games:
      "Display title":
        "BLUS12345":
          "01.03":
    Group: "Performance"
    Patch:
      - [be32, 0x00100000, 0x3f800000]
```

The current engine supports exact app-version keys and `all`. It combines
patches by PPU hash and description, and gives more specific serial/version
matches precedence over broad matches. Duplicate definitions are merged by
patch version; the engine keeps the higher patch version.

The engine recognises direct scalar/byte patch operations among a larger native
set, including `byte`, `le16`, `be16`, `le32`, `be32`, `bd32`, and 64-bit/float
forms. RPCS3 also supports allocation, jumps, file moves/hiding, UTF-8 and
other native operations. EmuWiz only normalizes the small proven direct-write
subset. Unknown operations are retained as opaque and prevent a fully
interpreted readiness claim.

`patch_config.yml` stores enabled state below hash → description → title →
serial → app version. The engine accepts a mapped `Enabled: true/false` value
and a legacy scalar boolean. EmuWiz writes this user-owned config path only.
The patch engine consumes the files at game execution/patch-engine load time,
so the adapter reports restart/reload required; it does not claim live reload.

## Identity and safety model

Apply requires a verified PS3 title ID/serial and an exact matching `Games`
serial. A title-only match is not eligible. The selected app version must be
an exact version key or the explicit `all` key; missing and incompatible
versions are reported separately and do not authorize unattended apply.

The PPU hash and RPCS3 title/version declaration are provenance and targeting
evidence. They do not replace EmuWiz's stronger verified PS3 identity. Disc or
package identity remains owned by the existing PS3 evidence pipeline.

The parser is bounded by file bytes, lines, line length, entries, operations,
and indentation depth. YAML anchors and aliases are refused to avoid expansion
or ambiguous retention. Generated output is reparsed before a shared preview.
No expressions are executed.

## Apply/rollback

The adapter stages deterministic `imported_patch.yml` and `patch_config.yml`
under an EmuWiz-controlled staging root, previews both destinations, then
uses the existing shared transaction journal, atomic publication, destination
preconditions, backups, and exact rollback. A stale source/destination
fingerprint prevents apply. External modification blocks destructive rollback.

Only these RPCS3-owned user paths are targeted:

| Destination | Purpose |
| --- | --- |
| `patches/imported_patch.yml` | user/imported patch definitions |
| `patch_config.yml` | user patch enablement |

`patches/patch.yml` and other shipped/community files are deliberately not
selected. This first adapter preserves unrelated definitions at the model
boundary, but does not attempt to rewrite arbitrary unsupported YAML constructs;
such content is opaque and should remain preview-only until a lossless merge
path is available.

## Current EmuWiz integration

Before this feature, RPCS3 local inspection exposed patch inventory but generic
cheat routing reported `InventoryOnly`. The adapter adds:

- bounded `Rpcs3PatchFile`, group, entry, operation, issue, readiness, state,
  plan, and loadability types;
- verified title-ID and version readiness;
- direct-write normalization evidence plus opaque-operation retention;
- deterministic user patch/config rendering;
- shared preview, atomic apply, journal, and rollback integration;
- RPCS3 routing as `Supported` while leaving ordinary file-layer mods as a
  separate adapter;
- GUI explanation that RPCS3 patch application changes user patch/config files,
  not game files.

## Explicit refusals and limitations

- no title-only apply;
- no wrong-title or wrong-version apply;
- no game/media mutation;
- no shipped patch DB mutation;
- no expression execution or guessed semantics for complex native operations;
- no provider/network acquisition;
- no live-reload claim;
- no broad YAML rewrite when unsupported constructs are present.

The current implementation is therefore safe for documented direct patch
definitions and exact local identity, while complex RPCS3 native patches remain
visible as opaque/preview-only until their semantics and lossless merge path
are proven.
