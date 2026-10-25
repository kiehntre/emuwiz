# Blue EmuWiz update check

Settings → About EmuWiz → **Check for Updates** performs an explicit,
read-only release-metadata check. It shows the installed package version,
available version, last completed result, optional plain-text release notes,
and a validated official release-page link. Results live in this session only.
There is no startup check, artifact download, installation, restart or setting
change. The worker runs outside the GUI thread; only one check can run at once.

## Recovered work

The core checker and worker/channel adapter come from
`feature/archivefs-unified-platform` at `eba83521`. The original 17 core tests
were recovered, with stricter response validation and additional cases. The
adapter now belongs to Blue Settings instead of the historical GUI root.
The existing HTTPS emulator transport, sealed HTTP proxy opt-out policy and
workspace package version remain authoritative. No networking backend was added.

## Trust and result semantics

The established official repository is `https://github.com/kiehntre/emuwiz`.
The only requested URL is
`https://api.github.com/repos/kiehntre/emuwiz/releases/latest`.
Only HTTP 200 is accepted. Redirects are refused, including other GitHub API
paths whose repository identity cannot be independently established. Metadata
is bounded to 2 MiB, length-checked, and parsed as a stable, non-draft release
with a valid semantic version. Build metadata does not affect precedence.

A returned page link must identify that exact repository and release tag over
HTTPS, without credentials, query or fragment, and is revalidated by the GUI.
Assets are ignored. Notes are plain text, with an 8,000-character GUI excerpt;
remote Markdown, HTML and images are not interpreted.

Results distinguish a newer release, equal version, newer local version,
unparseable current version, connection/transfer failure, unavailable endpoint,
malformed/untrusted metadata and an interrupted worker. A connection failure
may mean offline operation; it does not prove global network availability.
No published release (HTTP 404), throttling, or redirect is unavailable rather
than a guessed latest version. Failed checks retain the installed version but
do not reuse an earlier successful version as the new result.

The comparison concerns package version tags, not Git commits or binary
identity. Missing release links/notes remain absent. No real release request
or in-place update is needed for focused tests: fixtures inject metadata and
worker results, and existing HTTP tests exercise proxy opt-out locally.

## Historical preservation

The original named worktree is preserved as the existing archive under
`emuwiz-preservation/20261016-082-worktrees/emuwiz-082-batch7-self-update-dryrun`;
its manifest and source were inspected read-only. This task does not modify or
retire it. Preserve the historical branch: recovery of this checker slice does
not account for its other unique work.
