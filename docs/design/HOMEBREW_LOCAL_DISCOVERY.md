# Homebrew Discovery Phase 1

`homebrew_catalogue::LocalCatalogueProvider` reads an explicitly selected local
JSON file, validates the entire catalogue, and keeps an immutable snapshot.
It implements the existing `ProjectDiscoveryProvider`. Its separate
`ArtifactReleaseProvider` implementation returns only stored build metadata;
it does not call or replace `homebrew_github::GithubReleasesProvider`.

An executable example accepts a catalogue path, optional search text and optional
canonical platform ID:

```sh
scripts/cargo-iso --low-debug run -p archivefs-core --example homebrew_discover -- \
  crates/archivefs-core/tests/fixtures/homebrew/starter-catalogue.json "tobu" "Game Boy"
```

The output includes full records, build variants, rights and observations as JSON.
There is no default catalogue, remote URL loading, account, refresh, download,
installation, identity resolution, DAT matching or GUI change. Acquisition URLs
remain future, explicit handoffs subject to the existing acquisition policy.

## Catalogue contract

Schema version 1 has `schema_version`, `provider` and `records`. Each record has
an existing `HomebrewProject`, zero or more existing `HomebrewArtifactListing`s,
and one or more `CatalogueObservation`s (`source_url`, `observed_on`, `note`).
Observations record a calendar date and the scope/confidence of evidence, not an
expiry guarantee. The checked-in [starter catalogue](../../crates/archivefs-core/tests/fixtures/homebrew/starter-catalogue.json)
is a complete example. Existing public metadata models and trait signatures are
unchanged; these wrappers define a new local file format, not a cache migration.

Each catalogue owns exactly one provider namespace. Every project and listing
must agree with it. Curator IDs such as `emuwiz-starter/tobu-tobu-girl` remain
curator identities; they are not manufactured GitHub repository IDs. Loading a
second catalogue with another namespace preserves independent identities even
when titles and URLs match. Explicit project links retain their evidence and
never deduplicate identities. Callers aggregating catalogues must retain the
full provider/project pair, never deduplicate by title or filename.

Each versioned listing needs an explicit `provider_release_id`; an unversioned
project may have one `release: null` batch. Artifact IDs distinguish builds
inside a release. Equal filenames are allowed with distinct artifact IDs.
Projects, releases and artifacts with identical IDs and equivalent metadata
collapse only exact duplicates. Conflicting metadata is an error; records and
rights are never merged to repair it. Claims, links and observations are sorted
and exact duplicates removed before comparison. Results sort by folded title,
then exact project ID; listings and builds sort by their IDs.

Search is a trimmed, case-insensitive Unicode substring of title or summary,
using canonical normalization and full default case folding (including sharp-s,
Greek sigma and composed/decomposed accents). It does not perform locale-specific
collation, fuzzy matching or translation. IDs and stored
display text remain unchanged. The two Unicode dependencies add three locked
packages; existing dependency versions are unchanged.
The optional platform filter accepts an exact registry ID and returns projects
with at least one matching recorded artifact claim. It does not rewrite or hide
other builds in that project's stored listing. Unknown/absent claims stay visible
in unfiltered discovery but cannot satisfy a registered platform filter. No
platform is inferred from an extension, title, URL or adjacent build. Even a
matching claim is evidence, not tested compatibility or verified game identity.

The loader rejects malformed JSON, unsupported schema versions, unknown fields
(including misspelled nested rights fields), mismatched identities, empty required
evidence, bad dates, invalid URLs/digests, conflicting duplicates and files over
4 MiB. Errors name a field or file where possible. Local files must be regular
files; all reads are bounded. Discovery after loading performs no further I/O.
Reloading and handling stale observations are explicit caller decisions.

## Starter corpus and rights

Observed 2026-10-11. Only metadata and original short descriptions are included.
No ROM binaries, artwork, music, repository archives or compiled games were
retrieved or bundled. Filenames describe developer-documented build outputs,
not inspected or verified downloadable assets. Sizes and checksums are unknown.

