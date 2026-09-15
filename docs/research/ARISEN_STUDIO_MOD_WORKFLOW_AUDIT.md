# Arisen Studio Mod Workflow Audit

Research date: 2026-09-15  
Reference: [ohhsodead/arisen-studio](https://github.com/ohhsodead/arisen-studio), default branch `main`  
EmuWiz reference HEAD: `4ed476deccf5f1ee1452f0380981eb92783303d2`

This is an architectural audit only. No Arisen Studio source was copied, no
mod payload was downloaded, and no EmuWiz production code was changed.

## 1. Executive Summary

Arisen Studio is a Windows desktop manager aimed at browsing, downloading and
installing community content for PlayStation 3 and Xbox 360. Its useful product
idea is a single catalogue plus a guided path from a selected game/mod to a
device action. Its public documentation also shows a broad tool surface:
cheats, saves, homebrew, resources, packages, file management, console
commands, and device transfer.

The strongest EmuWiz lessons are catalogue filtering, explicit platform/game
context, visible install instructions, and a separate device-deployment
boundary. The risky parts are also clear: remote catalogue trust, archive and
destination handling, console-specific assumptions, and a backup prompt that
is described as recommended rather than demonstrated as an enforced
transaction. EmuWiz should retain its immutable-base, evidence-first and
shared-transaction foundations.

## 2. Project / Licence

The repository default branch is `main`; GitHub reports 566 commits at the time
of review. The repository contains a Visual Studio solution and describes a
Windows installation requiring .NET Framework 4.8. The dependency credits name
DevExpress, WebView2, FluentFTP, HtmlAgilityPack, Newtonsoft.Json, Tomlyn,
NLog, PS3Lib, xdevkit and webMAN MOD, which is consistent with a C#/.NET
desktop application ([repository README](https://github.com/ohhsodead/arisen-studio),
[credits section](https://github.com/ohhsodead/arisen-studio#credits--libraries)).

`LICENSE.md` contains the GNU Affero General Public License, version 3, and
the README states that modifications and commercial uses must remain
open-source. This is a copyleft/distribution constraint, not permission to
reuse source in EmuWiz. Any code or dependency reuse would need an independent
licence review; this audit does not reuse it.

The project’s code licence is distinct from:

- catalogue/database metadata, which the README says moved from a self-curated
  repository to a more secure server;
- mod, save, package, homebrew, image and other payload rights, which belong to
  their respective authors/rightsholders and are not granted by the AGPL;
- external hosting, screenshots, icons, translations, and third-party library
  terms.

The public pages do not establish a complete licence or provenance policy for
each database record or payload. EmuWiz should require that those terms be
recorded or linked per source before redistribution or bundling.

## 3. Architecture

The reviewed public material supports this subsystem map:

| Area | Arisen Studio evidence | EmuWiz implication |
| --- | --- | --- |
| Catalogue | Community-curated database of mods, saves, homebrew, resources, themes and packages; server-backed catalogue | Add a provider/index layer, not a second package model |
| Cheats | Over 1,000 runtime cheats and a separate Game Cheats page; changelog says cheats moved to GitHub hosting | Keep cheats separate from file-mod transactions |
| Game identity | Automatic PS3 region detection and remembered region are explicitly advertised | Feed only verified identity facts into matching |
| PS3 | webMAN/multiMAN/Rebug Toolbox integration, package manager, mounts and console manager | Future device adapter with explicit capability checks |
| Xbox 360 | XEX launcher, trainers, module loading and XBDM commands | Separate Xbox adapter; do not conflate with PS3 paths |
| Downloads | Local downloads, mod detail/download tabs, server-backed records | Download is a later trust-and-verification stage |
| Installation | One-click transfer to “appropriate file paths”; optional original-file backup prompt | Require an EmuWiz preview and transaction before writes |
| File manager | Local and console listings, upload/download/delete/rename | Useful UX reference, high-risk mutation surface |
| Device transfer | FTP is a stated capability; PS3 requires webMAN, multiMAN or Rebug Toolbox; Xbox requires RGH/JTAG and xbdm | Device deployment must be opt-in and capability-scoped |
| Updates | Official game updates and PSN package downloads are advertised; changelog records updater-library changes | Keep emulator/game updates outside the Mods package contract |
| Settings | Profile/connection setup, console selection and install preferences are evidenced by help/features | Store credentials and overwrite policy separately from mod metadata |

The public repository pages do not expose enough source detail to claim the
exact class or database schema behind each subsystem.

## 4. Mod Data Model

The public README and help pages prove these user-visible concepts: mod/game
details, creator, version, console/platform, region, category/type, status,
download files, installation instructions, and sometimes multiple downloadable
versions. The help page lists PS3 categories as Game Mods, Homebrew, Resources,
Packages and Game Saves; Xbox categories include Plugins and Game Saves. The
README also advertises themes and cheats.

The reviewed evidence does not prove that every record contains a cryptographic
checksum, archive format, destination manifest, exact title ID, or a formal
rollback transaction. URLs are visible in the separate database repository’s
JSON records, but that is evidence of links, not of authenticity or checksum
verification.

Useful minimum fields for EmuWiz are therefore:

`record_id`, platform, title identity candidates, region/revision, category,
display title, author, version, description, source URL, payload URL(s),
payload hash when supplied, archive/member manifest, destination intent,
instructions, licence/provenance, and compatibility confidence.

## 5. Game Identity

Arisen publicly claims automatic PS3 region detection and remembering supported
regions. It also requires console-specific environments: PS3 with webMAN,
multiMAN or Rebug Toolbox, and Xbox 360 with RGH/JTAG, DashLaunch and
`xbdm.xex` ([README requirements](https://github.com/ohhsodead/arisen-studio#requirements),
[help page](https://arisen.studio/help)).

The reviewed public material does not prove the exact implementation or field
precedence for PS3 Title IDs, Xbox Title IDs, Media IDs, TU versions or game
paths. Database URLs contain short game-like path keys, but those are not enough
to classify them as verified console identities.

Evidence ranking for an EmuWiz adapter should be:

1. verified content hash and parsed platform-native identity;
2. verified PS3 Title ID or Xbox Title ID/Media ID obtained from the selected
   installation;
3. verified region/revision/update evidence;
4. catalogue-declared identity;
5. README or filename hints.

This maps directly to EmuWiz `VerifiedIdentityFact` and selected-game evidence.
Catalogue declarations must remain candidate evidence until independently
verified.

## 6. Mod Types

Documented categories and tools include game mods, homebrew, resources,
packages, game saves, Xbox plugins, themes, runtime cheats/trainers, official
updates, PSN packages, and console/file-management utilities. The public pages
do not establish a uniform distinction between replacement files, executable
patches, texture packs, DLC, scripts, or trainers inside every record.

EmuWiz should preserve these as separate operation families. A file replacement
is not a cheat; a save is not a ROM patch; a console command is not a package
payload; and an executable/tool is never an automatically trusted mod.

## 7. Download Model

The application advertises a regularly updated database, local downloads, a
download folder, and mod-detail download tabs. The public database repository
contains records with URLs such as `https://db.arisen.studio/...`, while the
README says the active database moved to a server. The evidence therefore
supports centrally served metadata and externally reachable payload URLs.

The reviewed material does not prove retry rules, mirror selection, per-payload
hashes, signature verification, or a complete dead-link policy. The help page
does tell users to obtain the application only from official GitHub or
PSX-Place releases, which is release-source guidance rather than mod-payload
verification.

EmuWiz should keep discovery, download, verification and inspection as
separate states. A URL is not proof of content identity, and an HTTP success is
not proof of safe or correct payload bytes.

## 8. Installation Model

The documented journey is: browse a filtered library, open mod details, choose
a downloadable version/file, then select Install or Download. Install transfers
files to the “appropriate file paths” on the connected console. The help page
mentions prompts to back up original files before installation, and describes
uninstall as restoring those originals.

PS3 package management, mounting and webMAN commands are also advertised. Xbox
workflows include XEX launching, module loading and XBDM commands. These are
device operations rather than portable local-package semantics.

The public evidence does not prove atomic staging, path confinement, symlink
handling, per-file identity revalidation, rollback journals, or whether scripts
are ever launched. Those are required review points, not assumptions about the
implementation.

## 9. Backup / Rollback

Backup/restore of original PS3 game files and backup/restore of configuration
files are explicitly advertised. The help page says an install may ask whether
to back up originals and calls that recommended. This demonstrates a useful
restore concept, but does not prove an immutable, journaled transaction or
ownership-checked uninstall.

EmuWiz already has the stronger foundation: shared transaction/rollback
infrastructure, preview/apply separation, destination safety, and owned
derived outputs. Future device deployment should record the exact remote path,
pre-write identity, backup receipt, post-write identity, and ownership before
allowing removal.

## 10. PS3 Workflow

The documented flow is:

`browse/filter → open mod details → choose file/version → connect to PS3 → transfer to the declared destination → optionally back up originals → uninstall/restore later`.

The connection prerequisite is not generic PS3 support: the help page requires
webMAN, multiMAN or Rebug Toolbox to be running/available. The README also
mentions mounting, package management, console information, webMAN commands and
boot-plugin backup/restore.

Useful concept: show platform capability and required console component before
the install action. Dangerous assumption to avoid: “connected” does not mean
the remote game, region, path or writable target has been verified.

## 11. Xbox 360 Workflow

The documented Xbox flow is similarly device-oriented:

`select/filter Xbox content → choose a mod/trainer/save → connect to an RGH/JTAG console → use FTP/XBDM or related console facilities → transfer or invoke the selected operation`.

The README names DashLaunch, `xbdm.xex` as plugin #1, and optionally JRPC2 as
requirements. It also advertises XEX launching, trainer support, module loading,
XUID spoofing, Neighborhood editing and `launch.ini` backup/restore.

The public evidence does not prove exact Title ID/Media ID/TU matching or the
precise remote filesystem implementation. EmuWiz should therefore treat those
as adapter requirements to verify, not fields to guess from filenames.

## 12. Security Findings

### Safe patterns or useful boundaries

- Explicit platform and category filters reduce accidental cross-platform use.
- The product separates local download from direct console installation.
- It documents required console components instead of claiming stock-console
  support.
- Backup/restore is visible to users and uninstall is part of the workflow.
- The project warns users to obtain the application from official release
  locations ([help FAQ](https://arisen.studio/help)).

### Requires review

- Remote catalogue records and payload URLs are trusted enough to drive a
  one-click action, but public evidence does not establish signatures or hashes.
- “Appropriate file paths” and console FTP transfer need path and destination
  identity checks.
- Backup is described as a prompt/recommendation; enforcement and ownership are
  not proven.
- The broad file manager can delete, rename and upload, so it needs a separate
  authorization boundary.
- Cheats, trainers, plugins, packages and tools may have execution or runtime
  effects that differ from ordinary data files.
- Credentials, IDPS/PSID, XUID and console-management features require careful
  secret handling and audit logging.

### Unsafe for EmuWiz without independent controls

- Executing package-provided scripts/installers.
- Applying remote archive contents directly to a game or emulator directory.
- Treating filenames, URL paths or README claims as identity proof.
- Removing a remote file or local backup without an ownership receipt and
  post-change identity check.

These are engineering classifications for EmuWiz design, not claims that every
Arisen implementation is defective.

## 13. UX Findings

Arisen makes discovery easy: a single library, filters for console/type/region,
sortable results, details, versions and a direct Install/Download choice. It
also explains prerequisites and exposes a file manager and connection setup.
That is a strong novice-oriented “what can I use?” journey.

The confusing boundary is between catalogue confidence and operational safety.
The public flow puts “install” close to “download”, while exact destination
identity, backup guarantees, version compatibility and rollback details are
not prominent in the evidence reviewed. A novice can understand the action but
not necessarily the blast radius.

EmuWiz should retain the easy browse/review flow while putting evidence,
compatibility state, exact destination, backup/rollback ownership and
“original remains unchanged” beside the confirmation action.

## 14. EmuWiz Mapping

| Arisen concept | EmuWiz existing primitive | Gap | Recommended approach |
| --- | --- | --- | --- |
| Title/region-oriented game selection | `VerifiedIdentityFact`, selected-game evidence, DAT/content identity | PS3/Xbox native identity adapters are not yet a Mods contract | Add bounded PS3 Title ID and Xbox Title ID/Media ID evidence adapters |
| Catalogue filters and mod details | typed local mod metadata, archived package inspection | remote catalogue/provider model for mods | Add read-only provider records with provenance and explicit confidence |
| Archive download then install | safe ZIP/7z/RAR inspection and standalone patch inspection | no remote download pipeline and no archive-to-install bridge | Keep download, inspect, match and apply as separate reviewed stages |
| Backup/restore originals | shared transaction/rollback and owned derived output | remote-device receipts | Extend transaction vocabulary for remote identity and backup ownership |
| FTP console transfer | no device deployment adapter | PS3/Xbox transport and capability checks | Future opt-in adapter; never let FTP be generic filesystem authority |
| Cheats/trainers/plugins | patch manager and adapter-specific cheat modules | console-native runtime semantics | Keep separate from file-mod/package identity and transactions |
| File manager | no equivalent broad destructive surface | not needed for core Mods | Do not add a general-purpose delete/rename console browser as a prerequisite |
| Multiple profiles/consoles | emulator/install provenance concepts | device profile storage | Add capability-scoped device profiles only when a deployment adapter exists |

## 15. Legal / Distribution Boundary

The AGPL governs the Arisen code repository; it does not grant rights to game
files, mods, saves, patches, screenshots, database records, or linked hosts.
The public pages establish that community members contribute content and that
the database is curated, but do not provide a complete per-record rights model.

For EmuWiz, distinguish:

- indexing descriptive metadata and linking to a user-selected external source;
- downloading a user-selected payload under that source’s terms;
- redistributing or mirroring payload bytes;
- bundling copyrighted games, patches, saves or mod assets;
- distributing cheats, trainers, plugins or executable tools;
- storing credentials or device identifiers.

The first may be feasible with careful provenance and source terms; the others
need product-policy and legal review for the particular content and
jurisdiction. This document is not legal advice.

## 16. Recommended EmuWiz Roadmap

1. Add read-only PS3/Xbox identity evidence adapters, starting with Title ID
   and platform-native revision/version facts, and feed them into the existing
   compatibility precedence.
2. Define a provider-neutral mod catalogue record containing provenance, source
   URL, content hash when supplied, category, compatibility evidence and
   explicit destination intent.
3. Add a review projection that shows evidence, warnings, exact destination,
   backup/rollback ownership and immutable-base/remote-write consequences
   before any future deployment.
4. Build device deployment as separate PS3/Xbox adapters with capability checks,
   path confinement, pre-write revalidation and shared transaction receipts.
5. Add external-source linking and user-selected downloads only after source
   trust, archive verification, licensing and credential policies are agreed.

## 17. Things We Should NOT Copy

- One-click remote writes without a prominently verified target and destination.
- Filename or catalogue-path matching as proof of game identity.
- Optional/recommended backups without an ownership-bound restore receipt.
- A broad file manager as the implicit authority for Mods.
- Automatic execution of scripts, trainers, plugins or package installers.
- Treating the code licence as a licence for database records or mod payloads.
- Combining PS3 and Xbox workflows behind one untyped filesystem operation.

## Sources consulted

- [Arisen Studio repository README](https://github.com/ohhsodead/arisen-studio)
- [Arisen Studio licence](https://github.com/ohhsodead/arisen-studio/blob/main/LICENSE.md)
- [Arisen Studio help](https://arisen.studio/help)
- [Arisen Studio database site](https://db.arisen.studio/)
- [Arisen Studio database example](https://github.com/ohhsodead/arisen-studio-database/blob/main/game-saves.json)
- [Arisen Studio changelog](https://github.com/ohhsodead/arisen-studio/blob/main/CHANGELOG.md)

ARISEN STUDIO MOD WORKFLOW AUDIT READY
