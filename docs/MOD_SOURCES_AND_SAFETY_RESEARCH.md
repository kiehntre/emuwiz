# Mod sources, downloads, and safety research

> **Research snapshot** — This document records research and design reasoning
> for discovering, downloading, and safely installing game mods in EmuWiz. It
> is research only: no code, Cargo file, schema, or migration was changed to
> produce it, and no provider integration is authorized by it. For current
> capability documentation see the [README](../README.md), the [Cheats & Mods
> safety model](CHEATS_MODS_SAFETY.md), and the [roadmap](../ROADMAP.md).
>
> **Repo state audited:** `4fbaebc01973cefae4b6af28266c8310cd68aa34`, clean
> tree.
>
> **Claim tags used throughout:** `[FACT]` = documented fact, source cited ·
> `[CODE]` = conclusion from reading this repo's source, file cited ·
> `[INFERENCE]` = reasoned but not directly sourced · `[COMMUNITY]` =
> community wiki/forum knowledge, not authoritative · `[UNVERIFIED]` = could
> not be confirmed during research; do not build on it without checking.
>
> **Legal note:** Section 3 is general information about legal risk, not
> legal advice. Where no certainty exists, the document says so and
> recommends the posture that remains safest under uncertainty.

---

## 1. Executive recommendation

**Recommendation in one sentence:** EmuWiz should treat mods as first-class
candidates discovered from named providers, downloaded **only** from the
original upstream host on an explicit per-request user action, cached in an
immutable content-addressed snapshot, applied **only** when the mod is a
declarative patch format and the target game's identity is exactly verified,
applied into a **derived copy** rather than the original source artifact,
with full attribution always shown and executable installers **never
executed** — surfaced instead as a deep link back to the original project
page.

The supporting findings:

1. **The architecture already exists.** EmuWiz already has the provider →
   normalized record → classification → preview → confirm → atomic apply →
   journal → rollback pipeline for cheats, a shared transaction engine, an
   immutable content-addressed source cache with provenance and SHA-256
   recording, and a three-state trust model (`Trusted` / `Unverified` /
   `Blocked`) `[CODE: docs/SHARED_SAFE_APPLY_ROLLBACK.md,
   docs/RETROARCH_CHEAT_SOURCES.md, docs/CHEATS_MODS_SAFETY.md]`. Mod support
   should extend this tree, not build a parallel one.

2. **The safest legal posture is conduit, not publisher.** Linking to the
   original mod page and fetching bytes from the original host at the user's
   explicit request is the model every mainstream mod manager already uses
   (Vortex, r2modman/Thunderstore, Steam ROM Manager). No surveyed host
   grants third parties a redistribution licence, so mirroring or bundling
   mod files would create direct and contributory infringement exposure with
   zero offsetting benefit `[FACT: host ToS/permission-model research, §3]`.

3. **Not all mod formats are equal.** A small set of formats — IPS, BPS, and
   (with more care) xdelta/VCDIFF — are fully specified, declarative,
   statically parseable, hashable, and carry no executable semantics. They
   are the only formats recommended for automatic apply in Phase 1.
   Everything else ranges from preview-only to refuse `[INFERENCE from
   format specs, §4]`.

4. **Compatibility must be proven, never guessed.** Existing EmuWiz matching
   vocabularies (`CheatMatchConfidence`, `PreviewMatchStrength`,
   `IdentityConfidence`) already express exactly the tiers needed. A mod is
   installed automatically only at the top tier; anything weaker stays
   preview-only `[CODE: crates/archivefs-core/src/game_identity.rs,
   crates/archivefs-core/src/patch_manager/shared_preview.rs, §6]`.

5. **Preservation-first rules remain authoritative.** The original archive
   or disc image is never modified in place. ROM-hack patching in particular
   is inherently a *derived-copy* operation: patch bytes are applied to a
   copy, producing a new artifact alongside the untouched original `[CODE:
   docs/CHEATS_MODS_SAFETY.md "EmuWiz never silently deletes, rewrites, or
   sanitizes the user's original source", §8]`.

6. **Phase 1 should be small and boring:** GitHub-hosted declarative patch
   files (IPS/BPS) for hash-identified loose-ROM platforms, reusing the
   proven libretro-database retrieval pattern (commit-pinned, immutable
   archive, hash-checked), plus GameBanana deep-links for everything else.
   No scraping, no mirroring, no installers, no new execution surfaces
   `[INFERENCE, §12]`.

---

## 2. Source/provider comparison table

Research current as of September 2026. Tags mark evidence strength; see the
header for tag meanings.

