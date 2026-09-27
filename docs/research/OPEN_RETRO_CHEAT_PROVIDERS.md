# Open Retro Cheat Providers

## Scope and implementation boundary

EmuWiz already had the provider registry, pinned Libretro snapshot workflow,
local cheat import, identity matching, emulator adapters, and reversible
installation machinery. This pass adds a legal/provenance catalogue and two
read-only normalizers. It does not download new sources, execute code, modify
ROMs, or change the existing install transaction.

## Provider modes

`OpenCheatProviderMode` makes the acquisition decision explicit:

- `MetadataIndex`: ZXDB is exposed as an attributed metadata/index source; its
  derived database content is not bundled.
- `Downloadable`: Libretro is available through the existing explicit,
  pinned HTTPS snapshot path. The repository describes `.cht` as plain-text,
  game-specific data, but its contents are community contributed and source
  terms/provenance must remain visible.
- `UserImport`: installed WHDLoad slaves and C64/Atari ST files remain local
  inputs. No unclear source is silently mirrored.
- `Bundled`: no new bundled payload was added.

Every declaration records URL, licence statement, redistribution status,
update method, platform coverage, format, provenance, last-update field,
mode, and trust state.

## ZX Spectrum

ZXDB is an open database and points users to ODbL 1.0 guidance. It is therefore
metadata/index mode with attribution and open-derivative obligations, not a
bundled cheat payload. The documented POK format is parsed conservatively:
`N` starts a trainer, `M`/`Z` carry bank/address/value/original-value decimal
fields, and `Y` terminates the file. No BASIC, shell command, or arbitrary
script is executed. Bank information remains in the parsed source model; the
generic cheat IR currently exposes the memory write while preserving source
provenance.

Sources: [ZXDB](https://github.com/zxdb/ZXDB), [World of Spectrum POK format](https://worldofspectrum.org/faq/reference/formats.htm), and [Fuse POK documentation](https://manpages.debian.org/unstable/fuse-emulator-common/fuse.1.en.html).

## Amiga / WHDLoad

WHDLoad options are read from the installed slave’s documented custom-option
declaration and never written back to the slave. `C1`–`C5` fields with the
documented option types are projected as labelled options; EmuWiz does not
guess that an option means infinite lives, invulnerability, or level select.
The installed slave path and hash remain the provenance anchor. Actual option
configuration belongs to the existing launcher integration.

Source: [WHDLoad options](https://www.whdload.net/docs/en/opt.html).

## Commodore 64 and Atari ST

The audit found no source with sufficiently clear redistribution terms,
stable format, and identity quality to register as a bundled or automatic
download provider. Both are represented as local user-import modes only.
EmuWiz does not scrape commercial or mystery cheat sites.

## Libretro / RetroArch

The existing provider already resolves a repository revision, downloads an
immutable archive on explicit request, validates and caches it, and retains
source provenance. This pass does not duplicate that path or claim bundled
redistribution. The source browser now makes the non-bundled/licence status
visible alongside the ZXDB, WHDLoad, C64, and Atari ST choices.

Source: [libretro-database](https://github.com/libretro/libretro-database) and [Libretro cheat documentation](https://github.com/libretro/docs/blob/master/docs/guides/cheat-codes.md).

## Identity and safety

Existing matching remains authoritative: exact ROM/hash or verified platform
identity precedes strong title/revision evidence, and title-only matches remain
weak suggestions. Local parsing is deterministic and network-free. Provider
metadata never grants permission to install, and unsupported or unclear
licensing remains visible rather than being silently promoted to bundled data.

## Limits

This task does not add ZX Spectrum emulator installation, a new RetroArch
adapter, a WHDLoad slave writer, C64/Atari ST database import, web scraping,
or payload redistribution. It also does not infer trainer semantics from
names; only documented POK writes and documented WHDLoad option declarations
are normalized.
