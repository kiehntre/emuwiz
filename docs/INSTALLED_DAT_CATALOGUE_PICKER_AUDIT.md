# Installed DAT Catalogue Picker / 1G1R Audit

This audit describes the code at release base
`4fbaebc01973cefae4b6af28266c8310cd68aa34`. It is intentionally a design
and usability audit only. No GUI, DAT parsing, migration, verification,
rename, repair, or 1G1R election code was changed.

> **Implementation status (updated 2026-09-01).** The shared backend this
> audit recommends now exists, landed by `e8f30bf`
> *feat(dat): unify installed catalogue selection* in the core module
> `crates/archivefs-core/src/dat/catalogue_selection.rs`:
>
> - `CatalogueRef` — the typed, path-free logical reference this audit
>   argues for (`Local { source_id, member }` and
>   `ManagedCurrent { source_id, snapshot_sha256 }`).
> - `InstalledCatalogueSummary` + `list_installed_catalogues` — the
>   deterministic, de-duplicated, per-row fault-tolerant inventory across
>   the local registry and the managed MAME / Redump stores, with
>   `EvidenceValue` fields so unknowns stay honest.
> - `resolve_catalogue` / `resolve_catalogue_for_platform` — fail-closed
>   binding that **re-hashes** managed snapshot bytes (the gap called out
>   in §4 / §8), and returns typed ambiguity (`MultipleCandidates`) rather
>   than auto-choosing.
> - Thin adapters (`playing_library_request`, `to_dat_audit_request`,
>   `to_combined_dat_audit_source`, `to_library_scan_request`) so Build
>   Playing Library, Verify, and Repair can consume one reference.
>
> Still open exactly as this audit's step list anticipates: the **GUI
> picker wiring** for those three pages, first-class enumeration of the
> No-Intro / TOSEC pack stores (only their registered local projections
> appear today), stable folder-member references, and a persisted
> per-workflow "active catalogue" choice. The design rationale below is
> retained unchanged as the record of why the backend is shaped the way
> it is; where it says a shared selector "does not exist", read that as
> "did not exist at base `4fbaebc`".

## Executive finding

EmuWiz does not currently have one definition, registry, or selector for an
"installed catalogue". It has several compatible parsing and audit primitives,
but four different persistence/projection paths feed them:

- registered local file or folder sources in `dat_sources.toml`;
- No-Intro pack snapshots in app-owned storage, whose accepted DAT files are
  also projected into `dat_sources.toml`;
- imported TOSEC release packs in `tosec_release_packs.json`, whose selected
  classic DAT files are also projected into `dat_sources.toml`; and
- typed managed MAME and Redump sources in `managed_dat_sources.toml`, with
  validated current/previous immutable objects in `managed-dats`.

Consequently, the three user journeys do not share one picker:

- Build Playing Library (1G1R) requires a raw DAT path in a text field.
- Verify Games lets the user act on a registered local source row; its backend
  still receives the stored path. Managed catalogues are not offered by that
  same per-source Audit control.
- Identify & Rename does not ask for one catalogue. It automatically combines
  every enabled local source with every resolvable installed managed *game*
  snapshot and preserves agreement/disagreement as evidence.
- Repair Review has a separate picker populated from enabled entries in the
  local registry only. It does not include managed sources.

The smallest safe direction is one core, read-only catalogue inventory and
resolution abstraction, followed by thin adapters into the existing request
types. Selection must be by a typed catalogue reference, not a path. Paths
remain an implementation detail and can be shown under Technical details.

## 1. Current architecture

### Terms that are distinct in the current code

The code currently uses "source", "catalogue", and "snapshot" at different
granularities:

- A `DatSourceEntry` is one registered path. It may be a single file or a
  folder containing several DAT catalogues.
- A parsed `DatSource` in `dat/model.rs` is metadata for one DAT file: format,
  detected ecosystem, header name/description/version/author/homepage, counts,
  warnings, and packing policy.
