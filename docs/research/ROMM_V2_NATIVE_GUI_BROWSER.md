# GUI-v2 native read-only RomM browser

Starting main: `1c43c8e48999382f8b38dd3a0fd33008f665a85c`.
Validation base after incorporating eight unrelated intervening commits:
`401af8c9d16478a282f08e7c073736785038cd6b`.
Branch: `feature/gui-v2-native-romm-browser`.

## Location and collision boundary

Open **Sources & Providers → Browse RomM library**. A prominent entry at the
top of the overview and the existing RomM provider card open the same native
browser. Setup/recovery opens the existing RomM configuration dialog directly;
Close/Escape/Alt+Left returns to the browser, which rechecks configuration.
No additional top-level navigation section is added.
The existing editor gains a settings-only constructor, preserves saved optional
values without reading a cached catalogue, and exposes the existing enabled
flag as an explicit local configuration choice. Source enablement remains
opt-in; connecting never bypasses a disabled source. Unsafe credential/query
URLs are withheld from prefilled fields and require a corrected address.

The current cached snapshot page and legacy browser remain available with
their existing navigation labels. Live browsing uses the promoted core browser,
never the legacy import/browser implementation.

The initial ownership scan found `gui_v2/mod.rs`, `gui_v2/pages.rs` and
`gui_v2/tests.rs` modified in `/home/davedap/emuwiz-internal-gold`. GUI library
root and legacy RomM controller files also had owners. These files are
untouched. The existing clean `native_workflows.rs` owns the new focused
module and presents its window from Sources & Providers. All scoped candidate
paths passed a separate worktree collision scan. Core `normalise.rs` remains
unchanged, including its concurrently owned media work.

## User experience

The connection card explains unconfigured/disabled settings, connecting,
connected, name lookup failure, unreachable/timeout, authentication, TLS,
unsupported versions/capabilities and invalid server replies. Errors explain
the failure, what stayed unchanged and a useful next step. Settings and Retry
are the recovery actions; Retry repeats the failed request. Technical details
are collapsed by default and contain safe typed results, version/capability
evidence and the approved server identity. TLS guidance never suggests
disabling verification.
Unreadable local settings settle into an error until an explicit retry or
settings action; they do not start an automatic reread loop. The existing
settings footer is shortened so Save/Cancel and its local-only explanation
fit a 700-pixel window.

The read-only explanation is always visible, including before connection.
Platform names are friendly labels; absent names use a normal placeholder.
Unknown/custom platforms remain visible through the backend's conservative
mapping. Optional search/filter/detail capabilities have explicit explanations.

Games use **50-item server pages**, Previous/Next and an evidence-based
"Showing …" range. Search is server-side, bounded to 128 characters (at most
512 UTF-8 bytes), and debounced for 350 ms. Platform filtering uses the typed
backend filter. No whole-library download or client-side catalogue filtering
is performed. Empty states distinguish an empty visible library, a platform
without visible games and an unsuccessful search; they do not invent an
account-permission diagnosis the server has not provided.

Selecting a title opens typed details with metadata, artwork, filenames,
validated hash/provider identifiers and provenance. Identity information is
labelled **external evidence**, with "RomM suggests …" wording. No identity
fact, DAT authority, catalogue binding or user-selected metadata is written.
Back/Escape return to the same server page, search and platform filter; the
previous selection remains highlighted. Escape first respects search focus
and open popups. Alt+Left is consumed for the visible browser before shared
page navigation can act beneath it.

## Backend, workers and artwork

The GUI glue calls the existing `RommBrowser::{discover, platforms, games,
game_detail}`, `ValidatedRommSource`, credential-file loader and
`UreqTransport`. It defines no second configuration, HTTP stack, normalizer,
identity representation or download API. Each independent operation discovers
the server's advertised capabilities before use, as required by the backend.
The studied RomM 4/5 version gate and endpoint/capability contract are
documented in [the promoted backend report](ROMM_V2_NATIVE_BROWSER_BACKEND.md).
No core backend source changes in this candidate.

All settings/network/image work runs through the existing GUI `Backend`
worker and Activity jobs. Browse and artwork each allow one in-flight request
and one latest pending request. A generation changes immediately on search,
filter, page, selection, settings or close actions. Cancelled/older replies
cannot replace the current view, including cover and detail replies. The
separate artwork worker keeps a slow image from delaying navigation/search.
Browser rendering clones only bounded page/detail `Arc`s and performs no I/O.
The reused configuration editor retains its existing bounded local token-field
validation; its save/name-resolution/preview work uses its existing worker.

