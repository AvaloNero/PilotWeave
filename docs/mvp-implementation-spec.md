# PilotWeave required MVP implementation specification

## 1. Status and interpretation

This document is the normative implementation contract for PilotWeave's required pre-release milestone. Everything described as **must**, **required**, or **shall** is required scope. This document intentionally contains no post-MVP roadmap.

Where an upstream GitHub, VS Code, package-manager, model-price, or client-log interface changes, the implementation must prefer a safe explicit Unsupported/SchemaError state over undocumented credential extraction, brittle private-state mutation, fabricated data, or silent fallback.

The checked-in code is an initial Connection/deployment prototype. It does not yet satisfy this complete specification.

## 2. Product goal

A user on a clean Windows 11 x64 machine must be able to:

1. open PilotWeave and see which Copilot surfaces and required components are installed;
2. install one selected missing component or all missing components from a reviewed native plan;
3. use official sign-in flows to make the three Copilot surfaces use the same GitHub account, or clearly resolve/accept unsupported and conflicting states;
4. configure a Connection/model catalog once and deploy supported configuration to the relevant clients;
5. view official Copilot quota/personal GitHub usage when authoritative sources expose it;
6. view locally observable usage for official and BYOK models, including input, output, cache-read, cache-write, cache-hit rate, attribution confidence, and coverage;
7. view API-equivalent cost estimates calculated with immutable versioned decimal price snapshots, separately from GitHub's authoritative monetary amounts;
8. understand exactly which values are verified, inferred, estimated, stale, partial, unavailable, or unknown;
9. recover safely from failed/interrupted configuration writes without losing unrelated user configuration.

## 3. Required scope

### 3.1 Copilot surfaces

The product manages these logical surfaces:

- **VS Code Copilot**: VS Code Stable and Insiders, including concrete profiles and the supported GitHub Copilot capability/extension state.
- **GitHub Copilot CLI**: the official `copilot` CLI/runtime.
- **GitHub Copilot app**: the official desktop application.

A logical surface may share a physical package/runtime/source with another surface. The system must model both logical surfaces and physical components so installation and usage are not duplicated.

### 3.2 Required capabilities

- environment/component discovery;
- reviewed installation plans;
- install selected missing component;
- install all missing components;
- post-install rediscovery and verification;
- official sign-in orchestration;
- same-account verification and conflict handling;
- Connection/model configuration and supported deployment;
- MCP, Skills, and Instructions synchronization where public/stable client paths are supported;
- official Copilot runtime quota/model data where safely available;
- authoritative personal GitHub AI-credit or legacy premium-request usage where the user's authorization and GitHub endpoint support it;
- local token-usage import from supported observable sources;
- official/BYOK/unknown route attribution with confidence;
- immutable price catalog and API-equivalent estimate calculation;
- Usage UI, source coverage, errors, and audit history;
- deployment/installer/login/usage security and recovery behavior specified below.

## 4. Explicitly excluded capabilities

Do not implement, scaffold, advertise, or add placeholder controls for:

- organization or enterprise Billing retrieval, administration, seat management, or team reporting;
- BYOK provider invoice APIs or provider-reported actual charges;
- cloud synchronization of local usage data;
- multi-user cost allocation;
- purchasing, enabling, or modifying Copilot overage settings;
- full macOS or Linux one-click installation;
- automatic upload of local logs;
- parsing private/opaque Copilot app credential or authentication databases;
- copying any client OAuth token, refresh token, cookie, SecretStorage value, password, passkey, or session secret between clients;
- automatically signing a client out;
- force-writing undocumented GitHub Copilot app provider state.

Existing macOS/Linux discovery and supported deployment behavior must not regress, but Windows 11 x64 is the required clean-machine installation acceptance platform.

## 5. Cross-cutting state semantics

The backend and frontend must preserve typed states instead of collapsing them to Booleans.

At minimum, use context-appropriate variants for:

```text
Available / Installed / Ready
Missing
Broken
UpdateRequired
Unsupported
Unknown
ActionRequired
InProgress
Applied / Completed
Skipped
Partial
Failed
Canceled
Interrupted
Verified
Inferred
UserConfirmed
Conflict
Unauthorized
Forbidden
NotCovered
NetworkError
SchemaError
Stale
SuccessfulEmpty
```

Numeric zero is a value, not a replacement for Missing, Unknown, Unsupported, NotCovered, or failed retrieval.

Every displayed official or local data card must expose:

- data source;
- covered account/client/source;
- covered time range, if relevant;
- fetched/imported timestamp;
- freshness/staleness;
- coverage/quality limitations;
- typed error state when unavailable.

## 6. Deployment hardening

The existing Connection deployment path must be brought to this contract before install/login orchestration can invoke it.

### 6.1 Backend validation

Rust validates all Connection/model/header/target inputs independently of the frontend.

Required checks include:

- bounded Connection count, model count, header count, string lengths, token limits, and serialized target sizes;
- valid HTTP(S) endpoint with host;
- remote plaintext HTTP rejected except explicitly allowed loopback addresses;
- username/password and fragments rejected in endpoint URLs;
- header names validated as HTTP tokens and unique case-insensitively;
- CR/LF rejected from header names/values;
- known authentication headers may not contain a literal secret in persisted Connection state; use the supported secret placeholder/reference;
- at least one enabled model and unique normalized model IDs;
- safe integer ranges for context/output limits;
- collection-level uniqueness for Connection IDs and secret references;
- invalid loaded state rejected or quarantined rather than merely normalized.

### 6.2 Preview plan

A deployment preview must:

- resolve concrete targets;
- validate the current Connection and credential state;
- read/parse all target state needed to build the preview;
- capture connection/state version and per-target identity/fingerprint;
- calculate semantic no-op versus change;
- retain secret-bearing/full plan data only in native memory;
- return a non-secret summary plus backend-generated `plan_id`;
- expire after 15 minutes;
- be consumable once.

The frontend may not send a serialized deployment operation list to apply.

### 6.3 Apply consistency

Apply receives only `plan_id` and explicit confirmation. It must:

- remove the plan from the pending map before execution;
- reject missing, expired, replayed, wrong-owner, or already-consumed plans;
- revalidate the Connection/state version, credential state, operation digest, and live target fingerprints;
- return PlanChanged/Stale rather than recomputing and executing a materially different plan.

### 6.4 Prepare-all transaction

Before the first target write:

- resolve and validate every target;
- reject symlinks, unsupported junctions/reparse points, non-regular files, oversized files, malformed JSON5, incomplete managed shell blocks, and ownership conflicts;
- create bounded before/after snapshots or private snapshot references;
- prove all operations are writable/prepared;
- persist Prepared deployment records and rollback manifest;
- persist Applying state.

### 6.5 Apply and rollback

- Apply in deterministic order.
- Recheck each live before-state immediately before its write.
- If a later target fails, roll back successful prior targets in reverse order.
- Rollback executes only when live state matches the expected post-write fingerprint; external changes cause rollback refusal, not overwrite.
- Windows replacement must use a safe replace/write-through primitive rather than remove-then-rename.
- Unix private files use owner-only permissions; existing unrelated file modes are preserved where appropriate.
- Semantic no-op must not rewrite JSON5 comments/formatting or create needless backups.

### 6.6 Audit and recovery

- Persist final Applied, Skipped, Failed, RolledBack, Partial, or Interrupted status.
- If final audit persistence fails after client writes, attempt restoration and report uncertain side effects.
- On startup, incomplete journal entries become visible Interrupted operations with safe recovery information.
- Keep an original VS Code backup distinct from per-deployment snapshots and `state.json.last-good`.
- If primary and last-known-good application state are invalid, open a read-only recovery dashboard instead of panicking or silently creating empty writable state.
- All frontend-facing diagnostics pass through centralized secret redaction.

### 6.7 Ownership

Managed targets require both persisted ownership evidence and on-target markers where possible.

At minimum:

- an installation-specific owner ID;
- Connection ID;
- concrete target identity;
- last deployed fingerprint/version.

A marker string alone does not prove ownership. Foreign-owner, unmanaged, legacy, and ambiguous state must be Conflict/ActionRequired.

### 6.8 Deletion

Expose two explicit operations:

- **Revoke from supported targets and delete**: reviewed transactional revocation followed by state/secret cleanup.
- **Stop managing/detach only**: removes PilotWeave state without claiming to clean existing client configuration.

Secret-cleanup warnings must not make a completed state deletion look wholly unsuccessful. Orphan-secret cleanup must not expose secret values.

## 7. Environment discovery

### 7.1 Component model

Model logical surfaces separately from physical components.

Suggested component kinds:

```text
VsCodeStable
VsCodeInsiders
VsCodeCopilotCapability(profile/installation)
CopilotRuntimeOrCli
GitHubCopilotApp
PackageManager(WinGet)
NodeNpmFallback
PowerShellRequirement
```

