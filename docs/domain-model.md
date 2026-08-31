# Domain model

## CURRENT BEHAVIOR

The core model separates what was observed, what is authoritative, what is
persisted, and what may execute:

| Concept | Role |
|---|---|
| Archive/library item | Durable source item and path/content observation |
| Platform / IdentityPlatform | Hardware/software target plus platform evidence |
| Identity evidence | Filename, folder, extension, header, member, detector, or hash observation |
| Verified identity facts | Fresh, bounded facts proven by structural/media inspection |
| DAT identity / set verdict | External release/hash and set/dependency authority |
| Media/content format | How bytes are represented and which readers are safe |
| Emulator profile | Discovered executable/config/core and installation context |
| Launch readiness | Preflight result; blockers are not identity |
| Launch plan/command | Exact executable, arguments, content, and evidence for execution |
| Provider/source | Origin and validation path for cheat/mod material |
| Transaction plan | Explicit proposed local writes, backups, and rollback ownership |
| Journal/history | Durable operation result and rollback evidence |
| Library View | Managed local symlink projection |
| Playing Library / 1G1R | Evidence-backed selection/grouping projection |
| RomM/ES-DE projection | External/library-facing path and metadata projection |
| Mod package plan | Bounded local inspection and proposed handling of a package |

Weak filename/folder/extension evidence can guide a candidate or platform
classification, but it must not silently become verified game identity. A
launch or apply consumer must request the verified facts it requires and fail
closed when they are absent, stale, ambiguous, or conflicting.

## Relationships

An archive item may contain one or more members; a direct media file has no
outer member but enters the same content pipeline. Ingestion records the
container/content/media observations. Identity fusion combines those
observations with platform evidence, verified facts, and DAT/hash authority.

The catalogue persists these results for explanation and efficient views.
Launch planning reads the selected item and freshly validates the execution
inputs. A readiness result can block a valid identity, and a compatibility row
can permit a family without producing any identity.

Providers produce candidate material. An adapter validates the target profile,
identity, destination, and materialized source. Only an eligible transaction
plan can reach the shared apply engine. Library, RomM, and ES-DE outputs are
projections; they do not become identity authority.

## Authority vocabulary

- **Evidence:** observation with provenance and confidence.
- **Authority:** verified direct-media facts or a matching DAT/hash/set result.
- **Persistence:** catalogue/cache/history storage of observations and results.
- **Projection:** a derived view or export, including Playing Library, RomM,
  and ES-DE.
- **Execution:** launching a process or applying an explicitly confirmed
  local transaction.

## HISTORICAL DESIGN CONTEXT

Older names such as ArchiveRecord and launch_plan remain in compatibility
interfaces and historical design notes. They should not be read as a single
flat authority model: current launch, identity, and transaction stages are
separate.
