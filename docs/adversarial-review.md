# Adversarial review

This document records hostile inputs, failure modes, ambiguity, and privacy cases that shape PilotWeave's required implementation. The normative behavior is in `docs/mvp-implementation-spec.md`.

Sections below describe required mitigations. They are not claims that the current prototype already implements every item.

## Threat model

PilotWeave runs as the local user and intentionally modifies developer-tool configuration. It may launch installers, open official authentication flows, read local usage metadata, query GitHub personal-usage endpoints, and calculate monetary estimates.

An attacker or faulty dependency may influence:

- Tauri command payloads from a compromised frontend;
- connection names, endpoints, headers, and model strings;
- package-manager output and PATH resolution;
- GitHub release metadata, redirects, asset names, and bytes;
- client configuration and log files;
- symlinks, junctions, reparse points, and files changed during an operation;
- malformed or hostile JSON, JSON5, SQLite, and JSONL content;
- GitHub HTTP error bodies and proxy diagnostics;
- timestamps, model aliases, and deployment history used for attribution or pricing;
- files containing prompts, code, tool output, or credentials next to desired counters.

PilotWeave does not assume that an allowlisted host, installed client, package manager, release asset, or local file is inherently trustworthy.

## Deployment threats

### Frontend bypass

A caller may invoke native commands without using frontend validation.

Required mitigation:

- validate every field and collection again in Rust;
- enforce length/count/numeric bounds;
- reject remote plaintext HTTP endpoints except explicitly supported loopback cases;
- reject credentials in URLs and raw credentials in persisted header fields;
- validate header-name grammar and case-insensitive uniqueness;
- return typed, redacted errors.

### Preview/apply time-of-check/time-of-use

A user may preview configuration A, another process changes the target, and apply writes configuration B.

Required mitigation:

- store the complete plan in native memory;
- return only a plan ID and non-secret summary;
- include connection version, target fingerprints, operation set, and digest;
- expire plans after a short fixed TTL;
- consume each plan once;
- reject apply when any fingerprint or version changes;
- never silently recompute and execute a materially different plan.

### Partial cross-client writes

VS Code may be updated before CLI preparation or write fails.

Required mitigation:

- prepare every target before the first write;
- persist a rollback manifest and write-ahead journal;
- apply in deterministic order;
- reverse successful writes when a later target fails;
- report per-target outcomes and overall partial/rolled-back status accurately.

### Audit persistence failure after client writes

Native configuration may change but the application record fails to persist.

Required mitigation:

- journal Prepared and Applying before writes;
- if final audit persistence fails, attempt to restore all applied targets;
- expose unresolved journal entries as Interrupted at startup;
- never report an operation as cleanly failed when side effects may remain.

### Unsafe Windows replacement

Delete-then-rename creates a window where a crash removes the destination.

Required mitigation:

- use a platform-supported replace-with-write-through primitive;
- keep validated last-known-good state;
- test interruption/failure between each filesystem phase.

### Symlink, junction, reparse, and path substitution

An attacker may redirect a managed path to another file between inspection and write.

Required mitigation:

- reject symlinks and unsupported reparse points for sensitive sources/targets;
- use canonical parent handling without accepting path traversal;
- open/write through platform APIs that minimize path re-resolution;
- recheck file identity immediately before replacement;
- bound file size before parsing or snapshotting.

### Forged ownership markers

A third party can add a PilotWeave-looking marker to unrelated configuration.

Required mitigation:

- markers include `installation_owner_id` and `connection_id`;
- persisted state keeps the last deployed target identity and fingerprint;
- marker text alone is never sufficient ownership proof;
- foreign-owner or ambiguous state is a conflict requiring user resolution.

### Rollback overwrites newer user work

A target may change after PilotWeave wrote it but before rollback.

Required mitigation:

- store both before and expected-after fingerprints;
- rollback only when live state still matches expected-after state;
- otherwise stop and report external modification;
- never force rollback over a mismatch by default.

### Credential-store ambiguity

A locked or unavailable credential store may be shown as “no secret.”

Required mitigation:

- distinguish Stored, Missing, Locked, PermissionDenied, and Unavailable;
- do not invite re-entry when the store is merely inaccessible;
- redact credential-store diagnostic data before frontend exposure.

### Connection deletion split-brain

State deletion may succeed while secret deletion or target revocation fails.

Required mitigation:

- expose explicit `revoke and delete` versus `detach only` operations;
- plan/prepare revocation transactionally;
- represent cleanup warnings separately from deletion success;
- detect orphaned secrets without enumerating or exposing secret values.

## Installation threats

### Remote command injection

A compromised frontend or remote metadata response may try to turn an install plan into arbitrary code execution.

Required mitigation:

- strategy kinds and package identities are compiled/backend-owned;
- frontend submits only a native-held plan ID;
- remote metadata never provides a shell command or argument vector;
- child processes use a fixed executable with separate arguments;
- prohibit `shell=true`, concatenated `cmd /c`, `Invoke-Expression`, and remote-script pipelines.

### Package substitution and lookalikes

An attacker may publish a similar package, alter PATH, or replace a downloaded asset.

Required mitigation:

- exact allowlisted package IDs and sources;
- trusted package-manager executable resolution or identity validation;
- GitHub Copilot app assets only from the official allowlisted repository;
- strict asset naming and architecture selection;
- content-length and streamed-byte limits;
- SHA-256 verification when authoritative metadata is available;
- Windows Authenticode validity and expected publisher verification;
- launch the verified temporary path rather than a same-named PATH entry;
- rediscover and verify product identity/version after installation.

### Malicious redirects

A release asset may redirect outside trusted delivery infrastructure or downgrade transport.

Required mitigation:

- inspect every redirect;
- permit only the documented GitHub asset-delivery host set;
- reject scheme downgrade and user-controlled destinations;
- cap redirect count and response sizes.

### Stale plan or architecture mismatch

The machine may change after preview, or a plan may be replayed on another architecture.

Required mitigation:

- plan digest includes platform, architecture, component observations, selected asset identity, and dependencies;
- apply repeats environment and asset validation;
- changed environment invalidates the plan;
- one-shot/TTL rules match deployment-plan handling.

### Elevation confusion

A user may cancel elevation or an unexpected binary may trigger a misleading prompt.

Required mitigation:

- preview product, publisher, source, and elevation requirement;
- launch only the prevalidated operation;
- treat cancellation/denial as an explicit result;
- wait for completion where supported;
- rediscover rather than trusting launcher exit code.

### Destructive repair

An installation workflow may remove settings, uninstall, or downgrade software.

Required mitigation:

- no automatic uninstall or downgrade;
- no deletion of unrelated application data;
- distinguish repair, update, missing component, and unsupported version;
- preview exact expected effects.

### Partial bulk installation

One operation can fail after others succeed.

Required mitigation:

- one result per component and physical package operation;
- dependent operations are skipped with explicit dependency status;
- independent operations may continue;
- overall status remains Partial when appropriate;
- shared CLI/app package operations are deduplicated rather than executed twice.

### False success

Installer exit code zero may not mean the client is usable.

Required mitigation:

- post-install rediscovery is authoritative;
- verify path, product identity, architecture, version, and required companion component;
- report `process succeeded, verification failed` distinctly.

## Account and sign-in threats

### Token-copy shortcut

A developer may try to implement “one-click sign-in” by copying existing client credentials.

Required mitigation:

- prohibit reading VS Code SecretStorage, browser cookie stores, CLI OAuth files, Copilot app credential stores, passwords, passkeys, device secrets, and refresh tokens;
- launch official browser/device/application flows only;
- tests search state, DB, logs, events, and frontend payloads for token fixtures.

### Wrong-account propagation

An inferred or stale identity may be used as the target for other clients.

Required mitigation:

- identity includes host and login, plus stable user ID when available;
- Verified, Inferred, UserConfirmed, Unknown, Unsupported, and Conflict remain distinct;
- inferred identity requires explicit confirmation;
- conflicting verified identities block completion;
- PilotWeave never silently signs out a client;
- final validation shows evidence for each observation.

### Unsupported-host confusion

A GitHub Enterprise identity may be treated as `github.com`.

Required mitigation:

- host is part of identity and equality;
- MVP reconciliation supports `github.com` only;
- another host is Unsupported rather than coerced;
- credentials for one host are never sent to another.

### Malicious deep links or commands

A sign-in launcher may open an arbitrary URL or command supplied by the frontend.

Required mitigation:

- sign-in entry points are adapter constants or validated official APIs;
- frontend cannot supply URI, host, executable, or command ID;
- no user-controlled interpolation into a shell string;
- display instructions are escaped before rendering.

### PilotWeave GitHub token leakage

HTTP errors or tracing may include Authorization values.