Each observation includes:

- stable component ID;
- logical surfaces satisfied;
- path/source/product identity;
- version and architecture when available;
- Installed/Missing/Broken/Unsupported/Unknown/UpdateRequired status;
- verification evidence and timestamp;
- safe diagnostic reason.

PATH presence alone is not sufficient for identity. Where practical, resolve a regular executable, invoke a bounded version command with timeout, and validate output/product identity without inheriting PilotWeave secrets.

### 7.2 Refresh behavior

- Discovery runs at startup and on explicit refresh.
- Slow probes execute outside the main state mutex.
- Independent probe failures do not suppress other component observations.
- Results are bounded and redacted.
- Installer completion always triggers fresh discovery rather than mutating observed status optimistically.

## 8. Installation plans

### 8.1 User actions

The Clients page must expose:

- `Install` for an individual missing/broken supported component;
- `Install all missing` for all required supported components;
- plan preview before execution;
- per-operation progress and final result;
- re-detection after completion.

### 8.2 Backend-owned allowlist

The frontend submits requested component IDs only. The backend selects from a compiled/versioned allowlist containing:

- strategy kind;
- exact package ID/source or official repository/asset policy;
- fixed executable resolution policy;
- fixed separate argument vector;
- supported platform/architecture;
- expected publisher/product;
- maximum download size;
- redirect policy;
- digest/signature requirements;
- elevation behavior;
- dependencies;
- expected post-install observation.

Remote data may provide release facts, never executable commands or arbitrary arguments.

Prohibited execution includes shell-concatenated command strings, `curl | sh`, `Invoke-Expression`, and user/remote-controlled `cmd /c` or PowerShell snippets.

### 8.3 Plan semantics

An install plan is native-held, immutable, one-shot, and expires after 15 minutes. Its digest includes:

- platform and architecture;
- requested logical surfaces;
- observed component fingerprints;
- deduplicated physical operations;
- package/repository/asset identity;
- dependencies;
- publisher/digest/size policy;
- elevation requirement.

Apply repeats environment and remote-asset validation. Changed observation or selected asset invalidates the plan.

### 8.4 Windows installation strategies

Use only current official sources verified at implementation time.

Supported strategy classes may include:

- exact WinGet package ID with noninteractive/accept-source terms only where appropriate and previewed;
- official VS Code extension installation through a verified VS Code executable and exact extension ID when the capability is not built in;
- official GitHub release asset download for the Copilot app when package-manager coverage is unsuitable;
- official npm package only as a documented fallback for the CLI/runtime when Node/npm identity is verified.

Do not assume CLI and desktop app require separate physical installation. If the official package/runtime satisfies both, deduplicate it and update both logical observations after rediscovery.

### 8.5 Direct asset verification

For a downloaded installer:

- source repository must be allowlisted;
- select by strict product/platform/architecture/name policy;
- inspect each redirect and remain in documented trusted delivery hosts;
- require HTTPS without downgrade;
- cap redirect count;
- enforce Content-Length when present and streamed byte limit always;
- save to a private temporary path;
- calculate SHA-256;
- compare with authoritative digest metadata when available;
- verify valid Authenticode signature and expected Windows publisher;
- launch the exact verified path;
- delete temporary installer after completion/cancellation when safe.

If an authoritative digest is not available, the preview and result must state that digest provenance is unavailable; publisher validation remains required.

### 8.6 Results

Each physical operation and logical component gets a result:

```text
CompletedAndVerified
ProcessSucceededVerificationFailed
SkippedAlreadyReady
SkippedDependencyFailed
Canceled
ElevationDenied
Failed
Unsupported
```

Bulk operation overall status is derived without hiding partial results. Independent operations may continue after a failure; dependent ones are explicitly skipped.

Do not uninstall/downgrade software or delete user data as rollback.

## 9. Account discovery and sign-in orchestration

### 9.1 Meaning of “Sign in and sync”

It means orchestrating official client flows and verifying a common GitHub identity. It never means copying login tokens.

### 9.2 Account observation

Each client adapter reports one of:

```text
Verified(identity, evidence)
Inferred(identity, evidence)
ActionRequired(reason)
Unknown(reason)
Unsupported(reason)
Conflict(details)
```

`identity` includes host, login, optional stable numeric user ID, and optional avatar URL from an authoritative safe source. Display name alone is insufficient.