- A managed snapshot is one immutable, content-addressed DAT object selected
  as current by a typed `ManagedDatState`.
- A TOSEC pack is a user-owned directory containing many inventoried DATs. Its
  enabled selection is stored as System + Category + Media groups.
- A No-Intro pack snapshot is one app-owned publication containing many
  separately validated DAT members.

There is therefore no universally correct existing synonym for "installed
catalogue". The UI currently calls each of the following installed in some
context:

| Storage path | What makes it present | What makes it usable |
| --- | --- | --- |
| Local registry | A `DatSourceEntry` exists | Its path still passes path policy and at least one DAT parses within limits |
| No-Intro snapshot | A complete published pack state and snapshot exist | Each member re-parses as No-Intro; the GUI additionally registers accepted files as local file sources |
| TOSEC pack | A `PersistedTosecPack` exists | The pack is available, a group is selected and applied, and the selected classic DAT validates and is registered locally |
| Managed source | Typed configuration exists | State matches the typed descriptor and its current non-symlink regular-file object resolves |

This distinction matters: a configured source, a present file, a last-valid
health record, and currently validated catalogue bytes are not equivalent.

### Local DAT source registry

`dat/sources/config.rs` durably stores `DatSourcesConfig` in
`dat_sources.toml`. `DatSourceRegistry` in `dat/sources/mod.rs` is the
in-memory registry for that file.

Each entry persists:

- stable source ID, display name, raw absolute path, and file/folder kind;
- typed ownership (`UserLocal` or `ImportedTosecReleasePack` in normal local
  registry use);
- enabled state, priority, optional platform assignment, free-text origin,
  and added time; and
- last validation health, time, detail, aggregate counts/formats, observed
  file size/mtime, and selected arcade revision metadata.

The registry can enumerate entries deterministically by priority and ID and
filter enabled entries by platform. An unassigned source participates in every
platform query. Multiple sources may be assigned to the same platform.

The registry enumerates *sources*, not necessarily catalogues. A folder entry
may contain many DAT files. `validate_dat_source` produces a bounded
`DatFileReport` per discovered file, including format, ecosystem, header name,
version, counts, diagnostics, or a parse failure. That per-file detail is kept
only for the current GUI session; the persisted health is aggregate. Folder
contents also cannot be given a reliable persisted stale/not-stale answer from
one directory fingerprint.

Local IDs are not content identities. `suggest_id` derives a slug from the
path's filename stem and adds a numeric suffix if needed. The path is persisted
and is the eventual parser input.

### Browser-assisted No-Intro pack import

`identity_source/no_intro/pack_import.rs` validates a user-supplied ZIP under
strict member and byte limits, parses every accepted DAT, and atomically
publishes an app-owned snapshot. Its state records the pack and snapshot
hashes plus, for every accepted member:

- member name;
- internally declared system name;
- conservatively detected `Headered`, `Headerless`, `Aftermarket`, `Bios`, or
  `Unknown` variant;
- upstream version;
- artifact SHA-256; and
- entry and ROM counts.

The pack state does not persist an import timestamp. After import,
`DatSourcesPageState::import_no_intro_pack` removes the earlier
browser-import projections and registers each accepted snapshot DAT as an
enabled local file source. Those projections get a new local `added_unix_seconds`
and the origin string `browser-assisted No-Intro pack import`, but the richer
variant/hash metadata remains in the separate No-Intro pack state rather than
the local entry.

`NoIntroSourceSelection` is an existing narrow selector. It parses and hashes
all enabled, platform-relevant local candidates and returns NotImported,
exactly one parsed No-Intro source, or Ambiguous. It is fail-closed and useful
precedent, but it is not a general installed-catalogue picker: it covers only
No-Intro, omits managed sources, performs nontrivial I/O, and expands folder
sources at resolution time.

### Imported TOSEC release packs