Artwork uses the existing `ArtworkCache`, thumbnail decoder, sizing helper and
platform glyphs. Only visible items request a cover, one at a time. Missing,
refused or failed artwork remains a placeholder and never becomes a browsing
failure. A failed cover is remembered for that view, preventing repeated
automatic fetches. The existing bounded local artwork cache may be populated;
small hosted covers are preferred when present, with the existing bounded
large-cover policy used only when no small reference is available.
No ROM, remote artwork mutation or new cache architecture is involved.

## Security and read-only boundary

TLS verification, environment-proxy isolation, private-endpoint validation,
redirect refusal, bounded timeouts and response limits are inherited unchanged.
There is no automatic retry. Configuration reading has a 1 MiB guard; remote
metadata remains subject to the core response/string/list/nesting limits.
Token-file permissions and validation use the existing credential policy.

The browser never renders raw configuration URLs, HTTP error bodies, transport
messages, Authorization headers or tokens. Provider text is display-only:
the opaque existing `RommToken` masks reflected credentials and URL user-info
before drawing, without changing the original typed identity/provenance
evidence. Technical details use the same display boundary. The overview's
RomM card no longer echoes arbitrary legacy diagnostics or unvalidated URLs.

Upload, delete, rename, rescan, metadata/favourites/collections changes and ROM
downloads have no controls or new public operations. Browsing performs no
catalogue import, cache-to-library reconciliation or verified-identity write.

## Validation

Validation uses `/tmp/emuwiz-native-romm-gui-5s765roo/target`, offline/locked
Cargo and local deterministic fixtures. The focused GUI tests cover
configuration, connection/errors/redaction, platforms, server filters/search,
debounce/stale replies, pagination, details/evidence, artwork failure, empty
states, read-only controls, settings return, Back/Escape and a genuinely slow
HTTP worker without blocking egui. The unchanged core RomM regression suite
is rerun using its promoted-base isolated test binary (159 passed, one opt-in
real-server test ignored), after verifying core/Cargo sources match that base.

Final checks:

| Check | Result |
| --- | --- |
| Native RomM GUI focused tests | 39 passed |
| Existing RomM configuration tests | 46 passed |
| GUI-v2 suite (includes the focused browser tests) | 530 passed, 3 existing ignores |
| Core RomM regression suite | 159 passed, 1 real-server opt-in ignore |
| `cargo check --offline --locked --workspace` | Passed |
| `cargo fmt --all -- --check` | Passed |
| `git diff --check` | Passed |
| Fresh default-profile release `emuwiz-v2` build | Passed |

The smoke profile is a disposable copy of
`/home/davedap/emuwiz-smoke2-profile`, with paths rewritten into the copy and a
synthetic loopback server. Neither real configuration nor real RomM data is
used. All 260 original regular profile files retain their original SHA-256 values.
Screenshots and command logs stay outside the repository under
`/tmp/emuwiz-native-romm-gui-5s765roo`.

The release smoke uses software rendering under Xvfb at **1280×800** and
**700×520**. It covers unconfigured settings, a connected 55-game synthetic
library, debounced search, custom-platform filtering, the 51–55 page, details
and external identity evidence, missing artwork, no search matches, an empty
library, DNS/unreachable errors, authentication refusal and unsupported
server capabilities. Settings/return, keyboard focus, Escape and Back are
checked. Text wraps without horizontal scrolling; compact results/details
scroll vertically with Back outside that scrolling body. Error details stay
collapsed and missing artwork never prevents browsing.

The recorded HTTP trace contains only GET requests, bounded game pages of at
most 50, server-side search/platform parameters and explicit next-page
offsets. It contains no ROM-content request or write request. Real-server
acceptance is not claimed. The original main worktree and all candidate
integration files are rechecked before commit; 393 worktrees were scanned
without a scoped collision.

## Next RomM / GUI gaps

- A direct full-page live RomM sidebar route can replace the cached route
  after the coordinator/dispatcher lane is free. The current Sources entry
  already exposes the complete live workflow without touching that lane.
  The legacy "RomM Library" label also remains compatible with the occupied
  GUI regression/dispatcher lane; clarifying that sidebar entry is deferred.
- Server-side sorting or additional filters require explicit native backend
  capability support; the GUI does not approximate them locally.
- Reconciliation of external identity clues into local verified evidence is a
  separate reviewed workflow. This browser does not perform it.
- Real-server acceptance remains separate from the deterministic synthetic
  checks. No real server or credential is required for this candidate.
- Existing startup work can briefly show Home before restoring the saved
  Sources route. Its compact loading copy is a separate GUI layout gap; this
  candidate changes neither startup navigation nor the Home page.