Evidence and confidence must be visible in UI. Inferred identity requires explicit user confirmation before it can become the target account. Another GitHub host is Unsupported for the MVP workflow.

### 9.3 Target account selection

- Prefer one unambiguous verified `github.com` identity.
- If multiple clients have the same verified identity, select it automatically and show evidence.
- If no verified identity exists but one inferred identity exists, ask for confirmation.
- If verified identities conflict, block automatic completion and show per-client account state.
- Do not silently sign any client out or overwrite client auth state.

### 9.4 Login run

A login run has a backend-generated ID, target identity, selected clients, per-step status, progress events, cancellation, timeout, and final persisted summary.

Per client:

1. verify installation/readiness;
2. launch a fixed official browser/device/application sign-in path owned by the adapter;
3. report ActionRequired with concise user instruction;
4. poll/recheck only through safe supported account observations;
5. stop on timeout/cancellation;
6. compare observed host/login/user ID to target;
7. report Verified, Conflict, Unknown, Unsupported, or Failed.

The frontend cannot supply an executable, URI, command ID, or host to interpolate.

### 9.5 Configuration sync after login

Only after selected clients have compatible identity states should PilotWeave offer to:

- deploy the chosen Connection/model configuration to writable supported targets;
- present manual provider instructions for the GitHub Copilot app while its adapter is read-only;
- sync supported MCP/Skills/Instructions resources;
- show a final verification summary.

Login success itself does not authorize configuration writes; use the normal deployment preview/confirmation lifecycle.

### 9.6 Separate PilotWeave GitHub authorization

Official personal usage/Billing retrieval uses a PilotWeave-owned least-privilege authorization, separate from client login.

Required behavior:

- store token only in OS credential store;
- persist secret reference, host, login, safe capability/scope summary, validation time, and status;
- validate identity through an official API;
- never reuse/extract a client token;
- allow clear/re-authorize;
- distinguish Missing, Unauthorized, Forbidden, Unavailable, and InsufficientPermission;
- redact token from every error/log/event.

A registered OAuth/device flow may replace manual token entry only when implemented with the same separation and least-privilege rules.

## 10. Shared resources

Within required scope, PilotWeave manages only publicly supported filesystem/configuration resources that can be previewed and rolled back safely:

- MCP configuration;
- Skills;
- repository/user Instructions or supported instruction files.

Requirements:

- client-neutral resource record plus per-client binding;
- detect actual supported target paths and precedence;
- preserve unrelated/unmanaged entries;
- preview semantic changes;
- use the normal ownership, fingerprint, journal, and rollback model;
- report Unsupported/Manual where a surface has no stable writable interface;
- never synchronize credentials embedded in an MCP config without explicit secret-reference handling;
- do not fabricate “all clients synced” when one target is manual or unsupported.

## 11. Official Copilot usage

### 11.1 Separate official sources

Do not combine:

1. Copilot runtime quota/entitlement/model observations.
2. Authoritative personal GitHub Billing/usage amounts and quantities.
3. Local token observations.

The UI can correlate them but each retains its own source, unit, period, status, and timestamp.

### 11.2 Runtime quota/model observation

Use a current supported official runtime/client interface verified during implementation. Do not scrape a web dashboard or read authentication secrets.

Persist a versioned snapshot with:

- account identity when safely provided;
- client/runtime version and adapter/parser version;
- fetched time;
- quota/entitlement entries as bounded key/value records;
- model catalog entries and availability metadata;
- reset/period values only when authoritative;
- unknown safe fields when useful for forward compatibility;
- typed status/error.

Support unlimited/absent/zero/exhausted distinctly. On refresh failure, keep the last successful snapshot as Stale.

### 11.3 Personal GitHub Billing/usage

Use current official GitHub personal endpoints and API versions verified at implementation time. The implementation must not assume that every account/plan exposes every field.

Persist snapshots/items containing:

- GitHub account identity;
- endpoint family and API version;
- covered period;
- fetched time;
- billing mode/product/SKU/model when provided;
- original quantity and unit;
- authoritative gross/discount/net amount and currency when provided;
- allowance/remaining/reset only when authoritative;
- typed coverage/status/error.

AI credits and legacy premium requests are incompatible units and are never summed. Personal usage is not presented as organization/enterprise coverage. Organization-paid data that the personal endpoint cannot see is NotCovered/Partial, not zero.

### 11.4 Refresh

