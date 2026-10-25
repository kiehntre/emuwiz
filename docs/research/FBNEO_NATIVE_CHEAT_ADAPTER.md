# FinalBurn Neo native cheat adapter

## Verified format

The adapter implements the documented FB Alpha/FBNeo per-set INI format:

```text
cheat "Infinite Lives" {
 type 0
 default 1
 0 "Disabled"
 1 "Lives", 0, 0x1234, 0x09
}
```

The file is named after the exact FBNeo driver/set shortname (`mslug.ini`) in
the configured FBNeo cheat-support directory. A numbered option contains
`CPU, address, byte value` triples; only those direct byte writes are
normalised. The parser is bounded by file, line, cheat, option, and operation
limits. Includes, malformed records, unknown lines, and unsupported native
operations remain represented as issues/opaque data and block safe readiness.

Sources:

- [FBNeo Cheat Format](https://github.com/finalburnneo/FBNeo/wiki/Cheat-Format)
- [FBNeo Cheat Dialog](https://github.com/finalburnneo/FBNeo/wiki/dialog_cheats)
- [FBNeo Support File Path](https://github.com/finalburnneo/FBNeo/wiki/dialog_support_path)
- [libretro FBNeo documentation](https://github.com/LLeny/libretro-docs/blob/master/docs/library/fbneo.md#cheats)
- [FBNeo CPU cheat registration](https://github.com/finalburnneo/FBNeo/blob/master/src/burn/burnint.h)

## Identity and systems

The primary identity is the exact FBNeo shortname. EmuWiz does not assume
that a MAME name is automatically an FBNeo name. Parent/clone relationships
remain external evidence and never authorize a different shortname. Titles
are display metadata only. The target also records the reported system (for
example Neo Geo or CPS2), but the adapter does not infer a board memory map
from that label.

The same bounded parser covers Neo Geo, CPS1, CPS2, CPS3, and other FBNeo
drivers because the file grammar is per-set; CPU/address-space semantics stay
on each operation. Multi-byte operations are not invented from a byte-write
line, and opaque operations are not sent to the neutral analyser.

## State and apply boundary

`default 0` is represented as disabled and a non-zero default option as
enabled. FBNeo's native dialog persists the selected option, but runtime
availability still depends on the core's native cheat support being enabled;
loadability facts therefore expose the native state and a runtime-enable
requirement rather than claiming in-game execution.

The writer deterministically renders and merges entries without replacing
unrelated entries or downloading/modifying community databases. An exact,
verified shortname is required. The apply plan validates the destination under
the configured FBNeo root, stages output, re-verifies it through the shared
preview, and uses the shared atomic transaction/history/rollback machinery.
The ROM/set archive is never a destination or source of mutation.

## GUI and legal mode

The adapter exports the typed facts needed by the existing Cheats workflow:
profile, shortname, destination, native format, state, and restart/runtime
requirements. Generic routing remains unchanged in this adapter-specific
feature; no new provider or cheat-pack acquisition path is added. GUI use is
therefore conservative until the existing arcade workflow supplies an exact
FBNeo target and a dedicated presentation seam.

All operation is local/user-import based. EmuWiz does not scrape or download
`cheat.dat`, FBNeo cheat packs, or other community databases.