`dat/tosec_release_pack/mod.rs` stores imported packs separately in
`tosec_release_packs.json`. A `PersistedTosecPack` records:

- pack ID, root path, and import time;
- selected System + Category + Media groups; and
- an inventory of relative DAT path, raw catalogue name, projected system,
  category, media, classification confidence, and optional bounded SHA-256.

Importing a pack selects nothing. Applying an explicit selection safely
resolves and parses each selected classic DAT, records parsed TOSEC header name,
version, and artifact SHA-256 in provenance, then registers it in the local
registry with typed `ImportedTosecReleasePack` ownership. A stable generated
source ID uses pack identity plus relative path; it does not use an arbitrary
absolute path or silently replace another owner's entry.

Thus an enabled TOSEC catalogue is represented in both the pack registry and
the local source registry. The pack registry has the richer classification and
import time; the local projection is what ordinary audit consumers use.

### Managed MAME and Redump sources

`dat/managed_sources.rs` deliberately uses a separate
`managed_dat_sources.toml`. Its accepted configuration is closed and typed:
MAME software-list authoritative names and fixed Redump BIOS/game system enums,
each with Disabled or Manual update policy. Arbitrary providers and URLs
cannot be configured.

`ManagedDatSourceId` is a provider-scoped stable ID, for example a managed
provider plus authoritative source key. `ManagedDatState` binds it to a
validated current SHA-256 object and optional previous object and persists:

- upstream revision and retrieval/check times;
- SHA-256, ETag, and Last-Modified;
- parsed ecosystem and authoritative name;
- validation summary and last failure; and
- corresponding provenance for the retained previous snapshot.

Resolution verifies descriptor/state consistency and the backing object's safe
regular-file path before returning `ManagedDatReadOnlySource`. It does not
re-hash the object's current bytes merely to resolve that path; the hash was
established when the validated snapshot was published, and downstream parsing
will still reject malformed bytes. A shared selector resolver should add an
expected-hash check when binding a user choice to an immutable snapshot. MAME
software lists, Redump BIOS, and Redump game DATs currently have sibling
resolved types/functions rather than one erased installed-catalogue result. A
resolution error can also abort enumeration of that configured family instead
of yielding one common per-catalogue unavailable row.

Managed snapshots are not projected into the local registry. Identify & Rename
manually gathers them alongside local entries. Repair's local-only registry
load therefore cannot see them.

### Is there one canonical registry?

No. `dat_sources.toml` is the operational rendezvous for manually registered
files plus the local projections of No-Intro and selected TOSEC files. Managed
sources/state and TOSEC pack inventory have separate canonical stores, and the
No-Intro pack has separate app-owned state as well. Each store is canonical
for different facts.

The backend can enumerate every class, but only through separate APIs:

- `DatSourceRegistry::entries` / sorted filters for local source entries;
- No-Intro installed pack summary/load APIs for the current snapshot;
- `load_tosec_packs` for imported TOSEC inventories and selections; and
- the three managed resolve functions for MAME, Redump BIOS, and Redump games.

There is no backend API that projects these into one list of individual
catalogues, and no general `InstalledCatalogueSummary` or catalogue picker
model exists.

## 2. Current user journey

### Verify Games

The Verify Games page is also the DAT source management page. A user adds a
local DAT file/folder with a file dialog, imports No-Intro/TOSEC through their
specific workflows, or configures a managed source. A local source row shows
its ID, path, enabled/health state, platform assignment, Validate, Inspect, and
Audit actions. Audit chooses a library folder/file; it selects the catalogue by
the row's source ID and internally builds a `DatAuditRequest` with the stored
path and kind.

So Verify Games does **not** require the user to retype a DAT path for every
run, but local installation still begins by selecting a filesystem path and
the backend request remains path-based. Managed rows expose Check/Update/
Rollback rather than the same per-source Audit control.

### Identify & Rename / Quick Rename

