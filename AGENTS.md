# PilotWeave implementation handoff

This file is the entry point for a coding agent continuing the repository on `main`.

## Read first

Read these files before changing code:

1. `docs/mvp-implementation-spec.md` — normative required behavior and acceptance criteria.
2. `docs/architecture.md` — current and target architecture, trust boundaries, persistence, concurrency, and adapter ownership.
3. `docs/adversarial-review.md` — hostile inputs, failure cases, and required mitigations.
4. `SECURITY.md` — disclosure and sensitive-data rules.
5. Existing implementation in `apps/desktop/src-tauri/src/` and `apps/desktop/web/`.

Do not substitute an earlier conversation, mockup, local patch, or stale design note for the checked-in requirements.

## Branch policy

`main` is the active development branch while PilotWeave is pre-release. Do not create a feature branch or pull request unless the maintainer explicitly requests one. Keep commits reviewable and leave `main` buildable after every completed vertical slice.

## Current repository state

The checked-in implementation is an initial connection/deployment prototype. It currently provides:

- client-neutral Connection and Model records;
- operating-system credential-store integration for provider API keys;
- discovery of VS Code Stable/Insiders profiles, GitHub Copilot CLI, and the GitHub Copilot app;
- deployment preview DTOs;
- VS Code Custom Endpoint projection that preserves non-PilotWeave groups and creates an original backup;
- Copilot CLI provider activation through the user environment on Windows and managed shell files on macOS/Linux;
- a read-only/manual boundary for GitHub Copilot app provider configuration;
- a browser fallback for reviewing the frontend without native writes.

Treat this as a prototype. It does not prove that transactions, ownership, rollback, recovery, installer, login, official-usage, local-usage, or pricing guarantees are complete.

The repository does not yet implement the complete required MVP: workstation installation, account orchestration, official runtime quota and personal Billing, local usage import, price catalog, complete Usage UI, and the deployment hardening defined by the specification.

## Required scope boundary

Implement only `docs/mvp-implementation-spec.md`.

The required milestone does not include:

- organization or enterprise Billing retrieval, administration, or team reporting;
- BYOK-provider invoice APIs or provider-reported actual charges;
- cloud usage synchronization or multi-user cost allocation;
- purchasing, enabling, or changing Copilot overage settings;
- full macOS or Linux one-click installation;
- uploading local usage logs;
- parsing opaque/private Copilot app stores to manufacture usage or authentication support.

Do not add placeholder controls, roadmap sections, schemas, or speculative abstractions for excluded capabilities.

## Non-negotiable constraints

### Harden deployment before building on it

The target deployment contract is normative even where the prototype does not yet satisfy it:

- frontend input is untrusted;
- preview and apply are separate;
- plans are native-held, expiring, and one-shot;
- live target fingerprints are rechecked before writes;
- every writable target is prepared before the first write;
- journal and rollback data are persisted before mutation;
- rollback refuses to overwrite externally changed state;
- frontend-facing errors pass through central redaction;
- invalid primary and last-known-good state enters explicit read-only recovery.

Implement these invariants before installation or login workflows are allowed to trigger configuration deployment.

### Never copy client authentication material

`Sign in and sync` launches and verifies official client flows. It must never read or replicate:

- VS Code SecretStorage;
- browser cookies or browser databases;
- Copilot CLI OAuth or refresh tokens;
- GitHub Copilot app credential stores or opaque authentication databases;
- passwords, passkeys, device secrets, or session cookies.

PilotWeave's GitHub API authorization is a separate least-privilege secret stored under its own operating-system credential-store reference.

### No arbitrary installer execution

- The frontend submits only a backend-generated plan ID.
- Installation strategy kinds, package IDs, repositories, asset policies, executable paths, and arguments are backend-owned allowlisted data.
- Never execute remote command strings, `curl | sh`, `Invoke-Expression`, concatenated `cmd /c`, or equivalent shell pipelines.
- Verify architecture, bounded size, SHA-256 when authoritative metadata exists, and expected Windows publisher before launching a downloaded GitHub Copilot app asset.
- Re-detect the actual component after installation; an exit code is not proof of success.
- Do not automatically uninstall or downgrade applications as rollback.

### Preserve unknown values

Do not convert missing or inaccessible data into zero or success:

- missing cache-write tokens remain unknown;
- inaccessible or organization-paid personal Billing is not zero usage;
- inferred identity is not verified identity;
- ambiguous route is not GitHub official;
- unavailable pricing produces partial estimate coverage;
- partial installation remains partial.

