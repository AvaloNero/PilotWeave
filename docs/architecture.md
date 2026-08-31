# PilotWeave architecture

## Authority and scope

This document separates the checked-in prototype from the architecture required by `docs/mvp-implementation-spec.md`.

The repository currently contains a basic connection/deployment prototype. The required MVP must first harden that loop, then add environment installation, account orchestration, official usage, local usage, attribution, and pricing. Sections labeled **required** are contracts for implementation, not claims that the current code already satisfies them.

## Product model

PilotWeave is not organized as an application switcher. The source of truth is client-neutral and is projected into concrete client targets.

```text
Connection
  ├─ provider type and protocol
  ├─ endpoint and request headers
  ├─ credential reference
  └─ model catalog
          ↓
Client target
          ↓
Deployment plan
          ↓
Prepared change / rollback snapshot
          ↓
Journaled apply result
```

The required MVP adds three more control loops:

```text
Environment
  discover → preview install → apply allowlisted operations → rediscover

Account
  inspect safe identity evidence → choose target account → launch official flows
  → verify each surface → detect conflicts → deploy shared configuration

Usage
  refresh official quota/Billing + import local observations
  → normalize → deduplicate → attribute route/model
  → bind immutable price snapshot → aggregate with explicit coverage
```

These loops share stable client identities and dashboard summaries, but they do not share secrets or silently reinterpret one another's data.

## Stable identities

Use identifiers rather than display names:

- `installation_owner_id`: this PilotWeave installation's ownership identity.
- `connection_id`: a BYOK connection.
- `client_target_id`: a concrete target such as VS Code Stable default profile, a named profile, CLI environment, or Copilot app installation.
- `deployment_plan_id`: one native-held deployment intent.
- `install_plan_id` and `install_operation_id`: native-held installation intent and operation.
- `login_run_id`: one login orchestration run.
- `(host, login, user_id?)`: a GitHub identity; numeric user ID is stronger evidence where available.
- `usage_source_id`: one observable source and parser contract.
- `usage_record_id`: a stable source-specific identity derived without exposing sensitive raw paths or session IDs.
- `price_snapshot_id`: immutable price data used by an estimate.

Display labels may change without changing these identities. The MVP account-reconciliation boundary is `github.com`; another host is represented as unsupported rather than interpolated into commands or URLs.

## Trust boundaries

### Frontend

The frontend is a presentation and intent layer. All payloads are untrusted.

It may request a component, target, date range, filter, or confirmation. It may not provide:

- executable paths or command strings;
- package IDs or release repositories;
- authoritative install success;
- GitHub account verification;
- route attribution;
- official Billing totals;
- token prices;
- a completed deployment/login/usage status.

The backend enforces all bounds and invariants independently.

### Native state

`state.json` stores compact non-secret application state: connections, credential references, client summaries, deployment records, install/login summaries, settings, and database metadata.

Required behavior:

- schema versioning and explicit migrations;
- atomic replacement appropriate to each platform;
- a validated `state.json.last-good` copy;
- bounded collection and string sizes;
- no API-key or GitHub-token values;
- explicit read-only recovery when primary and last-known-good state are both invalid.

### Credential store

Secrets are separated by purpose:

- BYOK provider API keys;
- PilotWeave's separate least-privilege GitHub personal-usage authorization.

PilotWeave never extracts or reuses client-owned OAuth tokens, cookies, VS Code SecretStorage values, passwords, passkeys, refresh tokens, or private Copilot credential databases.

Credential state is not a Boolean. It must distinguish at least stored, missing, unavailable, locked, and permission denied.

### Usage database

Use a separate transactional `usage.sqlite3` for:

- schema migrations;
- sync runs and source status;
- source cursors and parser versions;
- normalized usage records;
- official quota snapshots;
- personal GitHub Billing snapshots/items;
- model aliases;
- immutable price snapshots/rows;
- stored historical estimates.

Usage-database failure must not disable Connection management. The database stores allowlisted metadata only, never prompt/response/tool/code content or authentication material.

### External processes and network