The normal rename journey has no per-run catalogue picker. It states that all
enabled applicable evidence is used. `combined_audit_sources` gathers:

- every enabled local registry entry, including No-Intro and TOSEC
  projections; and
- every locally resolvable managed MAME software-list and Redump *game*
  current snapshot.

Managed BIOS sources are deliberately excluded. The combined audit retains
each agreeing source's provenance and does not collapse disagreement to a
first match. `build_rename_plan` consumes the completed audit outcome, not a
DAT path, and does not parse the catalogue again.

This is safe multi-evidence behavior, but it is a different choice model from
"use this catalogue." Enabled state acts as global participation policy.

### Build Playing Library / 1G1R

`playing_library_page.rs` owns `dat_path_draft`, renders it as an editable text
field plus Browse button, and does not load the DAT registry. The draft is
session state initialized empty; it is not a persisted catalogue reference.

Preview performs the following on the GUI action path:

1. construct a `PathBuf` from the text;
2. parse that one DAT with the standard limits;
3. derive its platform identity;
4. collect and hash source candidates against the parsed DAT; and
5. call `build_playing_library_plan`.

The core planner does **not** need a path. `PlayingLibraryRequest` accepts an
already parsed `ParsedDat`, verified matches, destination root, and policy.
Therefore a catalogue ID/reference can be resolved and parsed before the
existing planner without changing any grouping or election semantics.

Multiple installed DATs for one platform are possible, but this page never
enumerates them. Ambiguity is whatever file the user manually chose; no
installed-catalogue ambiguity is detected. A missing file gets an immediate
UI hint and parse failure is surfaced at preview. A changed, stale, or deleted
registered catalogue is not relevant because the page does not consult the
registry or its health at all.

1G1R region/language/revision/parent preferences are carried in
`PlayingLibraryPolicy` and page draft state. They are not bound to a catalogue
reference and are separate from the global/per-platform DAT matching policy in
`dat_sources.toml`.

### Repair Review

Repair Review has its own `ScanSetupState`. Opening its scan dialog reloads the
local DAT registry and lists every enabled `DatSourceEntry`; a card click stores
`selected_dat_id`. The card prominently shows display name and path. The user
does not type a raw DAT path, but starting the scan resolves the ID back to the
entry and creates a path-based `LibraryScanRequest`.

The picker does not include managed catalogues and does not project parsed
ecosystem, variant, version, imported date, or content hash. It can offer a
folder source as one choice even though that source may contain several
catalogues. The scan worker runs off the UI thread and surfaces parse/audit
failure.

Saved repair plans retain source ID, display name, and DAT path. Apply runs a
fresh authoritative scan and refuses a changed path, generation, proposal, or
filesystem identity before mutation. This re-proof behavior must remain
unchanged.

### Direct answers to the audit questions

1. An "installed catalogue" is not one type today; see the four definitions in
   section 1.
2. There are several canonical stores, each authoritative for different
   provenance and current-selection facts.
3. The backend can enumerate each store independently, but has no unified
   individual-catalogue enumeration.
4. Friendly metadata is available, but split across registry entries, parsed
   DAT headers, pack inventories, and managed state.
5. The GUI selects a local Verify source by row, all enabled sources for normal
   rename, a raw path for 1G1R, and a separate local-only source row for Repair.
6. 1G1R requires a raw filesystem path in its current GUI, although its core
   planner only requires parsed data.
7. Verify Games does not require per-run path typing after local registration;
   the implementation still passes a stored path.
8. Repair does not require path typing in the GUI; its separate local-only
   picker resolves a source ID to a stored path.
9. They use separate selection/request abstractions. They converge later on
   the parser/audit models, not at installed-catalogue selection.

## 3. Exact pain points

- 1G1R bypasses installed sources completely, so users must recognize and
  locate app-owned snapshot files or their original DAT files.
