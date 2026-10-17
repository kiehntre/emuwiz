# RPCS3 native patch/cheat adapter

## Scope and sources

This feature is local/user-import only. It does not download patch collections,
modify PS3 game files, or rewrite RPCS3's shipped patch database.

Primary sources reviewed:

- [RPCS3 patch wiki](https://github.com/RPCS3/rpcs3/wiki/Game-Patches/b6fba89dfb545bba594f8d49379112309bdc48ce)
- [RPCS3 `Utilities/bin_patch.cpp`](https://github.com/RPCS3/rpcs3/blob/master/Utilities/bin_patch.cpp)
- [RPCS3 patch key constants](https://github.com/RPCS3/rpcs3/blob/master/Utilities/bin_patch.h)
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
          - "01.03"
    Group: "Performance"
    Patch:
      - [be32, 0x00100000, 0x3f800000]
```

The current engine supports exact app-version keys and `All`. It combines
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
an exact version key or the explicit `All` key; missing and incompatible
versions are reported separately and do not authorize unattended apply.

The PPU hash and RPCS3 title/version declaration are provenance and targeting
evidence. They do not replace EmuWiz's stronger verified PS3 identity. Disc or
package identity remains owned by the existing PS3 evidence pipeline.

The parser is bounded by file bytes, lines, line length, entries, operations,
and indentation depth. YAML anchors and aliases are refused to avoid expansion
or ambiguous retention. Generated output is reparsed before a shared preview.
No expressions are executed.

## Blue Cheats & Mods: installation boundary (October 2026)

Blue now exposes local activation through the existing backend and shared
transaction engine. Generic routing reports `LocalPreviewRequired`, not full
installation support. Format recognition, declaration compatibility, preview,
installation and in-game effectiveness are separate claims.

The supported installation subset is native Linux RPCS3 using the exact XDG
profile selected by Launch and a directly discovered ELF executable. AppImage,
Flatpak, portable profiles, executable wrappers, inherited `APPIMAGE`, and
profiles containing `vfs.yml` are refused. A verified extracted disc folder must
provide unique valid `TITLE_ID` and `APP_VER` fields and SELF evidence. Installed
updates for that title are refused because their effective version is not proven.
Launch's existing sole/remembered candidate selection and executable override
are reused; this feature never saves or changes emulator preferences. RPCS3
does not gain the separate shared resolver's AUTO/PREFERRED/FORCED mode API.

Blue scans the three canonical local definition paths:
`patches/patch.yml`, `patches/imported_patch.yml`, and
`patches/<TITLE_ID>_patch.yml`. Nothing is downloaded or imported. All present
databases must fit the conservative, round-trip-checked YAML 1.2 subset:
PPU plus 40 hexadecimal digits, one title/serial mapping per entry, exact
`NN.NN` versions or `All`, and numeric `byte`, `le16`, `be16`, `le32`, or `be32`
operations. Offsets must be hexadecimal, aligned, non-overlapping, and in range;
values must fit the operation width. Anchors, aliases, ambiguous scalar forms,
extra operands, unknown metadata (including Author/Notes), complex operations,
and duplicate hash/description identifiers refuse the whole inspection.
Many normal community databases therefore remain unavailable in this first
conservative integration. The broader parser's recognition of other operations
is not installation support.

The user selects a compatible entry, explicitly chooses its intended enabled
state (default off), previews the exact generated `patch_config.yml`, then
confirms with RPCS3 closed. The `All` declaration is stored under the `All`
configuration key, as required by RPCS3. Only an **absent** `patch_config.yml`
may be created. Existing configuration is protected, even if parseable; merging
or editing it is not supported. No definition database is ever a Blue write
target. Enabling additional entries requires a future preservation-proven merge
path; the current journey creates one entry and can Undo it.

Immediately before Apply, the bridge re-resolves the selected profile and checks
game identity/version, executable and configuration evidence, all canonical
patch files, required-absent paths and directory identities against Preview.
It refuses a running or unobserved RPCS3 process and retained pending History.
The adapter mode tag fingerprints the exact output, verified title/version,
file digests/identities, absent paths and directory identities, so a confirmation
cannot authorize a different generated configuration or refreshed evidence.
The existing materialized-output transaction performs atomic publication,
readback and durable History. Interrupted, partial or uncertain outcomes are
reported without a success claim. History offers a separate preview and explicit
confirmation for guarded Undo; external changes to the created config block
removal. No recovery-journal architecture is changed.

The older standalone plan builder is not the Blue activation path. Its narrow
legacy definition-import path now rejects wrong/unknown identity readiness,
multi-entry plans and existing targets before staging. It must not be treated
as a lossless general YAML/config merge facility.

## Runtime limitations

Declaration compatibility does not prove the running game's decrypted PPU hash.
RPCS3 must match that hash and consume the configuration at its next patch-engine
load. No emulator runtime test was performed, so patch effectiveness, hot reload
and behavior across RPCS3 releases remain unverified. No PS3 content, firmware,
RAP licence, disc image or existing emulator setting is modified by the supported
Blue journey. Validation uses synthetic identities, local YAML and disposable
profiles only.
