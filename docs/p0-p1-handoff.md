# P0 / P1 handoff

Scope: the requested Home setup flow and connected Usage implementation. Development remains on `main`. The worktree already contained deployment/state hardening changes when this work began; those changes were preserved. Validation does not perform real installation, real login, real credential reads or real usage import. The remote main updates through `cbdbb9a` were integrated before independent review and publication.

## Implemented

- Home loads component, account, deployment and usage state, chooses a concrete next action and derives four core steps from current evidence. Usage authorization/opt-in is separate. Selected Connection and qualified user confirmations survive restart.
- Account confirmation retains target stable GitHub ID, time and original per-surface evidence. Verified and user-confirmed states remain different; changed evidence, a conflicting identity, a missing client or expiry invalidates the confirmation.
- All detected writable deployment targets are selected by default; profiles are under review details. New deployment records bind Connection revisions and prepared-output fingerprints for current drift checks. The upstream prepare-all transaction and reviewed recovery flow are preserved; a later external edit cannot be adopted as the expected deployment output. The Copilot app remains a three-step manual setup with qualified completion.
- Navigation is Home / Connections / Usage / More. Native feature modules mount into explicit containers. The unfinished Resources route and obsolete tracked one-off workflows are removed.
- Native Usage connects quota/model RPCs, independent monthly personal Billing, opted-in local CLI/shared-runtime and VS Code imports, versioned price catalogs, decimal estimates, filters, breakdowns, record details, source coverage and import/refresh activity.
- SQLite v3 preserves the earlier schema/data, adds adapter payloads/cursors/jobs/setup preferences/Billing retry state, uses WAL/foreign keys, and retains historical price bindings. A separate current-catalog pointer follows successful refreshes, including A-to-B-to-A price changes, without mutating historical snapshots.
- Browser mode prominently labels preview data and disables native installation, sign-in and usage operations. Its simulated Connection editing remains disposable tab-local data.

## Security properties implemented or preserved

- Existing reviewed deployment plans, stale checks, journal/rollback and read-only Connection recovery remain in place. State and credential context are checked after acquiring the cross-process deployment write lock. Restore still refuses external changes or undiscovered resources. A separately previewed and confirmed Keep current files action can dismiss the bounded pending journal without reading or writing any target or recording deployment success; the UI warns that pending rollback data is discarded. Changed/corrupt journals are rejected.
- Usage uses native-owned roots, fixed read-only methods/URLs, bounded input/process/network work and no client credential-store inspection.
- Only allowlisted metadata persists; fixtures test conversation/auth/environment sentinels against database/WAL bytes.
- Unknown cache buckets, ambiguous routes, model aliases, input semantics, unsupported app data, unavailable Billing and incomplete prices stay explicit.
- Cursor updates and metadata commit atomically. Repeated/rotated/cumulative imports do not duplicate totals or replace newer data with older snapshots.
- Price and monetary arithmetic is exact; counters cross the frontend boundary without JavaScript integer rounding. Concurrent price derivation cannot overwrite newer raw observations or a historical binding.
- Billing fallback is scoped to stable account and month, honors retry time, and preserves the last successful month snapshot through repeated failures.

## Validation

Final verification on 2026-09-10, against the uncommitted worktree on `main@cbdbb9a`:

| Check | Result |
| --- | --- |
| `npm run check:web` | Passed syntax checks and all 15 frontend tests (11 setup/usage/review and 4 retained upstream plan tests) |
| `cargo fmt --all -- --check` | Passed |
| `cargo test --workspace` | Passed 168 unit tests and 6 integration tests |
| `cargo clippy --workspace --all-targets -- -D warnings` | Passed |
| `git diff --check` | Passed after removing one trailing space |
| `node apps/desktop/web/tests/browser-smoke.cjs` | Passed all 12 isolated browser scenarios |
| `npm run build -- --no-bundle` | Passed Windows Release build; produced `target/release/pilotweave.exe` without launching it |

The optional browser test uses a fresh headless Edge profile and in-memory native fixtures. It covers preview routes, account confirmation, reviewed deployment arguments, manual provider completion, quota/Billing refresh, model/session views, source consent/import/clear, price refresh, filters and reviewed interrupted-deployment recovery, external conflicts and missing targets. Home, model and source screenshots were visually inspected. These fixture screenshots do not prove native readiness or real account/usage data.

New regression coverage includes metadata-only persistence, unknown versus zero token buckets, exact decimal and large-counter round trips, incremental/rotated/incomplete sources, cancellation and restart recovery, immutable price bindings under concurrent imports, returning catalog content and delayed older refreshes, account/month-scoped Billing fallback, rate-limit retry times and SQLite v1/v2 migration/reopen, external-conflict recovery retention, unknown/unreadable targets and stale/corrupt dismissal refusal. Changed and untracked paths contain source, documentation and sanitized fixtures; generated output, actual logs/databases, credentials and installers are not included in the handoff files. Commit scope is restricted to the reviewed paths; the generated executable is not included.

The source and calculation contracts, fixture provenance and regression coverage are detailed in [usage-sources.md](usage-sources.md). No live-latest response is a test dependency.

## Known blockers inside the required product scope

| Evidence / limitation | Current behavior | Next step |
| --- | --- | --- |
| CLI shutdown schema has counters but does not establish total-vs-fresh input semantics or CLI/app origin | Report raw counters; normalized input/cost and precise surface attribution remain unavailable | Verify an official semantic guarantee or a supported per-request source, then add versioned fixtures |
| Current VS Code spans may omit cache-write tokens | Keep cache-write/fresh-input/estimate unknown; show covered cache-read rate | Adopt an upstream source/version that exposes the missing counter explicitly |
| No separately established Copilot app usage interface | Unsupported card; shared runtime counted once | Add an adapter only after a stable supported source exists |
| Generic VS Code instrumentation may label custom endpoints `github`; Connection identity is absent | Conservative Unknown routing or explicit BYOK with unknown Connection | Use supported explicit connection evidence; never current-model-name attribution |
| Real Windows/client/account acceptance has not run in this scope | Pre-release; no claim of full MVP or release readiness | P2 clean Windows installation, real official flows and runtime/Billing capability acceptance |

Required Resources synchronization and other remaining full-MVP deployment/installer acceptance obligations remain described by the normative specification. Hiding Resources for the requested initial product flow does not mark that separate work complete.