- The same accepted No-Intro/TOSEC catalogue can have rich metadata in a pack
  store and a poorer local projection. Consumers see only whichever half they
  load.
- Managed catalogues are first-class in combined rename but absent from Repair
  and the 1G1R path picker.
- A local folder is one registry row but potentially many individual
  catalogues. It cannot safely become one single-catalogue 1G1R choice without
  enumeration and an explicit member choice.
- Local validation persists aggregate health, not per-file parsed identity.
  Reopening the GUI cannot reconstruct a friendly folder catalogue list from
  persisted data alone.
- Local platform is a manual optional assignment. Parsed DAT platform identity
  exists (`identify_dat_source`) and fails closed as Resolved/Ambiguous/Unknown,
  but its module explicitly is not wired into validation/persistence generally.
- Local source display names and origin strings are presentation/free text,
  not provider trust evidence.
- Local source IDs are stable registration handles but are filename-stem
  derived, not artifact identities. Generic local DAT SHA-256 is not persisted.
- No universal active/current catalogue exists. `enabled` controls local
  participation, TOSEC has group selections, managed state has a current
  snapshot, and 1G1R/Repair choices are transient.
- Selection and availability are conflated in some surfaces: an enabled local
  entry can be missing or stale; a configured managed source can be
  uninstalled; an imported TOSEC pack can have a missing root.
- Ambiguity rules differ: the No-Intro identity selector refuses multiple
  candidates, combined rename keeps all evidence, Repair asks for one local
  source, and 1G1R accepts whichever path was entered.

## 4. Available metadata

| Friendly field | Available evidence | Important limitation |
| --- | --- | --- |
| Platform/system | Local manual assignment; No-Intro system name; TOSEC projected system and parsed header; fixed Redump system; MAME authoritative list key; parsed `DatPlatformIdentity` | Manual and derived identity must be labeled separately; Unknown/Ambiguous must remain visible |
| Ecosystem/provider | Parsed `DatEcosystem`; typed managed provider/expected ecosystem; typed TOSEC ownership; No-Intro import gate | A filename/display name/free-text origin is not provider authority |
| Variant | No-Intro importer derives Headered/Headerless/Aftermarket/BIOS/Unknown from internal header text; TOSEC has category/media | No general cross-provider variant field exists; unknown is common and must not be guessed |
| Imported/retrieved date | Local `added_unix_seconds`; TOSEC pack import time; managed retrieval time | No-Intro pack state has no import timestamp; its local projection's added time is only the projection time |
| Source/catalogue name | Local display name; parsed DAT header name/description; TOSEC raw catalogue name and header; managed authoritative name | Parsed local per-file names are not persisted in aggregate health |
| Revision/version | Parsed DAT version; No-Intro member version; TOSEC header version; managed upstream revision | Generic local version is available only after parsing; only arcade revisions have a persisted local health projection |
| Content hash | No-Intro member SHA-256; TOSEC optional inventory hash and validated import hash; managed current SHA-256 | Generic local registry/validation does not persist a content hash |
| Validation/provenance | Local health/time/size/mtime; parser diagnostics; No-Intro complete snapshot plus reparse; TOSEC typed ownership/import provenance; managed typed state/validation summary | Local health can be stale, folder staleness cannot be inferred reliably, and existence alone is not validation |
| Active/current | Local enabled/priority; TOSEC selected groups; managed current snapshot | There is no one persisted "use this catalogue" choice for 1G1R/Verify/Repair |

The beginner card proposed in the task is achievable for a validated No-Intro
source: platform/system and variant come from internal metadata, ecosystem is
No-Intro, source/projected added time can be shown with an accurate label, and
artifact hash is available from the pack state. For a generic local DAT,
platform/ecosystem/version require a bounded parse and SHA-256 requires new
runtime computation; these facts must not be synthesized from its filename.

## 5. Proposed shared selector model

Add one read-only core projection after the integration branch settles. Keep
inventory, selection, and resolution separate:

