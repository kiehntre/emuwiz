# CI completion notifications

After this feature is approved and merged into `main`, GitHub Actions runs
`CI completion notification` on completion of **CI** or default-setup **CodeQL**.
No service, PAT, extra secret, schedule, Rust build or agent watcher is needed.
The existing CI gates and risk tiers are unchanged. A separate read-only
`CI notification tests` workflow runs on PRs touching the notifier; the
privileged `notify` job never runs PR code.

The notifier evaluates open PRs matching the event's exact head SHA (including
fork PRs and CodeQL runs whose event has no PR association). It selects the
latest CI and CodeQL runs for that head and inspects all their latest jobs,
head check runs and commit statuses. It posts one comment mentioning `@kiehntre`
with the PR number/link, full SHA, result and workflow/job links:

- **READY**: every applicable job and check succeeded, CodeQL's separate analysis
  result succeeded, and GitHub reports `MERGEABLE` and `CLEAN` on a non-draft PR.
- **FAILED**: completed checks failed, were cancelled, timed out, unexpectedly
  skipped/neutral, or mandatory jobs were missing. Failing jobs are linked.
- **BLOCKED**: checks passed but GitHub reports a draft, conflict, unknown
  mergeability, outdated base or another merge restriction. This is not READY.
- **PENDING**: something is still running or CI/CodeQL results have not arrived.
  No comment or notification is posted yet; the next completion event evaluates
  again. Missing results never count as success.

Only the six release/installer/reproducibility jobs already conditional in CI
may skip on the existing core/gui tiers. The notifier uses the trusted existing
path classifier and matches skip exceptions to exact check IDs in the CI run.
All four configured CodeQL languages (Actions, C/C++, Python and Rust) are
required. Update the notifier contract/tests when CI job names, shards or
CodeQL languages change. At installation, `main` has no branch protection or
rulesets; this feature does not change repository settings. Future GitHub
merge restrictions must still report `CLEAN` before READY.

## One result per PR head

Workflow concurrency serializes events per SHA; an authenticated bot comment
marker deduplicates the final result per PR number and SHA. Pending events may
coalesce safely because each evaluation reads current results. The notifier
rechecks the PR head/base before posting and ignores stale heads and closed PRs.
It never edits an old result. A rerun after a final FAILED/BLOCKED notification
does not send a second final notification for the same head. Push a new commit
for a new notification, or inspect the rerun during a separate readiness check.
Do not delete the bot comment if you want deduplication to persist.

GitHub does not provide an atomic transaction between check reads and a comment
write. A comment is a snapshot for its printed SHA, never permission to merge.
Always verify the current exact head, completed checks and approval before merging.

## Enable desktop and mobile delivery

1. On GitHub, open **Settings → Notifications** and enable **On GitHub** for
   participating conversations and mentions. The final comment mentions
   `@kiehntre`, so repository-wide watching is optional. Ensure the repository
   is not ignored; optionally use **Watch → Custom → Pull requests** or subscribe
   to a particular PR. Open the bell/inbox on desktop to read the result. Enable
   email too if you want desktop mail-client alerts; web inbox delivery does not
   itself guarantee an operating-system popup.
2. Install/sign in to GitHub Mobile. Under **Profile → Settings → Notifications**
   (iOS) or **Configure Notifications** (Android), enable **Direct mentions**.
   Allow notifications in the phone's OS settings and check working-hours/quiet
   settings. Optional Actions success/failure notifications report individual
   runs; the notifier's mention delivers the aggregate PR result.

See GitHub's [notification configuration guide](https://docs.github.com/en/subscriptions-and-notifications/get-started/configuring-notifications)
and [Actions mobile notification guide](https://github.blog/changelog/2023-01-17-push-notifications-for-actions-on-mobile/).

## Validation and operations

Run `node --test scripts/ci-notify.test.cjs`. Tests use synthetic API fixtures;
they do not post comments or use credentials. Coverage includes pending/failure
precedence, expected and unexpected skips, CodeQL, missing jobs, mergeability,
duplicate/spoofed markers, stale heads/bases, and fork-style empty associations.

The workflow must exist on the default branch before completion notifications
activate. Allow GitHub Actions to create PR comments with the built-in token;
the notify job explicitly requests `pull-requests: write` and read-only access
to contents, Actions, checks and statuses. It never calls merge, approval or
bypass APIs. Checkout is pinned to the trusted workflow revision with credentials
disabled; no PR artifacts or caches are consumed. All action versions are pinned.

Each completion event takes one snapshot, without a wait loop. Supplemental
external `check_run` events are best effort: GitHub suppresses these events for
Actions-associated suites/SHAs. If GitHub delivers the last event before a
separate CodeQL result is visible, an API request fails, or an event is lost,
use **Actions → CI completion notification → Run workflow**, select **main**,
and enter the full PR head SHA. This is one manual reconciliation, not a polling
schedule. Existing final comments remain deduplicated. See GitHub's
[event delivery rules](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows).

Development agents finish after local validation, authorized push/PR creation,
exact-head CI startup verification and one immediate-failure check. Report PR,
head SHA, run links and **PENDING** while CI runs, then end the session. On the
final notification, perform a separate short readiness check using completed
results; fetch detailed logs only for a failed check or specific evidence need.
Never automatically merge, approve, or weaken existing gates.
