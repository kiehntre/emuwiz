# CLAUDE.md

Read `AGENTS.md` first: it holds the repository's agent rules (worktree safety, file scope,
formatting, temporary files, commits). Everything there applies to Claude Code.

## Cargo builds

Run every Cargo command through `scripts/cargo-iso` (for example
`scripts/cargo-iso test -p archivefs-gui --lib`). It builds into a target directory unique
to the current worktree, overrides any inherited `CARGO_TARGET_DIR`, refuses the legacy
shared target `~/.cache/emuwiz-cargo-target`, and refuses to build when under 8 GiB is
free. Never run bare `cargo` for builds, never export a shared `CARGO_TARGET_DIR`, and never
clean or delete a target directory you did not create. See the "Cargo builds" section of
`AGENTS.md` for details, including shells that inherited the old shared target.

## Automatic repair and security policy

The [shared policy in AGENTS.md](AGENTS.md#automatic-repair-and-security-policy)
is authoritative for both Claude Code and Codex. Apply its seven subsections
alongside all existing project rules and task-specific restrictions:

1. **Automatic defect repair:** during implementation, determine the root cause,
   reproduce with synthetic fixtures, add a regression that fails before the fix,
   make the smallest safe correction, retest and continue. Ordinary compilation
   or routine test failures do not require owner permission for scoped repairs.
2. **Security and preservation first:** prioritise data preservation, filesystem
   identity and symlink safety, database/backup integrity, saves, Undo/recovery,
   approval freshness, credentials, temporary files and truthful results. Never
   weaken checks to pass tests or remove a failing test without a justified
   replacement preserving coverage; fail closed when safety is unproven.
3. **Avoid endless repair loops:** allow at most three focused correction/retest
   attempts per defect, then reconsider the architecture. Document evidence and
   options and obtain approval before substantial design changes; do not rename
   the defect to reset the count or rewrite unrelated code.
4. **Independent review stays independent:** a READ-ONLY INDEPENDENT REVIEW
   reproduces in isolation, reports exact evidence and source locations, returns
   PASS, PASS WITH CONDITIONS or FAIL, and recommends the smallest correction.
   The implementation owner repairs production code. Security-sensitive fixes
   require independent re-review of the exact final commit.
5. **Ownership and isolation:** check Git state and active file ownership before
   edits, preserve other agents' dirty work and worktree boundaries, and use the
   existing Cargo isolation rules plus private HOME/XDG/TMP/application roots.
   Never test destructive operations on real user data or create unnecessary
   worktrees or duplicate build caches.
6. **Automatic completion:** within approved implementation scope, continue
   through Implementation → Tests → Repair → Retest → Local commit → Report.
   Stop only for genuine safety, ownership, scope, evidence or architectural
   blockers. Read-only tasks remain read-only. Never push, merge, release or
   enable dangerous operations without explicit permission.
7. **Reporting:** include defects, causes, repairs, regressions, actual final
   test results, limitations, the exact local commit SHA and independent-review
   requirements. Distinguish executed tests from reported or proposed validation;
   explain unresolved blockers without claiming completion.