```rust
enum CatalogueRef {
    Local {
        source_id: String,
        member: Option<LocalCatalogueMemberRef>,
    },
    ManagedCurrent {
        source_id: ManagedDatSourceId,
        snapshot_sha256: String,
    },
}

struct InstalledCatalogueSummary {
    reference: CatalogueRef,
    display_name: String,
    platform: EvidenceValue<String>,
    ecosystem: EvidenceValue<DatEcosystem>,
    variant: EvidenceValue<CatalogueVariant>,
    revision: Option<String>,
    imported_or_retrieved_at: Option<u64>,
    content_sha256: Option<String>,
    provenance: CatalogueProvenance,
    availability: CatalogueAvailability,
    enabled: bool,
    capabilities: CatalogueCapabilities,
    technical_path: Option<PathBuf>,
}
```

The exact names are illustrative; the contracts are the important part:

- `CatalogueRef`, never `PathBuf`, is the GUI selection identity.
- A local ref includes the stable registry source ID. A folder member also
  needs a source-scoped member reference; a raw absolute child path alone is
  not identity. Until a stable member can be produced, a multi-DAT folder is
  shown as an aggregate source and is ineligible for single-catalogue use.
- A managed ref binds the provider-scoped source ID to the exact current
  snapshot SHA-256. A later update cannot silently change what an already-open
  choice meant.
- No-Intro and selected TOSEC files already have local projections, so v1 can
  use their local source IDs while enriching summaries from typed ownership
  and pack state. Inventory must deduplicate those projections rather than
  list a pack member twice.
- Every derived field carries evidence state such as Assigned, Confirmed,
  Ambiguous, Unknown, or Unavailable. A plain optional string cannot distinguish
  "not inspected" from "inspected and unknown."
- Availability is separate from enabled/selected state and should carry a
  plain-English reason.
- Capability flags state whether one summary is eligible for Verify,
  combined evidence, Repair, or single-catalogue 1G1R. BIOS/non-game sources
  are not silently offered as game catalogues.
- Technical path/hash/IDs are retained but are not the primary card label.

The corresponding resolver should accept `CatalogueRef`, revalidate the
backing artifact within current limits, parse it, and return an exact resolved
handle containing `ParsedDat` plus the legacy path/kind data needed by current
audit adapters. It must perform I/O off the UI thread. For content-addressed
sources it must verify the expected hash; for user-local files it must never
upgrade "registered" into "provider-trusted."

One inventory call should return every row, including unavailable/corrupt
ones, rather than fail the entire list because one managed state or local
source is broken. Ordering should be deterministic, for example canonical
platform display, ecosystem label, variant, display name, then typed reference.
No row is selected by the backend.

### Why no helper was added in this audit

A safe helper is not merely a projection of one persisted structure. It must
decide folder member identity, deduplicate No-Intro/TOSEC projections, reconcile
four stores, and turn family-level managed resolution errors into per-row
status. Implementing those decisions now would prematurely freeze the main
design and would need later GUI-consumer work in the exact integration files CC
is changing. This audit therefore remains research-only.

## 6. 1G1R integration

The first GUI consumer should be Build Playing Library because it is the only
flow that currently exposes a raw DAT path as the primary input.

Replace `dat_path_draft` with an optional `CatalogueRef` and an asynchronously
loaded list of summaries. Group or filter cards by confirmed/assigned platform,
but retain Unknown and Ambiguous groups rather than hiding them. A card should
show platform, ecosystem, variant when known, revision/import time, and health;
the path and hash belong under Technical details.

On Preview:

1. resolve the selected reference on a worker;
2. reject missing, changed/stale, corrupt, ambiguous-platform, or incompatible
   variant state with a visible reason;
3. reuse the resulting `ParsedDat` with
   `match_loose_files_against_dat`; and
4. call the existing `build_playing_library_plan` unchanged.

