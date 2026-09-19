# DAT D-05 — No-Intro managed user update

EmuWiz uses a browser-assisted, local-only No-Intro update flow. The user
opens the official DAT-o-MATIC page, downloads a ZIP in the browser, and
selects that file in EmuWiz. EmuWiz does not scrape the site, infer generated
URLs, automate login, persist browser cookies, or download the pack itself.

The existing bounded pack importer remains authoritative for validation. It
checks the ZIP and every supported DAT member, rejects traversal and unsafe
archive members, enforces the archive/member/aggregate limits, validates
internal No-Intro metadata, detects variants, and records the ZIP and DAT
member SHA-256 values.

The managed lifecycle is now explicitly staged:

1. Inspect/validate is read-only and produces release/version, system,
   variant, count, and hash signals.
2. Stage publishes a content-addressed candidate and a small staged manifest;
   it does not change the active pointer or registered verification sources.
3. The user explicitly activates the staged candidate. Activation updates the
   active pointer, preserves prior snapshots, registers the validated source,
   and returns `NeedsRecheck` for verification consumers.
4. Existing lifecycle comparison and rollback metadata remain offline. The
   rollback execution restores the previous valid pointer without deleting
   the newer snapshot and also returns `NeedsRecheck`.

Freshness is separate from activation. A single newly imported snapshot is
`NeverChecked`; a local comparison can establish `Current`, while ambiguous
or unavailable evidence remains `Unknown`/`CheckFailed`. Active does not mean
current upstream. No automated upstream freshness check exists.

The active content-addressed snapshot remains usable after restart and
without a network connection. No-Intro pack bytes are never rewritten by the
lifecycle layer, and failed validation cannot publish or replace the active
snapshot.