### Do not retain conversation content

Usage persistence may contain only allowlisted metadata. Do not store raw session/debug-log lines, prompts, responses, source code, tool payloads, environment values, cookies, or authentication headers.

### Money uses decimal arithmetic

Use `rust_decimal` or another exact fixed-point representation for credits, prices, rates, and currency. Do not persist or aggregate money with `f32` or `f64`.

## Implementation order

Follow section 15 of `docs/mvp-implementation-spec.md`:

1. deployment hardening and SQLite/domain foundation;
2. environment detection and reviewed installation plans;
3. account orchestration and separate GitHub authorization;
4. official runtime quota/model catalog and personal GitHub Billing snapshots;
5. versioned price catalog and decimal estimate engine;
6. Copilot CLI usage import;
7. VS Code opt-in usage import;
8. GitHub Copilot app safe usage discovery and shared-runtime deduplication;
9. attribution and complete Usage UI;
10. clean Windows end-to-end validation and regression checks.

Do not start with a large static UI mock. Build each vertical slice through domain, persistence, native command, tests, and UI.

## Suggested first slice

Start by implementing:

- native-held deployment plans with TTL and one-shot consumption;
- target fingerprints and stale-preview rejection;
- prepare-all cross-target writes;
- write-ahead deployment journal and rollback manifest;
- central error redaction and read-only state recovery;
- a migration-safe SQLite module;
- a separate `usage.sqlite3` with source, cursor, record, quota, Billing, model alias, price snapshot, and estimate tables;
- exact decimal serialization;
- bounded domain enums and DTOs;
- dashboard states that report unavailable data without fake values;
- regression tests for stale plans, external changes, interrupted apply, rollback refusal, migration/reopen behavior, unique source keys, foreign keys, and decimal round trips.

Keep `state.json` compatibility and current tests intact.

## Coding conventions

- Prefer small modules with one responsibility over expanding `commands.rs` or `domain.rs` indefinitely.
- Keep client-specific paths, schemas, commands, login probes, and usage parsing in adapters.
- Keep runtime quota/model-list handling separate from GitHub Billing endpoint handling.
- Keep price retrieval/parsing separate from usage import.
- Keep raw observations separate from derived attribution and estimates.
- Use typed enums instead of stringly typed statuses.
- Bound externally derived strings, arrays, files, lines, event counts, downloads, query ranges, and page sizes.
- Reject symlinks, junctions/reparse points, and non-regular sensitive source files where required.
- Store timestamps in UTC and render local time only in the frontend.
- Hash session/request identifiers before returning them to the frontend when raw IDs may reveal private context.
- Avoid new dependencies unless they materially improve correctness or platform verification; document why.

## Upstream-source discipline

Before implementing an adapter, verify the current official source listed in section 18 of the specification. Package IDs, API versions, endpoint schemas, release asset names, log paths, and token semantics can change.

A changed upstream schema requires:

- a versioned parser branch;
- a sanitized fixture;
- success and failure tests;
- an explicit unsupported/schema-error state for unknown versions;
- documentation updates when behavior or trust boundaries change.

Do not scrape a web page when an official REST endpoint or release API exists. Do not use undocumented credential data merely because it is locally accessible.

## Testing rules

Tests must use temporary directories, fake process runners, fake signature verifiers, fake credential stores, sanitized local-log fixtures, and local HTTP servers. Tests must not:

- install or uninstall a real application;
- modify real VS Code profiles, registry, environment, shell files, or Copilot sessions;
- read the developer's real home-directory usage data;
- use a real GitHub token;
- depend on the live latest release.

Every bug involving security, idempotence, unit semantics, price selection, or external schema drift needs a regression test.

## Required checks

Run from the repository root before handing off a slice:

```bash
npm run check:web
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

Verify that generated build output, real logs, database files, tokens, and temporary installers are not committed.

## Completion report format

At the end of a slice, report:

```text
Implemented
- concrete native behavior
- persistence/migration changes
- UI behavior and unavailable/error states

Security properties implemented or preserved
- relevant invariants and validation

Tests
- commands run and result
- new fixture coverage

Known blockers inside required scope
- exact upstream/API/platform blocker
- observed evidence
- next implementation step
```

Do not call a slice complete when only types, TODOs, static HTML, or mock data exist.
