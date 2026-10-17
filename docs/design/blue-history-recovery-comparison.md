# Preserved GUI recovery compared with Blue

Historical tip: `3c977acd889e4f87fa6745715c9e2b4fa3763a3f`
(`refs/archive/emuwiz/2026-10-16/feature/gui-history-recovery-view`).
Baseline: freshly fetched `origin/main` at
`6142bcd64cac96a5b0a916e4dd5b4255889f2540`.

The comparison follows user capabilities across the whole historical tip,
including its final DAT exact-resume commit, rather than treating removed GUI
filenames or the historical test count as missing functionality.

| Capability | Current Blue coverage / decision |
| --- | --- |
| Persistent transaction history, counts, entry states, recorded failures and paths | History & Undo plus the native Organisation > Repair History controller already cover these. Keep them. |
| Rollback review, Cancel and explicit approval | Native Repair History already uses its existing confirmation and worker. No additional approval flow or executor is needed. |
| Changed-object and occupied-original refusal; partial or refused outcomes | Existing shared rollback backend and native history tests cover these. Preserve feature dispatch, including optical conversion ordering. |
| Unreadable journals, re-verification, unresolved state and stale-record archiving | Already present in native Repair History. The older DAT adapter is not evidence of a missing Blue safeguard. |
| Finding journal recovery from Blue History | Add a direct handoff to the existing native Repair History controller. It opens the existing journal recovery view without approving Undo or duplicating game-scoped History filtering; the recovery view keeps its own filters. |
| Explaining a pending exact-resume record | Previously said recovery was required but offered no resume and did not explain the authority requirement. Add an explanation of the matching current approved plan requirement and the existing Undo/manual-review alternatives. |
| Historical forward exact resume after restart | A real difference, not equivalent to Undo. The historical `resume_plan_transaction` accepted its frozen approved payload without a current plan. Current DAT exact resume requires an independently supplied current generation and plan digest; ordinary Blue organisation journals do not create that envelope. R2 interrupted Apply permits a fresh reviewed Undo grant, not reuse of a spent Apply approval. Do not recreate the removed executor, infer current approval from its journal, or promise that replanning restores resumability. |

The integration changes navigation and explanation only. Transaction authority,
exact-object checks, conflict refusal, explicit approval, History filtering and
all existing Undo execution paths remain with their current owners. No recovery
engine or historical GUI is restored. Historical forward replay is deliberately
not exposed without the authority required by current APIs.

Validation uses disposable journals and game files, private HOME/XDG/application
roots, and the worktree's `scripts/cargo-iso` target. The rendering regression
covers pending, failed and interrupted resume checkpoints; existing tests cover
completed/rolled-back/contradictory states, confirmation and cancellation,
changed objects, occupied original paths, partial outcomes and unreadable records.
The handoff regression checks that navigation does not instantiate a recovery
controller or approve Undo. Exact test results and independent final review are
recorded in the external task audit ledger.

Production recovery state and preservation refs must remain untouched.