The planner already accepts parsed facts rather than a path, so neither its
parent/clone family grouping nor its region/language/revision/parent election
rules need modification. Region preferences remain independent user policy;
they are not properties of a catalogue.

When several usable catalogues describe one platform, show all of them and
require an explicit card selection. Do not rank one as "best" from local
priority, filename, newest-looking version, or provider name. Deleted local
files and invalid managed current snapshots remain visible but disabled with
their reason. Do not silently fall back to a retained previous managed snapshot.

## 7. Verify Games integration

Verify already comes closest to an ID-based picker for local sources. Reuse the
shared summary cards in place of rebuilding catalogue labels from
`DatSourceRowView`, while leaving source configuration/validation controls on
the DAT Sources page.

For an explicit one-catalogue audit, resolve the chosen `CatalogueRef`, then
adapt it into the existing `DatAuditRequest`. This makes managed current game
catalogues available through the same selection surface without inventing a
second parser or audit path.

The existing combined audit remains a separate, explicitly named mode. It can
adapt all enabled, usable summaries into `CombinedDatAuditSource`; its current
agreement/disagreement semantics are already fail-closed and should not be
replaced by picking a winner. BIOS catalogues remain excluded from game audit
and rename capabilities.

Validation and picker inventory are related but not identical. A registered
local source may stay visible when invalid, and Validate can repair its status;
it must not be selectable for an audit merely because its path exists.

## 8. Repair integration

Repair Review should consume the same summary list instead of independently
loading `dat_sources.toml`. Filter for the single-catalogue Repair capability,
then store the selected `CatalogueRef` in setup state. On Start, resolve it on
the existing worker and adapt it to `LibraryScanRequest`.

This preserves the current repair engine:

- `run_library_scan` still performs the authoritative DAT audit;
- `build_rename_plan` and the Repair adapter remain unchanged;
- the selected plan still records source/path provenance; and
- apply still performs its fresh scan and exact re-proof before mutation.

It also lets a validated managed game snapshot participate without copying it
into the local registry. Binding the reference to the managed snapshot hash
prevents an update between selection and scan from silently changing the
catalogue. Managed BIOS remains ineligible.

A multi-DAT folder must not be presented as one unqualified Repair catalogue.
Either enumerate and select one stable member, or explicitly label the source
as an aggregate and use only a consumer whose existing semantics support
multi-catalogue input.

## 9. Ambiguity and fail-closed rules

| Condition | Required behavior |
| --- | --- |
| Missing local file/folder | Keep the row visible as Missing; disable use; offer source management, never deletion or fallback automatically |
| Deleted managed snapshot/object | Mark current snapshot Unavailable; do not fall back to previous unless the user explicitly performs the existing rollback action |
| Corrupt/unreadable catalogue | Keep the row with the bounded parser/hash error; no capability that requires parsed evidence is enabled |
| Unknown platform | Label Unknown; never auto-associate it with the current library/platform. A platform-scoped v1 flow should require resolution/manual assignment before use |
| Ambiguous platform evidence | Show all candidate evidence in details and block platform-scoped use; never choose the first candidate |
| Several equally applicable catalogues | Select none by default. Require the user to choose one for single-catalogue flows; combined evidence may include all only under its existing explicit semantics |
| Headered/headerless or other variant mismatch | Block automatic use and state the incompatibility. Unknown variant is not equivalent to either variant |
| Local health is stale | Revalidate before use. A previous Valid record cannot authorize changed bytes |
| Folder source with several catalogues | Do not treat the folder as one catalogue for 1G1R/Repair; require an individual member or use an explicitly aggregate audit mode |
| Disabled source | Show it for management but do not include it in automatic/combined participation until explicitly enabled |
| Source merely exists | Describe it as user-supplied/registered, not trusted or current. Provider trust requires the persisted typed/parsed evidence that supports that label |