- explicit refresh button;
- optional bounded refresh on page entry with cooldown;
- independent runtime and Billing refresh jobs;
- cancellation/timeouts/retry-after handling;
- last successful snapshot remains visible as stale when refresh fails;
- raw error bodies never pass directly to frontend.

## 12. Local usage collection

### 12.1 Opt-in and privacy

Usage import is local. Enabling any client debug/agent log that changes client behavior requires explicit opt-in explaining:

- which client setting/file is affected;
- what metadata PilotWeave reads;
- that raw conversation content may coexist in the source file;
- that PilotWeave persists only allowlisted numeric/identity metadata;
- how to disable and clear PilotWeave's imported records.

PilotWeave must never persist raw lines, prompts, responses, tool calls/results, source code, environment values, cookies, headers, or authentication material.

### 12.2 Source model

Each source record includes:

- physical source ID and logical surfaces it may serve;
- path/runtime identity without unnecessarily exposing raw private paths;
- parser kind/version;
- enabled/disabled status;
- last scan, last success, cursor, file identity, size/mtime evidence;
- coverage start/end;
- typed source error.

CLI and Copilot app may share a physical runtime/session source. Import each physical record once and associate surfaces as metadata; never duplicate token totals.

### 12.3 Import bounds

Backend-enforced limits include:

- allowed roots;
- maximum directory depth;
- maximum candidate files;
- maximum file size;
- maximum line/event size;
- maximum events per file/run;
- maximum total bytes and runtime per sync;
- cancellation;
- database transaction batch size.

Reject symlinks/reparse points and non-regular source files. Do not follow arbitrary user-controlled paths supplied by the frontend.

### 12.4 Incremental/idempotent parsing

- Store source identity, size, modification evidence, cursor, and parser version.
- Read bounded new ranges where supported.
- Tolerate one incomplete actively written final JSONL line without advancing past it.
- Detect truncation, replacement, rotation, and parser-version changes.
- Use stable unique record keys.
- Cumulative session snapshots replace/upsert previous values for the same stable session+model identity.
- Per-request delta events append/upsert by stable request identity.
- Cursor advancement and record upserts occur in one SQLite transaction.
- Repeated scans produce no duplicate totals.

### 12.5 Canonical usage record

A record should support:

```text
usage_record_id
physical_source_id
logical_client_surfaces[]
session/request hash
raw_model
canonical_model_id?
route
attribution_confidence
input_reported?
fresh_input?
cache_read?
cache_write?
output?
input_semantics
started_at / finished_at?
source_created_at
imported_at
price_snapshot_id?
estimate fields?
quality flags[]
```

Raw source IDs that can reveal paths/private context are hashed before frontend exposure.

### 12.6 Token semantics

Use an explicit enum such as:

```text
FreshOnly
TotalIncludesCacheRead
TotalIncludesCacheReadAndWrite
SeparateBucketsWithNoTotal
Unknown
```

For known semantics:

```text
normalizedTotalInput = freshInput + cacheRead + cacheWrite
cacheHitRate = cacheRead / normalizedTotalInput
```

If the source reports total input including cache, derive fresh input only with checked arithmetic. Inconsistent counters produce a quality error/unknown derived value, never unsigned underflow.

Aggregate token counts first, then calculate cache-hit rate. Do not average per-session percentages.

Missing cache-write or cache-read fields remain null/unknown. Zero is stored only when the source explicitly reports or semantically establishes zero.

### 12.7 Supported source delivery

The required implementation must support:

- Copilot CLI local session usage when the current supported session schema exposes model/token metrics;
- VS Code Copilot usage from a documented/safely observable opt-in source when available, with parser versioning and privacy filtering;
- GitHub Copilot app safe usage discovery when a supported source exists;
- explicit Unsupported/Unavailable/Partial coverage when an app source is unavailable;
- physical-source deduplication between CLI and app.

Do not parse private credential stores or fabricate app usage from unrelated totals.

## 13. Model identity and route attribution

### 13.1 Model identity

Keep `raw_model` unchanged. Map it to optional canonical identity through exact, versioned, source-scoped aliases.

An alias record includes:

- source/parser scope;
- raw value;
- canonical provider/model ID;
- effective/version metadata;
- confidence/status.

Ambiguous aliases remain unresolved. Re-normalization may update canonical derived fields but never rewrites raw identity.

The Usage model list is the union of:

- locally observed models;
- official runtime models;
- official Billing item models/SKUs where meaningful;
- configured BYOK models.