Process execution and HTTP clients are backend-owned adapters with fixed strategy types, allowlisted hosts/IDs, separate argument arrays, timeouts, response-size limits, redirect policies, and redacted errors.

## Current backend layout

The prototype starts from:

```text
apps/desktop/src-tauri/src/
├── commands.rs
├── domain.rs
├── error.rs
├── secrets.rs
├── state.rs
└── adapters/
    ├── vscode.rs
    ├── copilot_cli.rs
    └── github_app.rs
```

Do not continue expanding `commands.rs` and `domain.rs` indefinitely. Move behavior into focused services and adapters.

## Required target modules

One acceptable layout is:

```text
src/
├── commands/
│   ├── dashboard.rs
│   ├── deployment.rs
│   ├── install.rs
│   ├── login.rs
│   └── usage.rs
├── domain/
│   ├── connection.rs
│   ├── client.rs
│   ├── deployment.rs
│   ├── install.rs
│   ├── account.rs
│   └── usage.rs
├── persistence/
│   ├── state.rs
│   ├── migrations.rs
│   └── usage_db.rs
├── deployment/
│   ├── planner.rs
│   ├── transaction.rs
│   ├── journal.rs
│   ├── rollback.rs
│   └── ownership.rs
├── install/
│   ├── planner.rs
│   ├── runner.rs
│   ├── catalog.rs
│   ├── download.rs
│   └── signature.rs
├── account/
│   ├── orchestrator.rs
│   └── github_auth.rs
├── usage/
│   ├── official_runtime.rs
│   ├── github_billing.rs
│   ├── import.rs
│   ├── normalize.rs
│   ├── attribution.rs
│   ├── pricing.rs
│   └── queries.rs
├── adapters/
│   ├── vscode/
│   ├── copilot_cli/
│   └── github_app/
├── process.rs
├── network.rs
├── redact.rs
└── error.rs
```

The exact filenames may vary, but the responsibility boundaries are required.

## Required deployment lifecycle

The prototype preview/apply path must be hardened to this lifecycle before installation or login workflows rely on it.

### 1. Preview

- Resolve concrete client targets.
- Validate the Connection and stored credential state.
- Capture the connection/state version and each target fingerprint.
- Calculate supported changes without returning secrets.
- Store the complete plan in native memory with a 15-minute TTL.
- Return only a non-secret DTO and `plan_id`.

### 2. Consume

- The frontend submits only `plan_id` plus explicit confirmation.
- Remove the plan from the pending map before application so it is one-shot.
- Reject expired, replayed, wrong-installation, or mismatched plans.
- Revalidate connection version, operation set, plan digest, credential state, and live target fingerprints.

### 3. Prepare all targets

Before the first write:

- read and parse every target;
- reject symlinks, junction/reparse points, non-regular files, oversized files, malformed JSON5, incomplete shell blocks, and ownership conflicts;
- build before/after snapshots for every writable target;
- identify semantic no-ops without rewriting user formatting.

### 4. Journal

Persist:

- Prepared deployment records;
- a private rollback manifest containing bounded before/after data or references;
- Applying state before mutation.

### 5. Apply and rollback

- Recheck the relevant before-state immediately before each target write.
- Apply targets in deterministic order.
- On failure, roll back successful prior targets in reverse order.
- Refuse rollback if a target changed externally after PilotWeave's write.
- Use platform-appropriate atomic replacement; Windows must not use delete-then-rename.

### 6. Commit audit state

- Persist Applied, Skipped, Failed, or RolledBack results.
- If audit persistence fails after client writes, attempt to restore applied targets.
- On startup, surface incomplete journal entries as Interrupted and offer safe recovery information.

## Environment installation architecture

### Observation model

A surface may require more than one component:

- VS Code Copilot: VS Code installation plus an available Copilot capability/extension.
- Copilot CLI: official CLI executable/runtime.
- GitHub Copilot app: official desktop installation.

Observed states include installed, missing, broken, unsupported, unknown, and update-required. A shared package may satisfy more than one surface and must be deduplicated in the plan.

### Install plan