Priority must not silently resolve catalogue ambiguity. Current DAT policy
priority ranks already verified candidates in policy evaluation; it is not a
general authority to select one installed catalogue for 1G1R or Repair.

## 10. GUI recommendation after CC integration

After CC's integration settles, introduce one reusable catalogue-choice panel
fed entirely by the shared backend projection. A normal card can read:

```text
Nintendo Game Boy
No-Intro · Headerless
Imported 28 Aug 2026 · Ready
[Use this catalogue]

Technical details
  source id: ...
  revision: ...
  SHA-256: ...
  path: ...
```

Only show claims supported by the row's evidence. For example, use "Platform
assigned by you" for a local assignment and "Platform confirmed from DAT
metadata" for a strong parsed identity. Use "Added" rather than "Imported"
when only `added_unix_seconds` is known, and "Retrieved" for managed snapshots.

Recommended presentation rules:

- primary grouping: confirmed/assigned platform;
- primary text: platform, ecosystem/provider, known variant, revision/date,
  and availability;
- explicit status badges for Missing, Needs validation, Ambiguous, Unknown,
  and Ready;
- zero implicit selection, even when only one row is currently visible;
- selected state keyed by `CatalogueRef`, never list index or displayed path;
- raw path, IDs, hashes, validation detail, and evidence provenance under
  Technical details; and
- background loading/resolution with generation checks so a stale completion
  cannot replace a newer user choice.

The panel should be a presentation component over a core projection, not three
GUI-specific filesystem scanners.

## 11. Smallest implementation steps

1. Add a core `catalogue_selection` (name provisional) module with typed
   `CatalogueRef`, `InstalledCatalogueSummary`, availability/evidence states,
   deterministic ordering, and injected config/data roots for tests. No schema
   migration is needed.
2. Enumerate local file entries and managed current game snapshots first.
   Enrich No-Intro/TOSEC local projections from their typed ownership/pack
   state without listing them twice. Represent a local folder honestly as an
   aggregate until stable child references are defined.
3. Make enumeration per-row fault tolerant. Missing/corrupt entries produce
   unavailable summaries; they do not abort unrelated rows.
4. Add a resolver from `CatalogueRef` to a revalidated, bounded `ParsedDat`
   handle plus legacy source/path/kind data. Hash-check immutable snapshots and
   never weaken existing parser/path limits.
5. Test multiple catalogues, same-platform ecosystem/variant distinctions,
   missing/corrupt artifacts, deterministic order, non-path identity,
   ambiguity with no auto-selection, and unchanged planner output when the
   same parsed DAT is passed through the adapter.
6. Integrate Build Playing Library first by replacing its path draft with the
   shared reference. Keep parsing/matching off the UI thread and discard stale
   generations.
7. Reuse the same summaries/resolver in Verify and Repair. Adapt into
   `DatAuditRequest`, `CombinedDatAuditSource`, and `LibraryScanRequest`; do not
   change their audit, rename, re-proof, or apply semantics.
8. Consider persistence of a user's explicit per-workflow selection only
   after the picker works. Do not create a global "active catalogue" whose
   meaning would be unclear across platforms and workflows.

## 12. v1 priority recommendation

The v1 sequence should be:

1. shared core inventory/reference/resolver;
2. Build Playing Library picker integration;
3. Repair Review reuse, including eligible managed game snapshots; and
4. Verify Games presentation reuse while retaining both explicit single-source
   and existing all-enabled combined modes.

1G1R is first because it is the only normal flow that still makes a catalogue
filesystem path the user's primary selection mechanism, while its planner is
already cleanly decoupled from paths. Repair is second because a picker exists
but is local-only and metadata-poor. Verify is third because registered local
source rows already avoid per-run path typing and the combined rename path
already handles multi-source disagreement safely.

No election/1G1R semantics, DAT parsing semantics, archive safety limit,
managed trust contract, repair re-proof rule, or combined-audit disagreement
rule needs to change to deliver this v1.