| Provider | Official API | Anonymous access | Auth | Rate limits | Machine metadata | Direct downloads | Hashes | Licence metadata | Redistribution | Stable IDs | Recommended role |
|---|---|---|---|---|---|---|---|---|---|---|---|
| **GitHub** | Yes — REST v3, GraphQL, releases `[FACT]` | Yes | Optional PAT | 60 req/h unauthenticated; 5,000 req/h authenticated `[FACT]` | Excellent (releases, commits, licence field) | Yes — commit-pinned immutable archives, release assets | Archive SHA-256 computable; asset digests on releases | Licence file/API field commonly present | Per-repo licence; no blanket right to rehost | Excellent — commit SHAs, tags | **Primary Phase 1 download source.** Already the authoritative pattern (`libretro-database`, `xenia-canary/game-patches`) `[CODE: docs/RETROARCH_CHEAT_SOURCES.md, docs/XENIA_PROVIDER.md]` |
| **GitLab** | Yes — REST v4 `[FACT]` | Yes | Optional token | Authenticated limits verified only from a Dec 2023 snapshot; current values `[UNVERIFIED]` | Good | Yes — commit-pinned archives | Same approach as GitHub | Licence field present | Per-repo licence | Good | Secondary; same architecture as GitHub, lower ecosystem priority |
| **GameBanana** | Yes — `apiv11` JSON API `[FACT]` | Yes | None observed | Published limits `[UNVERIFIED]` | Good — submission/file/author/licence fields | Yes, host-controlled (stable but not commit-immutable) | **MD5 per file available** `[FACT]` | Per-submission credit/licence fields commonly present | Not granted to third parties | Good — numeric submission IDs | **Phase 2 download source** (texture packs, per-game mods); **Phase 1 deep-links only** |
| **Nexus Mods** | Yes — `api.nexusmods.com` `[FACT]` | No | **API key required** `[FACT]` | Tight daily/hourly throttling; revocation for abuse `[SECONDARY]`; exact numbers `[UNVERIFIED]` | Excellent | Restricted; automated bulk downloading against ToS `[SECONDARY]` | MD5s exposed for some files | Per-mod author permission system | **Explicitly author-permissioned** | Good — mod/file IDs | **Deep-link only, all phases.** No programmatic downloads recommended. |
| **ModDB** | No general API; RSS/JSON feeds exist `[FACT]` | Yes (feeds) | None | Unpublished | RSS feed metadata only | Host-controlled, not immutable | Not exposed | Not exposed | ToS grants DBolical a distribution licence for uploads, not third parties `[SECONDARY]` | Good — numeric mod IDs | **Deep-link only**; optional feed-based metadata later, with attribution (ModDB explicitly encourages feed use with attribution `[FACT]`) |
| **PCGamingWiki** | Yes — MediaWiki API `[FACT]` | Yes | None | Standard MediaWiki limits | Excellent (wiki text, links) | N/A — not a download host | N/A | **CC BY-NC-SA** for wiki text `[FACT]` | Text under CC BY-NC-SA; patch files are separate per-project | Good — page revisions | **Metadata/link hub only.** Attribution mandatory if wiki text is displayed. |
| **Romhacking.net** | No public API `[FACT: absence of known API]` | Site behind Cloudflare; automated fetches returned 403 during research | N/A | N/A | None | Downloads reported available post-archival, **not confirmed** this session `[UNVERIFIED]` | None | None | "No rehost without author permission" is community norm `[COMMUNITY]`; primary ToS text not retrievable `[UNVERIFIED]` | Numeric hack IDs in stable `/hacks/<id>` URLs | **Read-only archive since 1 August 2024** `[FACT: Wikipedia/PC Gamer/Polygon citing the RHDN announcement]`. **Deep-link only.** |
| **Romhack.ing (RHDI) / Romhacking.com (RHDC)** | No known API; sites are JS-only `[FACT: absence observed]` | Human-browsable | Registration for some features | Unpublished | None | Site-hosted | None | Per-project, shown on site | Not granted to third parties `[COMMUNITY]` | Good | **Deferred.** Successor ecosystem is real (RHDI alpha Aug 2024; public registration Mar 2025 `[FACT: romhack.ing/help/about via Wikipedia]`) but has no machine interface and no reviewed policy. |
| **Internet Archive** | Yes — advancedsearch + metadata APIs `[FACT]` | Yes | None for public items | Generous but enforced | Excellent — file lists with MD5/SHA1/CRC32 in `_files.xml` `[FACT]` | Yes, stable per-file URLs | **MD5/SHA1/CRC32 exposed** `[FACT]` | Per-item, often unspecified | Per-item licence only; RHDN-dump redistribution terms `[UNVERIFIED]` | Excellent — item IDs immutable | **Provenance/reference source** for the RHDN archival corpus; candidate read-only metadata provider in Phase 2+. |
| **BSFree / GameHacking.org / libretro-database / xenia-canary/game-patches / RPCS3 patch repo** | Yes — already reviewed and integrated or directly integrable `[CODE]` | Yes | None | Reviewed, bounded (existing adapters) | Parsed natively | Commit-pinned (GitHub-hosted ones) | Recorded in manifests `[CODE: docs/RETROARCH_CHEAT_SOURCES.md]` | Recorded | Reviewed for catalogue use only | Per-entry / per-commit / per-serial | Already integrated or **Phase 2 candidates** — these are the pattern to generalize, not new risk |
| **PPSSPP community cheat.db mirrors** | None | Varies | None | Unpublished | None | Unstable provenance | Not systematic | Not systematic | Unclear | Weak | **Deferred** — poor provenance contradicts the Trusted-source model |
| **Discord/Dropbox patch hubs** | None | N/A | N/A | N/A | None | Unstable, unverifiable | None | Rarely | Rarely granted | None | **Never automated.** Deep-link only. |

**Key takeaways:**

- Only **GitHub/GitLab** currently satisfy every requirement of EmuWiz's
  existing retrieval policy: anonymous access, immutable commit-pinned
  archives, digestable content, licence metadata `[FACT]`.
- **GameBanana** is the strongest general mod host by machine interface
  (anonymous JSON, per-file MD5s, author/licence fields) and is the natural
  Phase 2 download provider `[FACT]`.
- **Nexus, ModDB, RHDN, RHDI/RHDC** should be deep-link-only in every phase:
  Nexus because of its API-key/throttling/author-permission model, the others
  because there is no machine interface or no reviewed redistribution policy
  `[INFERENCE from the table]`.
- **PCGamingWiki** is valuable as a discovery/attribution hub, not a file
  source, and its CC BY-NC-SA text carries real attribution obligations
  `[FACT]`.

---

## 3. Legal/distribution risk model

**This is general information about legal risk, not legal advice, and it
asserts no legal certainty.** The purpose is to choose the product posture
that minimizes risk *and* respects creators, which for EmuWiz is also the
posture most consistent with its preservation-first rules
`[CODE: docs/CHEATS_MODS_USER_POLICY.md]`.

DMCA safe harbor (17 U.S.C. §512) is a framework for *service providers*
storing or transmitting user content. A desktop client like EmuWiz is not a
host; what matters is whether EmuWiz itself copies, caches, or redistributes
content, and how user-directed each action is `[FACT: legal framework
research]`. The eight postures, from safest to most dangerous:

| # | Posture | Risk | Assessment |
|---|---|---|---|
| 1 | **Link/open the original mod page** | **Lowest** | EmuWiz makes no copy. Linking to publicly available content is generally not direct infringement; the app neither curates infringing files nor brokers them `[INFERENCE from framework; mainstream mod managers do exactly this]`. EmuWiz can do this freely and should do it always. |
| 2 | **Download directly from the original host on explicit user request** | **Low** | The user initiates; EmuWiz is a conduit to the same host the user could have used in a browser. This is the Vortex / r2modman / Steam ROM Manager model. Residual risk: automated access must respect each host's terms and rate limits (§2). Acceptable with reviewed, allowlisted hosts `[INFERENCE]`. |
| 3 | **Temporarily cache a downloaded mod** | **Low** | Transient caching of a user-requested fetch for functionality is standard practice. EmuWiz's immutable content-addressed snapshot model already records provenance and digests for exactly this purpose `[CODE: docs/RETROARCH_CHEAT_SOURCES.md]`. Keep retention rules honest and user-visible. |
| 4 | **Permanently mirror/rehost mod files** | **High** | Direct + contributory infringement exposure, plus breach of every host's terms — no surveyed host grants third parties a redistribution licence `[FACT: §2]`. **Never do this.** |
| 5 | **Ship mod files inside EmuWiz** | **High** | Same as #4 plus the project becomes a distributor of third-party content. The only safe exception is content EmuWiz's own licence permits (e.g., its own code, or permissively-licensed data it authored). **Never bundle third-party mods.** |
| 6 | **Mods containing only original patch data** (IPS/BPS/xdelta deltas, PNACH text, TOML/YAML patches) | **Lowest content risk** | A delta patch contains no copyrighted game bytes — it is the mod author's own expression. This is why the ROM-hacking community distributes patches rather than pre-patched images `[COMMUNITY]`. Caveat: the *patch* is still the author's copyrighted work — the conduit posture (#1–3) still applies to it. This is the recommended Phase 1 class. |
| 7 | **Mods containing copyrighted game assets/content** (pre-patched ROMs, full texture rips redistributed as packs, music replacements) | **High** | Downloading or caching these makes EmuWiz a party to reproducing someone else's game content, unlike #6. Texture packs are a grey zone: some are original art (low risk), some are modified rips of the game's textures (high risk) — and EmuWiz usually cannot tell which `[INFERENCE]`. **Refuse at download time; deep-link to the project page instead** and let the user obtain files themselves. |
| 8 | **Mods requiring user-owned original game files** (deltas applied to the user's dump) | **Low** | This is the correct architecture, not merely a mitigation: the patch contains only original data, and the user supplies the game. The user owns the ROM-acquisition question; the tool never touches it `[COMMUNITY + INFERENCE]`. EmuWiz's preservation rules already require user-owned sources. |

**Product policy (safest realistic posture):**

- **Always**: deep-link to the original project page; show author, source,
  and licence/attribution metadata; never obscure where content came from.
- **Approved with review**: direct downloads from *allowlisted reviewed
  hosts* on explicit per-request user action (posture #2), cached
  content-addressed (#3), restricted to declarative patch formats (#6)
  targeting user-owned originals (#8).
- **Never**: mirroring (#4), bundling (#5), accepting complete copyrighted
  game images or asset-rip archives (#7), or circumventing any host's access
  controls (login walls, Cloudflare challenges, paywalls) `[CODE: the
  existing policy "must not be used to bypass copy protection, licensing
  systems, access controls, or other technical protections" extends naturally
  to host access controls]`.
- **Uncertainty rule**: where a host's terms are unverifiable (RHDN today,
  RHDI/RHDC), the answer is deep-links only — never automated access. Lack of
  verified permission is treated as absence of permission.

---

## 4. Mod format matrix

Legend: **Preview** = can EmuWiz statically show what it does? · **Hash** = is
the file safely hashable/content-addressable? · **Transactional** = can apply
go through the shared transaction engine? · **External exe** = does correct
use require running a foreign program? · **Exact rollback** = can undo be
byte-exact? · **Compat provable** = can compatibility with a specific game be
established from identity evidence?

| Format/type | Preview | Hash | Transactional | External exe | Exact rollback | Compat provable | Notes |
|---|---|---|---|---|---|---|---|
| **IPS** | Yes (byte-level delta replays statically) | Yes | Yes | No | Yes (derived copy; original untouched) | Only via metadata — IPS carries **no** target identity | Fully specified, trivial parser, bounded size. Target ROM identity must come from provider metadata, never from the file itself `[FACT: format spec]`. |
| **IPS32** | Yes | Yes | Yes | No | Yes | Same as IPS | Extended-IPS variant ("EOF" + `!` extension); same trust profile `[FACT]`. |
| **BPS** | Yes | Yes | Yes | No | Yes | **Best-in-class: embeds source & target CRC32 + sizes** | BPS stores source-checksum, target-checksum, and patch checksum — compatibility with a specific ROM is *provable* by checksum before apply `[FACT: format spec]`. Phase 1 priority. |
| **UPS** | Yes | Yes | Yes | No | Yes | Embeds CRC32 of source/target | Older uniform patch system, largely superseded by BPS; same security profile `[FACT]`. |
| **xdelta3 / VCDIFF** | Yes (parseable delta) | Yes | Yes | No (if EmuWiz implements the decoder) | Yes | No embedded identity; needs external metadata | Requires a VCDIFF decoder; window settings can raise resource use — needs the same bounded-decode limits as archives `[INFERENCE]`. Phase 2. |
| **PPF** (PlayStation Patch Format) | Mostly (binary delta; optional embedded undo data) | Yes | Yes | No | Yes (embedded undo, else derived-copy rollback) | No embedded identity; PS1-image-adjacent | Historically PS1-scene; variants describe image sectors — needs careful bounds `[COMMUNITY]`. Phase 2–3. |
| **ASM / source patch** | Partially (reviewable text, not executable) | Yes | No — requires a build toolchain | **Yes** (assembler/compiler) | No | N/A | EmuWiz can *display* it with attribution but must never build or apply it. Preview-only or deep-link `[CODE: docs/SHARED_SAFE_APPLY_ROLLBACK.md "No path launches a script, binary, emulator, or downloaded installer"]`. |
| **Executable patcher** (`.exe` patchers, installer wizards) | No | Yes (hash only) | **No** | **Yes — by design** | No | Rarely | **Refuse automatic handling entirely.** Never executed (existing rule); offered only as a deep link with a plain-English explanation `[CODE: docs/CHEATS_MODS_SAFETY.md "Executables, scripts, installers... are never launched"]`. |
| **Texture pack** (emulator hierarchies, e.g., Dolphin `Load/Textures/<GAMEID>/`) | Yes (file inventory + image metadata) | Yes (per-file + pack manifest digest) | Yes — **already implemented** for Dolphin via manifest-backed packs `[CODE: crates/archivefs-core/src/patch_manager/dolphin_texture_mod.rs]` | No | Yes (replacements under backup/journal) | Game ID directory + manifest | Content provenance is the §3 posture #7 grey zone — the *mechanism* is safe, the *source* needs care. |
| **Replacement assets** (audio, models, fonts) | Partially | Yes | Case-by-case | No | With backups | Weak | Same provenance grey zone; format-specific adapters only. |
| **Widescreen / 60 FPS patches** | As delivered: usually PNACH/Gecko/cheat-DB entries or ASM | Per container | Via the relevant emulator adapter | Depends on container | Via adapter | **Strong** — these ecosystems key patches by serial/Game ID | Widescreen/60fps communities publish per-game patch text keyed to console identity — an excellent fit for the existing GameHacking.org-style pipeline `[COMMUNITY + CODE]`. |
| **PCSX2 PNACH patches** | Yes (plaintext `patch=` directives) | Yes | Yes — existing PCSX2 install path `[CODE: docs/PCSX2_CHEAT_ADAPTER.md]` | No | Yes | **Exact** — `0E7F91DA.pnach` filename + verified executable CRC | Already supported as cheats; the same adapter carries non-cheat patches. |
| **Dolphin Gecko / Action Replay / Riivolution** | Gecko/AR: yes. Riivolution XML: yes (declarative file-replacement manifest) | Yes | Gecko/AR: existing path. Riivolution: transactional-with-backups (replaces/adds whole files) | No | Yes for Gecko/AR; Riivolution via backups | Gecko/AR: exact Game ID. Riivolution: disc identity + file layout | Riivolution is a Phase 3 candidate: declarative, but its file replacements can include copyrighted assets — §3 rules apply per file. |
| **RPCS3 `patch.yml` entries** | Yes (YAML) | Yes | Yes (writes into RPCS3 patch-manager storage) | No | Yes | **Exact** — keyed by PS3 serial | Strong Phase 2 candidate; mirrors the Xenia TOML adapter shape `[CODE: docs/XENIA_PROVIDER.md]`. |
| **Xenia Canary TOML patches** | Yes | Yes | **Already implemented** `[CODE: crates/archivefs-core/src/patch_manager/xenia_install_plan.rs]` | No | Yes | **Exact** — Title ID keyed | Existing; the template for `patch.yml`. |
| **RetroArch cheat/patch files** | Yes | Yes | Existing catalogue path `[CODE: docs/RETROARCH_CHEAT_SOURCES.md]` | No | Yes | Serial/hash-driven via catalogue matching | Existing. |
| **Mod directories copied into emulator/game folders** | Inventory-only | Yes per file | Only through the shared engine with per-file backup/journal | No | Yes (restores/removes owned files) | Weak — usually manual association | Generic "folder of files" mods are the highest-variance category; only manifest-backed variants qualify (§5). |
| **Translation patches** | Per container (IPS/BPS/xdelta dominate) | Per container | Per container | Some ship exe patchers — those are refused | Per container | Per container | Historical fan translations sometimes ship Windows patcher executables — deep-link only in that case `[COMMUNITY]`. |
| **Bug-fix / restoration hacks** | Per container | Per container | Per container | Sometimes | Per container | Per container | No distinct format — treat by container. |

**Summary of the matrix:** the only formats that are simultaneously static,
bounded, hash-safe, transactional, exactly rollbackable, and executable-free
are the **delta-patch family (IPS/IPS32/BPS/UPS, then xdelta/PPF)** and the
**emulator-declarative family (PNACH, Gecko/AR, TOML, `patch.yml`, cheat
text)**. EmuWiz already implements the second family. Phase 1 should add the
first family's safest members (IPS/BPS), and treat everything else as
preview-only, deep-link-only, or unsupported.

---

## 5. Security model

**Foundational rule (existing, unchanged):** EmuWiz must never execute
arbitrary downloaded code merely because something calls itself a mod.
Executables, scripts, installers, and macros are never launched during
inspection, preview, matching, installation, verification, rollback, or
cleanup `[CODE: docs/CHEATS_MODS_SAFETY.md "Unknown code and original
files"]`. Mods do not get an exemption; they get the same rule plus stricter
format gating.

### 5.1 Refusal classes

| Category | Policy |
|---|---|
| `.exe`, `.msi`, `.bat`, `.cmd`, `.ps1`, `.sh`, `.scr`, `.apk`, `.appimage` | **Refuse as installable content.** Hash and record for provenance; offer only "open the original page". Never staged, never run. |
| Python, Ruby, Perl, Lua-hook, AutoHotkey, AutoIt scripts | Same refusal class — they are executable code regardless of intent. |
| Java/JAR installers | Same refusal class. |
| Native binaries of any kind (including "tools" like patchers) | Same refusal class. |
| Self-extracting archives (SFX) | Treat as executables: refuse. An SFX is a program, not an archive. |
| Password-protected / encrypted archives | **Refuse.** Bounded structural validation is impossible without extraction, and password distribution channels defeat provenance. The existing retrieval pipeline rejects unsupported structure `[CODE: docs/RETROARCH_CHEAT_SOURCES.md]`. |
| Nested archives | Bounded depth only (existing archive-depth limits); a nested archive is *inspected*, never recursively extracted into a destination. |
| Symlinks, special files, device entries | **Blocked** — existing no-follow rules `[CODE: shared preview/destination safety]`. |
| Path traversal, absolute paths, `..` components | **Blocked** — existing unsafe-path rejection. |
| Archive bombs / high compression ratios | **Blocked** — existing size, entry-count, expanded-size, and ratio limits `[CODE: docs/PATCH_CHEAT_MANAGER_DESIGN.md §resource limits]`. |
| Huge files | Hard per-file and total ceilings, source-type aware, raisable only via explicit local configuration (existing pattern: bounded download bytes, 8 MiB Phase 1 catalogue ceiling, 256 MiB cheat-source ceiling) `[CODE]`. |
| Writes outside approved roots | Impossible by construction: every destination is resolved and validated through the existing no-follow destination-safety layer before any write `[CODE: docs/SHARED_SAFE_APPLY_ROLLBACK.md]`. |

### 5.2 Positive policy

1. **Prefer declarative patch formats.** The installable set in Phase 1 is
   exactly: IPS, IPS32, BPS (and, through existing adapters, the
   emulator-declarative family). These are data, parsed in bounded memory,
   with no evaluation semantics.
2. **Executable-only mods are manual-only, and "manual" means *outside
   EmuWiz*.** EmuWiz shows the mod's metadata, attribution, and a deep link,
   and states plainly that it cannot install this safely. There is no
   "consent override" — consistent with the existing rule that consent
   cannot override a concrete technical block `[CODE: docs/CHEATS_MODS_SAFETY.md]`.
   A hypothetical future safe architecture would require a reviewed
   sandboxed patch-execution environment with zero network access, no user
   file access beyond an injected source/target, and a declared
   input/output contract — none of which exists today, so the refusal stands.
3. **Manifests over conventions.** A downloadable multi-file mod must arrive
   as a manifest-backed unit (explicit file list, digests, target paths,
   declared patch format, declared target identity). Convention-only folder
   dumps stay Unverified and preview-only.
4. **Nothing new executes.** A mod feature adds *no* new process-launching
   code paths. Emulator launching remains a separate, reviewed capability.
5. **Structural check ≠ malware clearance.** Inspection findings are
   presented as what they are (existing rule: EmuWiz is not antivirus, and
   passing a structural check does not promote Unverified to Trusted)
   `[CODE: docs/CHEATS_MODS_USER_POLICY.md]`.

---

## 6. Compatibility / identity model

### 6.1 Evidence EmuWiz already has

`[CODE: crates/archivefs-core/src/game_identity.rs — IdentityKind]`:
serials (PS1/PS2, PSP Disc ID, PS3 Title ID, Saturn product number,
Dreamcast product code, Sega CD product code), PCSX2 executable CRC,
Dolphin Game ID/revision/region, Xbox Title ID and Xbox 360 Title ID/Media
ID, ScummVM game ID, loose-ROM SHA-256 (exact-bytes and canonical
byte-order-normalized), 3DO/PC-FX/PC Engine CD/Neo Geo CD structural
identity, and NES/SNES header metadata. Hash matching against DAT identity
(No-Intro, Redump, MAME, TOSEC, WHDLoad, FBNeo, Hasheous) is established
`[CODE: crates/archivefs-core/src/identity_source/]`.

### 6.2 Confidence tiers (existing vocabulary, reused)

A natural design would be Exact / Strong / Likely / Manual-only. EmuWiz
already has three compatible ladders; this research proposes mapping mods
onto them rather than inventing a fourth:

| This research | Existing vocabulary | Meaning for mods |
|---|---|---|
| **Exact** | `PreviewMatchStrength::VerifiedExact` / `CheatMatchConfidence::Exact` / `IdentityConfidence::ExactBytes` | The mod's declared target identity matches the game's cryptographically verified identity — e.g., BPS source-CRC32 equals the CRC32 of the user's verified ROM; PNACH filename CRC equals the verified PCSX2 executable CRC; patch.yml key equals the verified PS3 serial; Game ID equals the verified Dolphin disc header. **The only tier where automatic apply is offered.** |
| **Strong** | `Strong` | Multiple independent non-cryptographic evidences agree (e.g., serial + region + revision, or a Git provider record explicitly declares the target DAT name and the DAT match is `Strong`). **Preview-only by default; apply allowed only through an adapter that can prove the relevant identity at apply time.** |
| **Likely** | `Candidate` / `Weak` | Title/platform/filename-level agreement only. **Preview-only, never installable.** |
| **Manual-only** | `Ambiguous` / `Unsupported` | Conflicting evidence, no evidence, or a format that cannot prove anything. Shown with attribution and a deep link; no install control exists. |

Rule inherited unchanged from the cheat pipeline: **never install an
ambiguous mod automatically**; filename text is never promoted to Exact
`[CODE: docs/PATCH_CHEAT_MANAGER_DESIGN.md, docs/DOLPHIN_CHEAT_CATALOGUE.md]`.

### 6.3 Which formats can prove compatibility

| Evidence source | Formats | Provable tier |
|---|---|---|
| Patch-embedded checksums | BPS (source+target CRC32), UPS | **Exact** against the user's verified ROM hash — self-contained, no provider trust needed |
| Provider-declared target hash | GitHub-hosted patch manifests that declare target SHA-256/CRC + DAT name | **Exact** when the declared hash matches; the declaration itself is provenance-bound (commit-pinned) |
| Provider-declared serial/Game ID/Title ID | PNACH, Gecko/AR, Xenia TOML, RPCS3 patch.yml, Dolphin texture-pack GameID directories | **Exact** — this is exactly how the existing cheat adapters already gate `[CODE: pcsx2_identity.rs, gamehacking_*_provider.rs, xenia_patch_document.rs]` |
| Filename/README text only | Bare IPS files, most RHDN-era downloads | **Likely at best** — never installable without additional evidence |
| Nothing | Executable installers, opaque archives | **Manual-only** |

Design consequence: a bare IPS with no declared target is *not* an edge case
to solve with heuristics — it is a **Strong-or-below mod** shown with a
warning and no install button, unless the provider record supplies a target
identity. BPS's embedded checksums are why it outranks IPS for Phase 1
despite IPS being simpler.

---

## 7. Download / provenance design

**Answer to the A–F question:** adopt **B + C + D + E**, with **A always**
and **F never**.

| Option | Verdict | Rationale |
|---|---|---|
| A. Only open original project pages | **Always available, never sufficient alone** | Every mod, in every phase, keeps its deep link. But link-only UX would make EmuWiz a bookmark manager, not a wizard. |
| B. Download directly from approved upstream on explicit user request | **Adopt** | The Vortex/r2modman model; lowest legal risk with real UX (§3). |
| C. API-driven downloads | **Adopt, per reviewed provider** | Only for providers whose API is anonymous or user-tokened and whose terms permit client access (GitHub today; GameBanana in Phase 2). |
| D. Content-addressed local cache | **Adopt** | Already the house pattern: immutable snapshots keyed by digest, provenance recorded, cache maintenance/locking already designed `[CODE: docs/RETROARCH_CHEAT_SOURCES.md, docs/RETROARCH_CHEAT_CACHE_LOCKING.md, docs/RETROARCH_CHEAT_CACHE_MAINTENANCE.md]`. |
| E. Local metadata catalogue | **Adopt, minimal** | Cached provider records (title, author, licence, target identity, digest, source URL) so the Mods page works offline and re-fetches are bounded. Not a new database schema in Phase 1 — reuse the existing source/manifest files. |
| F. Mirror anything | **Never** | §3 postures #4/#5. |

### 7.1 Provenance record (per downloaded mod)

Every cached mod snapshot records, immutably, the following — extending the
fields the cheat-source registry already uses (`download_url`,
`permitted_host`, `canonical_repository_url`, `provenance`, `licence_url`,
`pinned_version`, `maximum_expected_bytes` `[CODE:
docs/design/DAT_CHEAT_POLICY_MIGRATION.md]`):

- **Content digest** — SHA-256 of the downloaded artifact (plus the host's
  own MD5 where published, e.g., GameBanana).
- **Immutable source snapshot** — for Git providers, the exact resolved
  commit ID and the commit-pinned archive URL; "a branch name alone is never
  an installed snapshot identity" `[CODE: docs/RETROARCH_CHEAT_SOURCES.md]`.
  For non-Git hosts, the provider's stable project/file IDs and the fetch
  timestamp stand in for immutability, and the record says so honestly.
- **Source URL and permitted host** — the exact origin; redirects must land
  on the same approved host (existing redirect policy).
- **Author(s) and project title** — from provider metadata, displayed
  everywhere the mod appears.
- **Licence** — the provider-declared licence string/URL, or an explicit
  "licence not declared by source" state. Missing licence never blocks a
  *preview* (existing rule) but is displayed `[CODE: docs/CHEATS_MODS_SAFETY.md]`.
- **Downloaded timestamp** and **EmuWiz source-adapter version**.
- **Declared target identity** — serial/Game ID/CRC/SHA-256/DAT name the
  provider claims the mod targets, feeding §6.
- **Declared format** — the patch format(s) the manifest claims; verified
  structurally, not trusted.

### 7.2 Transport rules (inherit, unchanged)

Certificate-validated HTTPS GET only, disabled proxies, identity encoding,
dedicated user agent, zero-to-three manual redirects with exact approved
host retention, no credentials/localhost/private endpoints, DNS preflight,
bounded connect/idle/total timeouts, bounded body size, hash-verified
snapshots `[CODE: docs/RETROARCH_CHEAT_SOURCES.md §Network and extraction
protections]`. A mod download is the same transport with a different
allowlisted host and ceiling — it is not new network code.

---

## 8. Transaction / rollback design

**Mapping to the existing engine:** mod installation should be one more
consumer of the shared preview → plan → confirm → apply → verify → journal →
rollback pipeline, not a new one `[CODE: docs/SHARED_SAFE_APPLY_ROLLBACK.md;
shared_transaction.rs is already used by RetroArch materialization, PCSX2
PNACH, Dolphin Gecko/GameSettings, GameCube/Wii providers, and texture/mod
flows]`.

### 8.1 The derived-copy rule

**The source game file is never modified — ever.** For disc/cartridge games,
patch application is inherently destructive to the source: an IPS/BPS/xdelta
apply consumes the original ROM bytes and produces a different image. So:

- The patch is applied to a **copy** of the verified source artifact, and the
  result is written as a **new derived artifact** (e.g.,
  `Game (USA) [Mod — Author Title].z64` in an EmuWiz-managed derived location),
  leaving the original untouched.
- This is stronger than backup-based rollback: the original is never in
  danger, so "rollback" for ROM patches means **deleting or archiving the
  derived artifact**, which is always exactly possible.
- It also composes with EmuWiz's read-only mounting model: originals are
  proof-of-identity artifacts; mods attach to derived copies `[CODE: the
  existing rule "EmuWiz never silently deletes, rewrites, or sanitizes the
  user's original source" — docs/CHEATS_MODS_SAFETY.md]`.

For config/dir-based adapters (PNACH, Gecko, TOML, patch.yml, texture packs),
the existing behavior already matches: new files created, replaced files
backed up and journaled, originals outside the destination untouched.

### 8.2 Rollback classification

| Mod class | Exact rollback? | Mechanism |
|---|---|---|
| ROM delta patches (IPS/BPS/UPS/xdelta) into a derived copy | **Yes — trivially** | Remove the derived artifact (or re-apply to regenerate); original was never touched. No backup of the original is even needed. |
| New config/patch files (PNACH, TOML, patch.yml entries, cheat files) | **Yes** | Journal deletes owned, unchanged installed files — existing behavior. |
| Replaced files (texture packs, Riivolution-style) | **Yes, with the verified backup** | Existing backup + hash-revalidated restore; changed destinations fail closed `[CODE: docs/SHARED_SAFE_APPLY_ROLLBACK.md]`. |
| Manifest-backed multi-file packs | **Yes, per file** | Journaled per-entry; a locally changed file is reported and left untouched unless the user explicitly approves replacement. |
| Executable-patcher-based mods | **No** | Not installed by EmuWiz at all; nothing to roll back. If the user installed it manually outside EmuWiz, EmuWiz has no journal and says so. |
| Anything installed outside EmuWiz | **No** | No journal, no ownership — preview-only forever. |

### 8.3 Preview-only categories

Everything at Strong-or-below confidence (§6), every executable-bearing
container, every encrypted archive, every manifest-less multi-file dump, and
every source whose content class is §3-refused (asset rips, pre-patched
images) is preview-only: EmuWiz explains what it found, shows attribution and
the deep link, and offers no apply control.

### 8.4 Verification pass

After apply, the existing verify step re-reads every written path and checks
it against the plan's digest (existing behavior). For derived-copy ROM
patches, verification additionally re-checks the target checksum carried by
BPS/UPS patches — the format's own integrity proof, applied to the produced
artifact.

---

## 9. Beginner GUI journey

The Mods journey follows the established beginner-workflow pattern: the
default view shows outcomes and choices; every safety mechanism stays
available but collapsed behind **Details**, and opening Details never
triggers network requests or state changes `[CODE:
docs/CHEATS_MODS_BEGINNER_WORKFLOW.md]`.

1. **Game** — the user picks a game from their library (existing
   Cheats & Mods entry point).
2. **Mods** — a new "Mods" step beside the existing cheats step. Status line
   in plain English: *"We found 3 mods that fit this game"* or *"No mods
   found for this game yet"* — honest empty states, no spinner-forever
   `[CODE: same document's "no compatible cheats found" pattern]`.
3. **Compatible mods found** — a short list: mod title, author, one-line
   description, and a small type badge ("Patch", "Cheat file", "Texture
   pack"). A link "See the original page" is always present (the deep link).
4. **Choose one** — the mod's page shows author, source site, licence (or
   "the author hasn't declared one"), what it changes in plain English, and
   the download source. Technical details (commit ID, digest, provider IDs)
   live under Details.
5. **See what it changes** — the preview: which file(s) will be created or
   replaced, in which emulator folder — or, for ROM patches, that a new
   *copy* of the game will be created and the original will not be touched.
6. **Verify compatibility** — a plain-English identity statement: *"This
   patch is confirmed to fit your copy of the game"* (Exact) vs *"This patch
   looks like it fits your game, but we can't prove it"* (Strong/Candidate —
   no install button). No CRC jargon in the default view; the raw evidence
   (digests, serials, tiers) is under Details.
7. **Install safely** — one explicit confirmation binding the plan (existing
   plan-ID confirmation), then the transaction applies with backup, journal,
   and verification. Result: a plain success/failure statement
   `[CODE: docs/CHEATS_MODS_BEGINNER_WORKFLOW.md's simplified flow]`.
8. **Verify** — the success statement includes what was verified
   ("the patched copy was created and checked").
9. **Undo** — an always-present **"Undo installation"** button after any
   install, backed by the journal, with the same fail-closed rules as cheat
   rollback.
10. **Executable-only mods** — the mod's page says plainly: *"This mod uses
    an installer program, which EmuWiz can't run safely. You can still get
    it from the author's page."* with the deep link. No shame, no jargon, no
    hidden path.

---

## 10. Recommended provider order

1. **GitHub** — the only provider that today satisfies the full retrieval
   policy (anonymous, immutable, digestable, licensed metadata). Hosts the
   already-integrated `libretro-database` and `xenia-canary/game-patches`
   patterns, plus the largest declarative-patch corpus (RPCS3 patch.yml,
   widescreen/60fps patch repos, individual hack repos with releases).
   **Phase 1 download provider.**
2. **Emulator-native declarative sources already integrated or adjacent**
   (GameHacking.org, BSFree, libretro-database, xenia-canary/game-patches,
   and RPCS3's patch repo as the next adapter) — lowest incremental risk
   because the provider, transport, and adapter patterns are proven
   `[CODE]`.
3. **GitLab** — same architecture, second ecosystem. Optional Phase 2.
4. **GameBanana** — the best general mod host by machine interface; Phase 2
   downloads (texture packs, per-game mods) with MD5 verification and
   mandatory attribution display; Phase 1 deep-links only.
5. **PCGamingWiki** — metadata/link hub for discovery and attribution, never
   downloads. CC BY-NC-SA obligations on any displayed text.
6. **Internet Archive** — read-only provenance and metadata (checksums!) for
   the RHDN archival corpus; explicit user-requested downloads acceptable in
   a later phase once redistribution terms for specific items are checked
   per item.
7. **ModDB** — deep-link + optional attributed feed metadata.
8. **Romhacking.net (deep links), Romhack.ing / Romhacking.com (deep
   links)** — cultural center of the patch ecosystem; no machine interface;
   never automated.
9. **Nexus Mods** — deep-link only, indefinitely; its key/throttle/permission
   model is the opposite of EmuWiz's anonymous-reviewed-source posture.

---

## 11. Explicit unsupported categories

These are refused by policy, not deferred for lack of engineering:

1. **Complete copyrighted games** — ROMs, ISOs, disc images, BIOS files, or
   any mod distribution that embeds them (pre-patched images included). The
   existing rule already blocks ROM/ISO acceptance into any trusted cache
   `[CODE: docs/PATCH_CHEAT_MANAGER_DESIGN.md "must never request, accept
   into its trusted cache, install, or distribute complete copyrighted
   ROMs, ISOs, disc images, or games"]`.
2. **Executable installers and scripts** (§5.1) — including "just run this
   patcher" workflows.
3. **Self-extracting and password-protected archives** (§5.1).
4. **Asset-rip redistributions** — packs whose content is modified game
   files (texture rips, extracted audio, model rips) rather than original
   art (§3 posture #7).
5. **Mirroring/rehosting anything** (§3 posture #4) and bundling third-party
   mods into EmuWiz releases (#5).
6. **Scraping or automated bulk access to any host** — including hosts that
   permit human browsing but not automated access (RHDN's Cloudflare posture
   observed during research).
7. **Circumventing host access controls** — paywalls, login walls, region
   gates, or bot challenges. Consistent with the existing no-circumvention
   rule `[CODE: docs/CHEATS_MODS_SAFETY.md §Responsible use]`.
8. **Mods for games EmuWiz cannot identify to the Exact tier** (§6).
9. **Paid or account-gated downloads** — EmuWiz will never handle credentials
   for content hosts; if a mod requires an account, it is deep-link only.
10. **Cheat/patch content that requires decryption** (e.g., encrypted
    cheat-device formats) — already the boundary in the cheat pipeline
    `[CODE: docs/research/CHEATS_MODS_EXPANSION_RESEARCH.md §"no decryption"
    P1 reasoning]`.

---

## 12. Phase 1 implementation proposal

A deliberately small v1, chosen because every piece reuses a proven pattern:

**Scope**

1. **One provider: GitHub.** Reuse the libretro-database retrieval pattern —
   compiled-in reviewed source(s), commits-API resolution to an exact commit,
   immutable commit-pinned archive, SHA-256 recorded, bounded extraction
   `[CODE: docs/RETROARCH_CHEAT_SOURCES.md]`. No user-supplied URLs in v1.
2. **Two patch formats: BPS and IPS/IPS32.** Pure-Rust, bounded-memory,
   structurally validated parsers with hard size/expand ceilings. BPS first
   (embedded source/target checksums make compatibility provable, §6);
   IPS/IPS32 supported only when the provider record declares a target
   identity (hash, DAT name, or serial).
3. **One apply path: derived-copy ROM patching** for platforms whose loose
   ROMs EmuWiz already identifies exactly (LooseRomSha256 / canonical SHA-256
   / cartridge headers `[CODE: IdentityKind]`). Output is a new derived
   artifact; the original is never touched (§8.1).
4. **Exact identity required.** Install is offered only at the Exact tier.
   BPS self-proves via source CRC32; provider manifests must declare the
   target for IPS.
5. **Preview before install, transactional apply, rollback** — through the
   existing shared preview/transaction/journal engine; rollback deletes the
   derived artifact (always exact).
6. **Attribution and source link always displayed**; provenance record per
   §7.1 stored beside the immutable cached snapshot.
7. **Everything else is deep-link or preview-only**: executable installers,
   scripts, encrypted archives, GameBanana/Nexus/ModDB/RHDN downloads, and
   any format not listed above (§4, §5, §11).

**Explicit non-goals for Phase 1:** no GameBanana/Nexus/ModDB/RHDN/RHDI
downloads; no texture-pack downloads (the *existing local* Dolphin texture
workflow continues as-is); no xdelta/PPF; no Riivolution; no account/token
handling for content hosts; no new process execution anywhere.

**Why this is the right first slice:** it is the intersection of (a) the
formats with the best built-in compatibility proof, (b) the one provider
whose terms and mechanics match the existing transport code exactly, and
(c) the apply path whose rollback is exact by construction. Every other
phase extends one of these three.

---

## 13. Phase 2+ roadmap

| Phase | Content | Preconditions |
|---|---|---|
| **2a** | **RPCS3 `patch.yml` provider + adapter** — declarative, serial-keyed, GitHub-hosted; mirrors the Xenia TOML adapter. Config-file apply path (exact rollback via journal). | PS3 identity evidence; RPCS3 profile discovery (pattern exists in `emulator_environment`). |
| **2b** | **GameBanana provider** — anonymous apiv11 metadata, explicit user-requested downloads, MD5 verification, mandatory attribution; texture packs and per-game mods for already-supported adapters. | Per-submission licence display; provenance record extended for non-Git hosts; per-host download ceilings. |
| **2c** | **xdelta/VCDIFF apply** — bounded decoder; target identity from provider manifests only. | Pure-Rust VCDIFF decoder decision + resource-limit tests. |
| **3a** | **GitLab provider** (same architecture as GitHub). | Current rate-limit verification. |
| **3b** | **Riivolution-style manifest packs** — declarative file replacement with per-file provenance checks (refuses asset-rip content class). | GameCube/Wii disc identity at Exact tier; per-file licence/provenance policy settled. |
| **3c** | **PPSSPP CWCheat adapter** fed by reviewed sources only (per the prior expansion research's P1 note — no community-mirror `cheat.db` until provenance exists). | PPSSPP profile discovery. |
| **4** | **Internet Archive metadata integration** for RHDN-archival provenance and deep links; possibly attributed per-item downloads after per-item licence checks. | Per-item redistribution review. |
| **Deferred indefinitely** | Nexus Mods programmatic access; scraping of any host; mirroring; executable-patcher execution (would require a reviewed sandbox architecture that does not exist). | — |

---

## Appendix A — Research provenance and open questions

**Primary repo evidence:** all `[CODE]` citations were read in this worktree
at `4fbaebc01973cefae4b6af28266c8310cd68aa34`.

**External research method:** direct fetches plus secondary sources
(Wikipedia citing the RHDN announcement, PC Gamer, Polygon, GamesRadar;
provider documentation). Sites that blocked automated access during research
(romhacking.net, parts of Nexus/ModDB documentation, JS-only RHDI/RHDC) were
treated as unverified rather than assumed.

**Open questions to resolve before Phase 2:**

1. GameBanana's current published rate limits and any API-terms updates
   `[UNVERIFIED]`.
2. Nexus API exact current limits (irrelevant while deep-link-only).
3. RHDN's current download availability and ToS text (Cloudflare-blocked
   during research) `[UNVERIFIED]`.
4. RHDI/RHDC content policies as the RHDN successor ecosystem matures
   `[UNVERIFIED]`.
5. Redistribution terms attached to specific Internet Archive items holding
   the RHDN dump `[UNVERIFIED]` — item-by-item before any Phase 4 downloads.
6. Choice of a pure-Rust VCDIFF decoder crate (or vendored implementation)
   for Phase 2c, with resource-limit hardening.