Required mitigation:

- store the token in a separate credential-store entry;
- never log Authorization headers;
- register secrets with central redaction before validation calls;
- reduce errors to bounded status and sanitized diagnostics;
- schema-parse responses instead of forwarding raw bodies;
- exclude secret-bearing request objects from panic/tracing output.

## Official quota and Billing threats

### Unavailable represented as zero

A 403, 404, expired token, organization-managed license, network error, or schema drift may be shown as no usage.

Required mitigation:

- distinguish successful-empty, unauthorized, forbidden, unsupported, not-covered, network-error, schema-error, stale, and numeric zero;
- preserve last successful data as stale after refresh failure;
- only a successful authoritative response may produce authoritative zero.

### Runtime quota schema drift

The runtime may add quota keys, omit fields, report unlimited entitlement, or change schema.

Required mitigation:

- pin/version the runtime adapter;
- preserve bounded unknown quota keys rather than hard-code only known names;
- Unlimited, Exhausted, Unavailable, Stale, and numeric zero remain distinct;
- retain last successful snapshot after parser/network failure;
- never convert runtime quota into fabricated Billing amount or token usage.

### Mixing incompatible units

AI credits and premium requests measure different things.

Required mitigation:

- persist unit and billing mode per item;
- never sum incompatible units;
- group UI totals by unit/mode;
- preserve original quantities when an authoritative response also provides monetary amounts.

### Overstating personal Billing coverage

Personal endpoints do not cover organization or enterprise-paid usage.

Required mitigation:

- identify account and endpoint family on every snapshot;
- show coverage limits explicitly;
- organization-paid usage is NotCovered, not zero;
- local request/token observations never fabricate missing official amounts.

### Floating-point money errors

Binary floats can drift and fail reconciliation.

Required mitigation:

- exact decimal/fixed-point types end to end;
- canonical decimal strings or scaled integers in persistence;
- exact serialization and aggregation tests.

### Fabricated remaining allowance

The API may provide usage without allowance/reset values.

Required mitigation:

- remaining and reset are optional and require authoritative fields;
- do not subtract local request counts from public plan marketing limits;
- show covered period and fetch time explicitly.

## Usage-log threats

### Prompt, response, source-code, or tool retention

Logs may contain sensitive content next to token metadata.

Required mitigation:

- parse allowlisted fields into typed records;
- do not persist raw lines or unknown content blobs;
- do not copy prompts, responses, source, tool input/output, environment, headers, cookies, or auth material;
- use sanitized fixtures in tests.

### Unbounded resource consumption

A hostile log tree or file may cause excessive traversal, memory, or DB growth.

Required mitigation:

- fixed allowed roots;
- directory depth, file count, file size, line size, event count, and date-range limits;
- streaming parsers;
- cancellation and progress;
- source errors do not block independent sources.

### Active partial writes

A running client may leave an incomplete final JSONL line.

Required mitigation:

- tolerate one unterminated final record;
- do not advance the durable cursor past unconsumed bytes;
- treat malformed complete interior records as parser/schema errors.

### Truncation and rotation

A file may shrink, be replaced, or reuse a path.

Required mitigation:

- cursor includes file identity, size, modification evidence, and parser version;
- detect shrink/replacement and enter a versioned rescan branch;
- stable record keys and upsert semantics prevent double counting.

### Cumulative versus delta counters

A shutdown event may contain cumulative session totals and be imported repeatedly.

Required mitigation:

- parser defines source semantics explicitly;
- cumulative snapshots replace/upsert by stable session+model identity;
- delta sources append with stable request IDs;
- repeated sync is idempotent.

### Shared CLI/app runtime double counting

Copilot app may expose the same underlying runtime records as CLI.

Required mitigation:

- identify physical source, not only logical surface;
- import a physical record once;
- record multiple consuming surfaces only as metadata when evidence supports it;
- never duplicate token totals to make each client chart look complete.

### Missing token fields converted to zero

Some clients may omit cache-write or even total-input semantics.

Required mitigation:

- use optional fields and semantic enums;
- calculate only derivable buckets;
- unknown cache/write/input values remain unavailable;
- coverage percentages accompany aggregates.

### Unsigned underflow

`fresh = total - cacheRead - cacheWrite` can underflow on inconsistent data.

Required mitigation:

- validate with checked/saturating logic;
- mark semantics inconsistent rather than emitting a huge number;
- retain source-reported counters for diagnostics without exposing content.

