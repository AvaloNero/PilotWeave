# Usage adapters: provenance, schemas and coverage

Verified against official sources on 2026-09-09. These are implementation contracts, not proof that a real installed client/account has passed release acceptance. All fixtures are synthetic, sanitized reductions of the listed schemas. No developer session or credential data was used.

## Copilot runtime quota and models

Source revision: `github/copilot-sdk@d8bbc9dd7a6167d4806780f405d8ce74add1cc7c`.

- [SDK client and handshake](https://github.com/github/copilot-sdk/blob/d8bbc9dd7a6167d4806780f405d8ce74add1cc7c/nodejs/src/client.ts)
- [Generated RPC contract](https://github.com/github/copilot-sdk/blob/d8bbc9dd7a6167d4806780f405d8ce74add1cc7c/nodejs/src/generated/rpc.ts)
- [SDK protocol version](https://github.com/github/copilot-sdk/blob/d8bbc9dd7a6167d4806780f405d8ce74add1cc7c/sdk-protocol-version.json)
- [Official usage and Billing guidance](https://docs.github.com/en/copilot/how-tos/copilot-sdk/features/usage-and-billing)

The adapter starts a native Copilot executable using fixed `--headless --no-auto-update --stdio --log-level error` arguments, with sensitive provider/GitHub environment overrides removed. A private Content-Length JSON-RPC connection is limited to protocol 3 and four methods: `connect`, `auth.getStatus`, `account.getQuota`, `models.list`. Telemetry forwarding is explicitly disabled. It never calls session creation/resumption, prompt, tool or credential-retrieval methods. npm shell wrappers are not executed on Windows; the supported native executable must resolve on PATH.

`quotaSnapshots` entries preserve the quota key, `isUnlimitedEntitlement`, `entitlementRequests`, `usedRequests`, `remainingPercentage`, and authoritative reset date. An unlimited `-1` entitlement becomes `unlimited=true` with no finite allowance. Zero, empty maps, missing counters, unavailable authentication, unsupported protocol and errors remain distinct. Plan name is unavailable in this response. Model policy availability has its own status even when quota retrieval fails.

The reader limits frames to 2 MiB, headers to 8 KiB, models to 512, and quota keys to 64; the entire refresh has a 35-second deadline. Cancellation kills the spawned runtime. Snapshots preserve runtime/parser version, account login when provided, fetched time, source and status. Failure displays the last successful snapshot as stale; a newly observed different account cannot receive another account's successful fallback. If identity itself is unavailable, the prior snapshot is explicitly labeled with its original account. Retention keeps the latest 100 attempts and latest 100 successes.

Fixture: `apps/desktop/src-tauri/tests/fixtures/usage/copilot-quota-v3.json`. Fake RPC tests assert the exact method set, empty/zero/unlimited cases, schema failure, account isolation and independent model visibility.

## Personal GitHub Billing

- [Official personal Billing REST endpoints, API 2026-03-10](https://docs.github.com/en/rest/billing/usage?apiVersion=2026-03-10)

PilotWeave's own separately authorized identity/token accesses only fixed personal `/users/{login}/settings/billing/ai_credit/usage` and `/users/{login}/settings/billing/premium_request/usage` endpoint families. No organization, enterprise, overage, purchasing, provider invoice or account-administration operation is implemented. Identity and period in successful reports must match the request. Endpoint families retain their original units, amounts and coverage independently.

Snapshots are keyed by stable GitHub user ID and host, so a reused login cannot inherit another user's stored Billing. Reads and stale fallback are scoped to account, endpoint family and requested month. Legacy snapshots with only an unverified login key are retained but are not attributed to a current stable identity. Up to 120 recent attempts per account/family are retained, together with the most recent successful snapshot for each stored month. A long failure sequence cannot evict the successful fallback.

Refresh supports the current month and preceding 23 months. Native requests reject redirects and use bounded response sizes and 20-second per-family timeouts. Cancellation skips the next family and discards in-flight responses after the current bounded HTTP call returns. Native exclusion and a five-second minimum interval prevent duplicate refreshes. Valid Retry-After/reset metadata is persisted and enforced before another request. Raw error bodies are never returned. On 404, inaccessible personal coverage remains NotCovered, never zero; AI credits are never added to premium requests. The bounded UI item list exposes its authoritative count and truncation flag.

Money and credit quantities use `ExactDecimal`, including serialization, SQLite persistence and frontend display. Existing Billing parser/store tests now compile and run because their modules and native commands are registered.

## Copilot CLI / shared-runtime local usage

- [Generated session events](https://github.com/github/copilot-sdk/blob/d8bbc9dd7a6167d4806780f405d8ce74add1cc7c/nodejs/src/generated/session-events.ts)
- [Official CLI configuration-directory reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference)
- [Official session event guidance](https://docs.github.com/en/copilot/how-tos/copilot-sdk/features/streaming-events)
- [Upstream example of the v1 session envelope](https://github.com/github/copilot-cli/issues/2000)

Fixed source: `~/.copilot/session-state/*/events.jsonl`. The local adapter accepts a `session.start` event with version 1 and a bounded session ID, followed by `session.shutdown` with `sessionStartTime` in Unix milliseconds and top-level `modelMetrics`. A future version is rejected. The parser extracts each model's request count and input/output/cache-read/cache-write counters. It discards all message, prompt, tool, environment and debug content. Ephemeral `assistant.usage` events are excluded: the cumulative shutdown total is the authoritative physical record for this adapter.

One stable hash of session + raw model identifies one cumulative observation. Later shutdown totals replace previous values, older totals cannot overwrite a newer finish time, and rotated/copy-reimported files do not duplicate totals. Nested subagent metrics are not added again. Daily grouping uses shutdown time and therefore is not a per-request daily ledger.

The current generated shutdown schema does **not** establish whether input is fresh or includes cache, which client used a shared runtime, or whether the route was official/BYOK. Those fields remain Unknown; normalized input, fresh input and API-equivalent cost are unavailable for this schema. Cache-read/write counters remain observable independently. Model names and current active Connection do not resolve historical routing.

The Copilot app has no separately established stable usage store in the verified sources. Its source card is Unsupported. Shared-runtime records are imported once and labeled `shared-copilot-runtime`; no private app database, credential store, cookies or unrelated totals are inspected to manufacture app support.

Fixture: `copilot-session-v1.jsonl`, including nonpersistable message/credential sentinels and a deliberately ignored ephemeral usage event. Tests cover cumulative replacement, old rotation, repeated imports and shared-runtime deduplication.

## VS Code opt-in OpenTelemetry file export

Source revision: `microsoft/vscode-copilot-chat@5863f5a7088958050792b5dccbe8b46c6e13eccc`.

- [VS Code monitoring guide](https://code.visualstudio.com/docs/agents/guides/monitoring-agents)
- [File exporters](https://github.com/microsoft/vscode-copilot-chat/blob/5863f5a7088958050792b5dccbe8b46c6e13eccc/src/platform/otel/node/fileExporters.ts)
- [OTel configuration](https://github.com/microsoft/vscode-copilot-chat/blob/5863f5a7088958050792b5dccbe8b46c6e13eccc/src/platform/otel/common/otelConfig.ts)
- [Inference instrumentation](https://github.com/microsoft/vscode-copilot-chat/blob/5863f5a7088958050792b5dccbe8b46c6e13eccc/src/extension/prompt/node/chatMLFetcher.ts)
- [Anthropic BYOK instrumentation and token normalization](https://github.com/microsoft/vscode-copilot-chat/blob/5863f5a7088958050792b5dccbe8b46c6e13eccc/src/extension/byok/vscode-node/anthropicProvider.ts)
- [OpenTelemetry GenAI token semantics](https://opentelemetry.io/docs/specs/semconv/registry/attributes/gen-ai/)

The UI provides the exact native-owned inbox path, under the platform configuration directory at `PilotWeave/usage-inbox/vscode-otel.jsonl`. Enabling import creates that inbox directory only. It does not alter VS Code settings. The user explicitly enables export in VS Code's User settings, with:

```json
{
  "github.copilot.chat.otel.enabled": true,
  "github.copilot.chat.otel.exporterType": "file",
  "github.copilot.chat.otel.outfile": "<the exact native path shown in Data sources>",
  "github.copilot.chat.otel.captureContent": false
}
```

Restart VS Code after changing export settings. Export affects future observations. Disabling PilotWeave import does not disable VS Code export; the instructions explain how to turn off the client setting separately.

The file exporter writes JSONL spans, logs and metrics. Parser v1 accepts only `gen_ai.operation.name=chat` CLIENT spans (`kind=2`) with a stable trace/span context and start/end HrTime arrays in seconds + nanoseconds. Non-inference parent/tool spans, log bodies, inference-detail events and metric objects are ignored to avoid double counting. Unknown envelopes produce explicit errors, not guessed token data.

Input token attributes follow total-input-including-cache semantics. Fresh input is derived with checked subtraction only when both cache buckets are present. Missing cache-write is null, never implicit zero. Known totals/read counts can still support a weighted cache-hit percentage, with covered-record count shown. Inconsistent counters invalidate derived values rather than underflowing. Current instrumentation frequently omits cache-write; cost coverage is consequently incomplete or unavailable. `vscode-explicit-zero-cache-write.json` is an explicitly labeled synthetic numeric edge case, not a claim that every current client emits that field.

The generic ChatML fetcher can label custom endpoint requests `github`. That provider label with a concrete server address is conservatively left Unknown. An explicit metadata-based GitHub request without a custom server field is recognized as OfficialGithub. Verified `AnthropicBYOK` instrumentation is marked Byok/Explicit, with Connection ID still unknown. Unverified agent names are not a provider-routing heuristic. The parser never stores a server address, authorization header, resource/process attributes, source content or raw trace/session ID.

Fixtures: `vscode-otel-v1.json` and `vscode-explicit-zero-cache-write.json`. They contain artificial private-content sentinels so tests can inspect database/WAL bytes and prove filtering, as well as missing/explicit-zero and malformed cases.

## Prices and API-equivalent estimates

- [Official OpenRouter model API](https://openrouter.ai/docs/api/api-reference/models/get-models)
- [Model/pricing reference](https://openrouter.ai/docs/guides/overview/models)
- Fixed source URL: [OpenRouter model catalog](https://openrouter.ai/api/v1/models)

The adapter fetches only this fixed HTTPS REST source, with no redirects, an 8 MiB limit and a 25-second timeout. Input is a current OpenRouter published minimum-provider catalog-price comparison for **text tokens only**, not a direct-provider invoice, a GitHub charge, or proof of historical effective rates. Non-token fees are excluded and are disclosed in the UI.

Rates are decimal strings denominated in USD/token. Conversion to USD/million and the estimate formula use checked exact decimal operations. Zero/free prices are valid. Models with dynamic/negative or unknown tier schemas are explicitly unsupported. Required prices missing for a positive token bucket leave the estimate unavailable. Context overrides require per-request total-input evidence; cumulative session aggregates cannot choose a cheaper tier. Cache retention ambiguity between 5-minute and 1-hour writes also leaves the estimate unavailable.

The catalog snapshot ID is a hash of sanitized model/tier data. The source version, parser version, fetched time, rates and provenance are immutable. Repeated successful checks of unchanged rates retain that snapshot while refreshing separate job freshness. Exact full IDs and unique suffix aliases are versioned and source-scoped; ambiguous aliases stay unresolved. A cumulative observation preserves its bound snapshot. A price refresh can bind at most 10,000 unbound observations, in batches of 500, and never overwrites an already bound historical estimate. Concurrent observation changes cause that derivation update to be skipped rather than reverting raw counters. A later refresh can continue binding.

Fixture: `openrouter-text-prices-v1.json`, a sanitized reduction of the public response plus explicitly synthetic free/dynamic model cases. Tests cover per-token conversion, free vs missing prices, precise estimates, ambiguous aliases, tier/cache-retention limits, weighted cache rates, coverage and historical binding.

## Import bounds, persistence and recovery

- All sources are off initially. The frontend submits source IDs, never paths or raw records. Consent explains that source files may contain content even when content capture is disabled.
- Fixed roots, one session-directory level, at most 512 candidate entries, 64 MiB per file/run, 1 MiB per line, 100,000 events and 30 seconds per source. Limit exhaustion is Partial; committed cursors can resume. More than 512 session entries remains explicitly partial; the adapter does not promise coverage of directories beyond the bound.
- Sensitive sources and their ancestors reject symlinks/reparse points and non-regular files. Windows opens use no-follow final-handle flags and handle identity; Unix uses `O_NOFOLLOW` and device/inode identity.
- File identity, size/mtime, committed-prefix hash and parser version detect replacement/truncation/rotation. An incomplete final JSONL line does not advance its cursor. A malformed complete batch does not commit its records or advance past the error.
- SQLite v3 preserves v1/v2 tables/data and adds per-file cursor JSON, allowlisted observation payloads, persisted job outcomes, quota/price payloads, Billing retry time and setup preferences. The current-catalog pointer and its last successful check time update on every successful fetch, including returning content, while historical snapshot payloads remain immutable. Older concurrent observations cannot displace newer ones. WAL and foreign keys are enabled. New token counters serialize as decimal strings across IPC, with legacy numeric payloads still readable. Money is always exact decimal text.
- Long-running workers open their own native-owned connection. Atomic batches contain observations, surfaces and file cursor. Clearing a source disables it and removes only its imported records/cursors/jobs; original logs, Billing and price snapshots remain.
- Startup marks in-progress jobs Interrupted. Local, runtime, price and Billing refreshes have native exclusion and cancellation; bounded HTTP calls finish before cancellation discards their result. Runtime and Billing are independent jobs.
- Queries accept at most 366 UTC days, 100,000 matching-range observations and 100 detail rows/page. Aggregates expose priced/unpriced requests/tokens, unresolved models, unknown semantics, source freshness and coverage. Missing values never become successful zero.

## Remaining upstream/platform acceptance

P2 still requires actual supported Windows client builds, official account flows, real personal Billing capability and clean-machine installation validation. This implementation was tested with temporary source files, fake RPC/accounts, sanitized HTTP schemas and isolated browser fixtures. It does not certify a real user's runtime, token, client logs or subscription.

Current CLI input semantics, missing VS Code cache-write buckets, absent separate Copilot app usage and ambiguous historical Connection attribution are explicit source limitations. The next implementation step for any of these is a newly verified official interface/schema, a versioned parser branch and sanitized success/failure fixtures. No missing value is fabricated to make setup or usage look complete.

### Price parser v2: latest aliases (verified 2026-09-10)

The live OpenRouter model catalog response now includes IDs such as ~openai/gpt-latest. [OpenRouter's official quickstart](https://openrouter.ai/docs/quickstart) documents these as moving latest-model aliases. Parser v2 accepts one leading tilde followed by a bounded provider/model slug; it retains the tilde as part of the identity and does not infer an unqualified or concrete-model mapping. Existing v1 snapshots remain readable and immutable. Unknown identifiers still produce SchemaError rather than silently dropping rows.

The openrouter-latest-aliases-v2.json fixture retains only the public alias ID/pricing fields from that response plus the existing synthetic concrete-model fixture. Tests cover v1/v2 coexistence, stored parser version, malformed identities and alias separation. Import model identity validation accepts the same bounded spelling without changing the versioned log envelopes.
