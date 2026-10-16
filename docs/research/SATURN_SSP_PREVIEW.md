# Saturn SSP preview: supported boundary

Mods & Cheats offers **Saturn SSP patch preview** for the selected Saturn
CUE/BIN. Choose a local package, select **Inspect SSP and disc**, then use
**Recheck source evidence** to detect changed inputs. Nothing is extracted or
patched. **Apply is disabled**, including after a successful inspection.

## Format evidence

The [Sega Saturn Patcher author's discussion](https://segaxtreme.net/threads/sega-saturn-patcher-discussion-thread.24721/page-3)
explains that an SSP can be renamed to ZIP and contains `version.txt`. This
establishes a packaging convention, not a complete native payload specification.

The [OpenROM builder at revision d9434b9385392d3a9e845b4f24082e543ee3c043](https://github.com/M5Devs/OpenROM/blob/d9434b9385392d3a9e845b4f24082e543ee3c043/core/lib/src/ssp_builder.dart)
emits `version.txt`, `changes.md5` and `._DFR` members. Its MD5 declarations
describe original ISO filesystem files, **not an entire disc**. Observation of
this producer is not evidence that every native SSP uses the same schema or
that its output interoperates with Sega Saturn Patcher. No upstream code or
game-derived payload is included in this implementation or its fixtures.

## What inspection establishes

* A bounded, unencrypted stored/deflated ZIP can be read completely, with CRC
  checks and SHA-256 fingerprints of its exact bytes and each file member.
* Member names are UTF-8, relative and unambiguous. Traversal, special entries,
  duplicate/case-alias names and file/directory conflicts are refused. Limits
  are 64 MiB package bytes, 128 MiB expanded bytes, 512 members and 16 KiB per
  metadata text member. Existing archive preflight limits also apply.
* `version.txt` and `changes.md5` are displayed as untrusted declarations.
  Missing `version.txt` leaves even the SSP packaging convention unconfirmed.
  Opaque payloads are hashed, never decoded as patch instructions.
* The selected CUE/BIN must have a complete existing Saturn manifest, recognized
  native System ID and product/version/disc fields. All regular components
  together must fit the existing 1 GiB inspection bound. Source stamps and
  manifest hashes bind the observed local inputs; they do not authenticate an
  official release or patch publisher.
* The existing manifest retains sector modes, multiple FILE relationships,
  INDEX 00/01, stored pregaps and separately declared/generated gaps. Sources
  are read only. Recheck refuses missing, replaced or modified evidence and
  requires another inspection. These are point-in-time checks, not writer
  exclusion or immutable-content guarantees.

## What inspection does not establish

Compatibility is always **Unknown: no verified SSP base binding**. A filename,
title, version declaration or per-file MD5 string cannot establish a match to
the selected disc. No payload is applied, no output exists, and no byte-exact
output verification or preservation claim is made. Lone BIN, CHD, GDI and
incomplete/unidentified CUE layouts are refused by this preview.

Application remains blocked on an established native SSP payload contract,
verified base/output identity evidence and a proven layout-preserving Saturn
rebuild path. Existing patch-application restrictions are unchanged. This
feature neither invokes an external patcher nor adds a conversion route.

Tests use self-authored ZIP containers and tiny native-header/track fixtures;
they verify the inspection boundary, not native SSP application compatibility.