A model remains visible when one metric family is unavailable.

### 13.2 Route

Route variants:

```text
OfficialGithub
ByokConnection(connection_id)
Unknown
```

Attribution confidence/evidence:

```text
Explicit
ExactConnectionIdentity
UniqueTimelineInference
Unknown
```

Evidence priority:

1. explicit supported route/provider/connection data from the source;
2. stable PilotWeave connection identity embedded in a managed runtime/session;
3. unique match against deployment timeline, concrete target, time interval, and model, labeled inferred;
4. Unknown.

Model name alone is insufficient when the same model can be official and BYOK. Current active Connection alone is never used to assign historical records.

## 14. Price catalog and estimates

### 14.1 Separate monetary concepts

Keep separate fields and UI:

```text
official_net_amount_usd        authoritative GitHub personal Billing amount
estimated_api_equivalent_usd   local tokens multiplied by model API-equivalent rates
```

Never add them together or label the estimate as “spent,” “charged,” or “GitHub cost.” Provider actual invoices are excluded.

### 14.2 Exact arithmetic

Use `rust_decimal` or equivalent exact fixed-point arithmetic for all prices, quantities involving credits, rates, and amounts. Persist canonical decimal strings or scaled integers. Do not use binary float for money.

### 14.3 Immutable price snapshots

A price snapshot includes:

- snapshot ID;
- source and source version;
- fetched/effective time;
- currency;
- provider and canonical model;
- release/status metadata;
- context threshold/tier when applicable;
- input rate per million;
- cached-input/cache-read rate per million;
- cache-write rate per million or explicit NotApplicable;
- output rate per million;
- provenance and parser version.

Price data must come from current official provider/GitHub documentation or an explicit user-configured rate with source label. Remote price input is parsed/validated and cannot supply executable content.

Do not mutate a snapshot. Historical estimates retain their snapshot ID. A current-price comparison is a separate derived query.

### 14.4 Formula

For a record with known semantics/rates:

```text
estimate = freshInput / 1_000_000 × inputRate
         + cacheRead / 1_000_000 × cacheReadRate
         + cacheWrite / 1_000_000 × cacheWriteRate
         + output / 1_000_000 × outputRate
```

Rates marked NotApplicable contribute explicit zero; Unknown rates make the estimate incomplete/unavailable.

Long-context/tier pricing is selected only when the source retains enough per-request input evidence. Aggregate-only ambiguity must be labeled partial/unavailable or calculated under an explicit documented conservative rule; never silently choose the cheaper tier.

### 14.5 Estimate coverage

Every aggregate estimate includes:

- priced token/request count;
- unpriced token/request count;
- unresolved model count;
- unknown token-semantics count;
- pricing snapshot/source/version;
- coverage percentage or explicit non-computable state.

## 15. Persistence

### 15.1 `state.json`

Persist compact operational state only:

- schema version;
- installation owner ID;
- Connections and credential references;
- client target summaries;
- deployment audit summaries and private snapshot references;
- compact install/login run summaries;
- GitHub authorization metadata/secret reference;
- usage settings and database schema/version metadata.

Do not store provider API keys, GitHub tokens, client tokens/cookies, raw usage content, or full unbounded installer output.

### 15.2 `usage.sqlite3`

Minimum logical tables:

- `schema_migrations`;
- `usage_sources`;
- `usage_sync_runs`;
- `usage_source_cursors`;
- `usage_records`;
- `usage_record_surfaces` where needed for shared-source metadata;
- `official_quota_snapshots` and `official_quota_items`;
- `github_billing_snapshots` and `github_billing_items`;
- `model_aliases`;
- `price_snapshots` and `price_rows`;
- `usage_estimates` or immutable estimate fields tied to records/snapshots.

Requirements:

- transactional migrations;
- foreign keys enabled and tested;
- unique source-record keys for idempotence;
- exact decimal round trips;
- bounded query ranges/page sizes;
- UTC timestamps;
- no raw log lines or credentials;
- database failure isolated from Connection management;
- read-only/error mode on failed migration rather than destructive recreation.

## 16. Native command/API contract

Names may vary, but the backend must expose equivalent typed operations:

```text
get_dashboard
refresh_environment
create_install_plan(component_ids | install_all_missing)
apply_install_plan(plan_id)
cancel_install_run(run_id)
get_install_run(run_id)
inspect_accounts
create_login_run(target_account, client_ids)
continue_or_refresh_login_run(run_id)
cancel_login_run(run_id)
set_or_clear_github_authorization
refresh_official_runtime_usage
refresh_personal_github_usage
sync_local_usage(source_ids | all_enabled)
get_usage_summary(range, filters)
get_usage_models(range, filters, page)
get_usage_sessions(range, filters, page)
get_usage_sources
set_usage_source_enabled
refresh_price_catalog
get_price_catalog
create_deployment_plan
apply_deployment_plan
rollback_deployment
```

Long-running operations emit bounded redacted progress events and persist final summaries. Commands enforce maximum date ranges, page sizes, filters, and concurrency. No frontend command can trigger an arbitrary path scan, URL fetch, process, or executable argument.

## 17. UI requirements

Primary navigation:

```text
Overview
Connections
Clients
Resources
Usage
Activity
Settings
```

### 17.1 Overview

Show real native data only:

- installed/ready/missing surface count;
- account consistency summary;
- Connection/client deployment drift;
- official usage refresh status;
- local usage coverage/status;
- recent install/login/deployment/usage activity;
- primary actions: Set up this computer, Add Connection, Sign in and sync, View Usage, Preview deployment.

No static success rate, resource count, or token value.

### 17.2 Clients

For each logical surface show:

- required physical components;
- installation/product/version/architecture state;
- account state and verification evidence;
- configuration state;
- last successful observation;
- install/repair/open/sign-in/sync actions supported by the adapter.

Top-level actions:

- Refresh;
- Install all missing;
- Sign in and sync.

Every write/action first shows reviewed plan or official-flow explanation.

### 17.3 Login wizard

Steps:

1. verify/install required components;
2. show observed accounts and choose target;
3. launch official flows per client;
4. verify identities and resolve conflict/unknown states;
5. preview and apply supported Connection/resource sync;
6. show final per-client result.

### 17.4 Usage

Tabs or equivalent sections:

- Official subscription/usage;
- Model statistics;
- Sessions/details;
- Data sources and coverage.

Required model columns/metrics:

- raw/canonical model;
- route and confidence;
- clients/surfaces;
- request/session count when observable;
- reported/normalized input;
- fresh input;
- cache read;
- cache write;
- output;
- cache-hit rate;
- API-equivalent estimate;
- official amount where authoritative and applicable;
- coverage/quality flags.

Filters:

- date range;
- client surface;
- route;
- Connection;
- model;
- attribution confidence;
- data source/status.

The UI must explain cache-hit formula and price source/snapshot. Unknown values render as unavailable, not `0`.

### 17.5 Activity

Combine typed records from installation, login, deployment, official refresh, local sync, and rollback. Each record shows status, timestamp, affected clients/sources, redacted diagnostic, and safe next action.

### 17.6 Browser fallback

Browser-only mode must display a prominent Demo/Preview badge, use disposable data, disable native writes, and never look like an authoritative machine scan.

## 18. Implementation sequence

Complete vertical slices in this order:

1. **Deployment and persistence foundation**
   - backend validation;
   - one-shot plans and fingerprints;
   - prepare-all transaction, journal, rollback, ownership, redaction, recovery;
   - migration-safe SQLite foundation and exact decimals.

2. **Environment discovery and installation**
   - physical/logical component model;
   - Windows probes;
   - backend allowlisted install catalog;
   - reviewed plan/apply/progress/cancel/rediscovery;
   - individual and bulk UI.

3. **Account orchestration**
   - safe per-client account observations;
   - target selection/conflict UI;
   - official flow launchers;
   - login run state machine;
   - separate PilotWeave GitHub authorization.

4. **Official usage**
   - runtime quota/model adapter;
   - personal GitHub usage/Billing adapter;
   - snapshot persistence, stale/error/coverage UI.

5. **Pricing**
   - source/versioned immutable snapshots;
   - aliases;
   - decimal engine and estimate coverage.

6. **Copilot CLI local usage**
   - bounded parser;
   - cumulative/idempotent semantics;
   - fixtures and UI.

7. **VS Code local usage**
   - explicit opt-in where needed;
   - safe parser/versioning/privacy;
   - fixtures and UI.

8. **GitHub Copilot app usage**
   - safe supported observation only;
   - Unsupported/Partial when unavailable;
   - shared runtime/source deduplication.

9. **Attribution and complete Usage UI**
   - official/BYOK/unknown routing;
   - model normalization;
   - aggregation, cache rate, price binding, coverage.