| Project / curator ID | Platform and builds | Version | Code / asset evidence |
| --- | --- | --- | --- |
| Tobu Tobu Girl / `tobu-tobu-girl` | Game Boy, original monochrome source build | Unknown | Developer [README](https://github.com/SimonLarsen/tobutobugirl#license) explicitly separates MIT code and CC-BY-4.0 game assets; [developer site](https://tangramgames.dk/tobutobugirl/) links source and future acquisition. Deluxe and packaging excluded. |
| Nova the Squirrel / `nova-the-squirrel` | NES source build, NES 2.0 caveat for level-editor slots | 1.0.6a | Tagged [README](https://github.com/NovaSquirrel/NovaTheSquirrel/blob/v1.0.6a/README.md#license) separates GPL-3.0-or-later code from CC-BY-NC-SA-4.0 assets and character-use restrictions; [release](https://github.com/NovaSquirrel/NovaTheSquirrel/releases/tag/v1.0.6a) establishes the version. |
| Super Tilt Bro / `super-tilt-bro` | NES RAINBOW networking and separate offline UNROM variants | Unknown | Repository [licence](https://github.com/sgadrat/super-tilt-bro/blob/master/LICENSE) is WTFPL version 2 (SPDX `WTFPL`); independent game-asset coverage remains Unknown. [README](https://github.com/sgadrat/super-tilt-bro#building) supplies variant and emulator caveats. |

All compatibility confidence is explicitly **unverified developer claim**.
No hardware, emulator or downloaded-package testing was performed. Recorded code
licences do not license downloadable assets. Independently stated game-asset
licences retain their conditions and scope. Blanket redistribution of a complete
downloaded package remains **Unknown** for all starter records; no licence here
authorizes automatic acquisition or redistribution.

The requested [Retro-Metro catalogue](https://www.retro-metro.xyz/homebrew/)
could not be retrieved through the research tool. Ordinary requests to the given
page and site returned retrieval errors. No crawling, account access, protection
bypass or import was attempted; its terms and evidence remain unassessed.
No live search or discovery adapters are provided for Retro-Metro, itch.io,
developer sites or GitHub repositories. The existing known-repository GitHub
Releases adapter remains available separately.

## Rights authority

A catalogue file is written by its curator. Every rights field in it, including
`redistribution: {known: {value: "permitted", ...}}`, is an **unverified
assertion**, never permission for EmuWiz to download, install, distribute or
republish anything. Loading accepts such a value so the claim and its evidence
can be reviewed, and returns it unchanged; nothing in this module acts on it,
and it has no acquisition path. The existing download and acquisition policies
remain authoritative. Any future acquisition feature must apply them to the
artifact and must not read local-catalogue rights as authority.

## Display text and platform claims

Titles and summaries are rejected when they contain control characters (C0,
DEL, C1: terminal escapes, CR/LF, tabs), bidirectional formatting characters,
zero-width space, word joiner, BOM, soft hyphen or line/paragraph separators.
Zero-width joiner and non-joiner, variation selectors and all ordinary
international text are accepted. The error names the field and code point.

A platform claim that is only a case or whitespace variant of a registered ID
(for example `nes` for `NES`) is refused at load time, because it would look
like that platform and silently fail every filter. Genuinely unregistered names
remain valid evidence and are listed by
`LocalCatalogueProvider::unregistered_platform_claims()` (the example prints
them to stderr); they never satisfy a platform filter.

## Extending providers

For another offline curator, assign a stable namespace and explicit local IDs,
then supply a catalogue in this schema. Do not repurpose another provider's ID
unless the curator actually records that provider's stable identity. Preserve
observations and put licence/platform evidence on its relevant project/build;
never propagate rights from code or between sibling builds. An empty claim list
and `Unknown` rights are valid metadata, not reasons to invent facts.

A future online discovery provider should implement `ProjectDiscoveryProvider`
separately from release enumeration, preserve provider identities, and use the
existing network/acquisition policies. This phase supplies no online crawler,
compatibility engine or installer. Any new serialized fields require a catalogue
schema change or an explicit extension contract; unknown fields currently fail
closed instead of silently losing evidence.