The backend owns an immutable plan containing:

- platform and architecture;
- observed component fingerprints;
- exact strategy and allowlisted package/repository/asset identity;
- fixed executable and argument vector;
- source and expected publisher;
- download limits, digest/signature policy, elevation, dependencies, and expected post-install observation;
- plan digest, creation time, expiry, and one-shot state.

The frontend never supplies a shell command.

### Supported MVP path

Windows 11 x64 is the primary acceptance platform. Prefer official package-manager IDs where identity can be fixed and verified. For direct GitHub Copilot app assets, use the official release API and strict asset-name/architecture policy; validate bounded size, redirects, digest where authoritative data exists, and Authenticode publisher before launch.

An exit code is not success. Re-run discovery and verify the component identity/version. Bulk installation reports one result per component; partial remains partial. No automatic uninstall or downgrade is performed as rollback.

## Account architecture

### Safe observations

Each client adapter returns non-secret evidence:

```text
Verified(login, host, user_id?, evidence)
Inferred(login, host, evidence)
ActionRequired
Unknown(reason)
Unsupported(reason)
Conflict(details)
```

A login display name alone is insufficient. Inferred identity requires confirmation. Conflicting verified identities block completion.

### Login orchestration

`Sign in and sync` means:

1. discover installed/ready clients;
2. choose a target GitHub identity from a verified observation or explicit user choice;
3. launch each client's official browser/device/application sign-in path;
4. poll or request safe re-observation;
5. verify identity consistency;
6. only after consistency, offer configuration deployment.

PilotWeave does not silently sign out a client and never copies authentication material between clients.

### PilotWeave GitHub authorization

Official personal usage/Billing retrieval uses PilotWeave-owned authorization, separate from all client sign-ins. Store the token in the OS credential store and persist only host, login, scopes/capabilities, last validation, and secret reference.

## Official usage architecture

Keep three datasets separate:

1. Copilot runtime quota/entitlement observations.
2. Authoritative personal GitHub Billing usage/amounts.
3. Locally observed model/token usage.

They may be displayed together but never substituted for one another.

Each official snapshot stores source, account, covered period, fetched time, units, status, schema/parser version, and raw unknown fields only when safe and bounded. Unauthorized, forbidden, unsupported, network error, schema error, not covered, successful empty, stale, and numeric zero are distinct.

AI credits and legacy premium requests are different units and must not be summed. Personal Billing does not claim organization/enterprise coverage.

## Local usage import architecture

### Import lifecycle

1. Discover bounded candidate files or supported runtime endpoints.
2. Reject symlinks/reparse points and non-regular files.
3. Compare source identity, size, modification time, parser version, and stored cursor.
4. Read only the bounded new range when possible.
5. Tolerate an incomplete actively written final JSONL line.
6. Parse only model, session/request identity, timestamps, token counters, and explicit route/cost metadata.
7. Convert cumulative snapshots to replacement/delta semantics before persistence.
8. Normalize tokens without unsigned underflow.
9. Attribute route only when supported by evidence.
10. Bind an immutable price snapshot only when token semantics and price selection are adequate.
11. Batch-upsert records and advance the cursor in one database transaction.
12. Record source-level errors without blocking independent sources.

Stable unique keys make repeated scans idempotent. Truncation, rotation, schema drift, and parser-version changes enter explicit recovery paths.

### Privacy

Do not persist raw log lines, prompts, responses, tool payloads, source code, environment values, cookies, headers, or authentication data. Hash raw session/request identifiers before frontend exposure where they may reveal private context.

## Token semantics

Canonical fields include:

- source-reported total input, when present;
- fresh input, when derivable;
- cache read;
- cache write;
- output;
- an explicit semantic enum.

Suggested semantic enum:

```text
FreshOnly
TotalIncludesCacheRead
TotalIncludesCacheReadAndWrite
SeparateBucketsWithNoTotal
Unknown
```

For known semantics:

```text
normalized total input = fresh input + cache read + cache write
cache hit rate = cache read / normalized total input
```