10. **Windows clean-machine validation**
    - complete end-to-end acceptance and security regression suite.

Do not implement only DTOs/TODOs/mock cards and mark a slice complete. Each slice must include native behavior, persistence, tests, error/unavailable states, and connected UI.

## 19. Upstream verification requirements

Before coding each adapter, verify current primary sources:

- official GitHub Copilot CLI repository/documentation for installation, login, runtime behavior, models, and supported session metrics;
- official GitHub Copilot app repository/releases/documentation for platform assets, installation, sign-in, and public interfaces;
- official VS Code/GitHub Copilot documentation and extension metadata for installation, profiles, sign-in actions, custom models, and supported debug/usage sources;
- official GitHub REST documentation for current personal usage/Billing endpoints, versions, units, permissions, pagination, and coverage limits;
- official GitHub/provider model pricing sources for rates and effective dates;
- official WinGet/package metadata for exact IDs and publishers.

Package IDs, asset names, schemas, paths, and endpoints are unstable facts. Keep them adapter-owned/versioned and covered by sanitized fixtures. Unknown schema/version becomes SchemaError/Unsupported, not best-effort silent parsing.

Tests must not depend on the live “latest” release or a real GitHub token. Capture sanitized fixed fixtures with provenance/version.

## 20. Test requirements

### 20.1 Unit tests

Cover:

- all validation bounds and secret-in-header/URL rejection;
- plan TTL, one-shot behavior, digest, replay, and stale fingerprints;
- ownership/foreign marker logic;
- prepare/apply/rollback ordering and rollback refusal;
- redaction patterns;
- install allowlist and operation deduplication;
- architecture/asset/package selection;
- exact decimal serialization/calculation;
- token semantic normalization and checked arithmetic;
- route attribution priority and ambiguity;
- model alias ambiguity;
- price tier/rate selection and estimate coverage;
- typed status aggregation.

### 20.2 Integration tests

Use temporary directories, fake credential stores, fake process runners, fake signature verifiers, local HTTP servers, and sanitized fixtures.

Cover:

- state migration/reopen/last-good/read-only recovery;
- usage DB migrations, foreign keys, unique records, transactional cursors;
- target changed between preview/prepare/write/rollback;
- multi-target failure and audit persistence failure;
- shell/registry/file transaction rollback;
- redirect escape, oversized download, digest mismatch, invalid publisher, elevation cancellation, and false installer success;
- same/inferred/conflicting/unsupported account flows;
- official usage unauthorized/forbidden/network/schema/not-covered/successful-empty/zero/stale;
- active incomplete JSONL, malformed interior line, truncation, rotation, cumulative upsert, repeated sync;
- CLI/app physical-source deduplication;
- token fixtures absent from state, DB, events, logs, errors, and diagnostic export;
- hostile HTML strings rendered safely.

Tests must not install software, modify real user profiles/registry/shell files, read real home usage logs, or call live protected APIs.

### 20.3 Required checks

```bash
npm run check:web
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

CI must run relevant checks on Windows, macOS, and Linux; Windows must also perform the required Tauri/build validation for the supported MVP package.

## 21. Final acceptance

The required milestone is complete only when an evaluator who did not implement PilotWeave can perform this Windows 11 x64 flow:

1. install/run PilotWeave from a clean package;
2. observe missing/installed states without fabricated data;
3. preview and install one selected missing component;
4. preview and install all remaining missing components, with shared operations deduplicated;
5. verify actual post-install clients/versions;
6. start `Sign in and sync`;
7. reuse one verified account or resolve an inferred/conflicting state through official flows;
8. confirm no client credential was copied into PilotWeave state/logs/DB;
9. add a Connection and models once;
10. preview and apply supported configuration/resources without destroying unrelated client state;
11. force one target failure and observe safe rollback/accurate audit state;
12. authorize PilotWeave separately for official personal usage;
13. refresh runtime quota and personal GitHub usage, including a handled unavailable/not-covered case;
14. import supported CLI and VS Code local usage twice without duplication;
15. show official and BYOK models with input/output/cache fields, unknown values, route confidence, source coverage, and cache-hit formula;
16. show official GitHub monetary amount separately from API-equivalent estimate;
17. verify historical estimates remain stable after a newer price snapshot;
18. export/view only redacted diagnostics;
19. interrupt a deployment/import and see an explicit recoverable Interrupted state;
20. complete all automated checks and documented security regressions.

No item in the excluded-capabilities section is required or represented as implemented.