## Attribution and model threats

### Model-name collision

The same model text may exist on GitHub official and one or more BYOK providers.

Required mitigation:

- model name alone never proves route;
- use explicit source route first, stable connection identity second, unique deployment-timeline inference third;
- inference is labeled and includes confidence/evidence;
- unresolved route remains Unknown.

### Current-state attribution

A historical session may be assigned to whichever Connection is active now.

Required mitigation:

- use time-bounded deployment history and target/client identity;
- require a unique match;
- never use current state alone for historical attribution.

### Alias drift

Aliases may change or become ambiguous later.

Required mitigation:

- preserve `raw_model` permanently;
- aliases are versioned/source-scoped;
- canonical normalization is derived and may be recomputed;
- ambiguous aliases remain unresolved.

## Pricing threats

### Estimate presented as actual charge

API-equivalent model cost may be mistaken for GitHub's bill.

Required mitigation:

- separate `official_net_amount_usd` and `estimated_api_equivalent_usd` fields, cards, labels, and data sources;
- never add estimates to official amounts;
- BYOK provider actual invoices are outside scope.

### Historical repricing

Updating a price table may silently change old totals.

Required mitigation:

- immutable price snapshots with effective/fetched time and source version;
- historical records retain snapshot references;
- current-price comparison is separate and does not mutate history.

### Wrong threshold/tier

Long-context pricing may depend on per-request input length, unavailable in an aggregate-only record.

Required mitigation:

- select tiers only with sufficient per-call evidence;
- otherwise mark estimate partial/unavailable or explicitly approximate under a documented rule;
- never silently choose the cheaper tier.

### Missing cache rate

A provider may omit cache-write pricing because it is not applicable or because data is unavailable.

Required mitigation:

- distinguish NotApplicable from Unknown;
- unknown required rates block complete estimation;
- zero is used only when the source explicitly establishes zero/not-applicable semantics.

### Currency confusion

Prices may not be in USD.

Required mitigation:

- store currency on every price row;
- do not aggregate currencies without an explicit versioned FX source;
- the required MVP may restrict calculations to USD rather than invent conversion.

## UI and reporting threats

### Mock data presented as native data

Browser fallback or unfinished pages may look authoritative.

Required mitigation:

- visible Demo/Browser Preview badge;
- native writes disabled in preview mode;
- unavailable capabilities display unavailable, not fabricated numbers;
- no static “success rate” or resource count without a real source.

### Status collapse

A Boolean status hides partial, stale, unknown, or conflict states.

Required mitigation:

- typed state machines in backend and frontend;
- badges/tooltips explain source and evidence;
- overall status derives from per-step records without erasing detail.

### XSS/HTML injection

Connection names, model names, diagnostics, or package output may contain markup.

Required mitigation:

- text rendering APIs by default;
- strict escaping/sanitization where templating is unavoidable;
- Content Security Policy compatible with the Tauri frontend;
- tests with hostile strings.

### Sensitive export

Diagnostics may include paths, usernames, model-session IDs, or secret-adjacent config.

Required mitigation:

- explicit allowlist for diagnostic export;
- central redaction before serialization;
- hash or shorten sensitive IDs/paths;
- preview exactly what will be exported;
- never attach raw usage logs or credentials automatically.

## Required adversarial test groups

The implementation is not complete without tests covering at least:

- direct native-command validation bypass;
- expired/replayed/stale plans;
- target modification between preview, prepare, write, and rollback;
- multi-target partial failure and audit persistence failure;
- Windows replacement interruption;
- symlink/junction/reparse substitution;
- forged/foreign ownership markers;
- locked/unavailable credential stores;
- installer command/argument injection;
- wrong package/source/architecture/asset/publisher;
- redirect escape, oversized download, digest mismatch, and elevation cancellation;
- installer exit success with failed post-install discovery;
- inferred/verified/conflicting identities and unsupported hosts;
- token fixtures absent from state, DB, events, errors, and logs;
- official-usage unauthorized/forbidden/schema/network/not-covered/empty/zero states;
- mixed AI-credit and premium-request units;
- truncated/rotated/active/oversized/malformed usage sources;
- cumulative snapshot idempotence and shared-source deduplication;
- missing token fields and inconsistent token arithmetic;
- model alias collision and ambiguous route attribution;
- exact decimal arithmetic, immutable historical pricing, threshold ambiguity, and missing rates;
- hostile UI strings and redacted diagnostic export.