Aggregate token counts first, then calculate percentages. Unknown or missing buckets remain unknown; they do not become zero merely to complete a chart.

## Route attribution

Route is:

```text
OfficialGithub
ByokConnection(connection_id)
Unknown
```

Evidence priority:

1. explicit route/provider from the supported source;
2. stable PilotWeave connection identity in the request/session;
3. unique deployment timeline + client + model match, marked inferred;
4. unknown.

A model name alone is insufficient when the model may be available through both GitHub and BYOK. Unknown records are never assigned to the currently active connection for convenience.

## Model identity

Keep both `raw_model` and optional `canonical_model_id`. Model aliases are exact, source-scoped records. Ambiguous aliases remain unresolved. Re-normalization may update canonical fields but never rewrites raw identity.

The model list is the union of configured BYOK models, official runtime models, local token observations, and official Billing items. A model remains visible when one metric family is unavailable.

## Pricing and reporting

Never collapse these into one value:

```text
official_net_amount_usd        authoritative personal GitHub Billing amount
estimated_api_equivalent_usd   local tokens × immutable price snapshot
```

Provider invoice APIs are outside the required milestone.

A price snapshot preserves provider, canonical model, source, source version, currency, effective/fetched times, threshold/tier, and all available input/cache/output rates. Use exact decimal arithmetic.

An estimate is available only when token semantics are known, required rates are available or explicitly not applicable, and the appropriate pricing tier can be selected. Unknown price is null, not zero. Historical records remain bound to the price snapshot used at calculation time; a current-price comparison is a separate query.

## Adapter ownership

### VS Code

Adapters own installation/profile discovery, safe account evidence, supported usage sources, and `chatLanguageModels.json` projection.

Managed configuration must include installation-owner and connection identity. A similarly named group or a marker created by another PilotWeave installation is not owned. Semantic no-op must not rewrite JSON5 comments/formatting.

### Copilot CLI

Separate adapters/services own environment projection, executable discovery/version verification, official sign-in launch/evidence, official runtime quota/model data, and local usage import.

PilotWeave refuses to overwrite unmanaged or foreign-owner provider overrides. Unix changes across managed environment and shell files form one prepared transaction.

### GitHub Copilot app

Provider projection remains detection/manual guidance until a stable external interface exists. PilotWeave may launch official sign-in and consume supported non-secret identity/usage interfaces. Missing support remains unknown/partial; it does not justify parsing private credential state.

CLI and app may share runtime/session data. Deduplication must use physical source identity so the same record is not counted twice.

## Command boundary

Tauri commands are thin, validated entry points. Long-running install, login, and usage sync operations expose:

- backend-generated run ID;
- bounded progress events;
- cancellation;
- per-step status;
- final persisted summary;
- redacted diagnostics.

List/detail queries enforce backend maximum ranges, page sizes, and sort keys. Frontend input cannot cause an unbounded directory walk, database scan, process launch, or network allocation.

## Concurrency and failure isolation

- Only one environment-mutating install run executes at a time.
- Only one login-sync run targets the same client set/account at a time.
- Deployment uses its own transaction lock.
- Usage imports serialize per physical source; independent sources may run concurrently within a bounded worker limit.
- Runtime quota and GitHub Billing refresh independently.
- Usage-database failure does not disable Connection management.
- Install/login partial failure does not rewrite deployment history.
- Shutdown cancels or marks active runs interrupted; startup exposes recovery status.

## Architecture invariants

The required MVP is architecturally complete only when:

- backend-owned plans, commands, identities, prices, and attribution cannot be replaced by frontend payloads;
- install success is verified through rediscovery;
- login orchestration never copies client credentials;
- conflicting verified identities block completion;
- official GitHub monetary values and local API-equivalent estimates remain distinct;
- unavailable, unknown, partial, stale, conflict, action-required, and zero remain distinct;
- local import is bounded, incremental, idempotent, deduplicated, and content-minimizing;
- historical price snapshots are immutable;
- unknown token semantics or price data do not become zero;
- deployment validation, ownership, journaling, rollback, redaction, and recovery satisfy the required contract.
