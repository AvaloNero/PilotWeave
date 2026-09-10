# Connection model discovery

The connection editor can query a provider's model catalog before saving a connection. After entering a URL and API key, leave the edited field to start a debounced query. **Fetch models** supports explicit queries and retries, including public/local catalogs without a key. Filter the returned list, select models and use **Add selected**, then save the connection. Existing manual entries and custom names are preserved; selection never saves or deploys by itself. A connection remains limited to 128 models.

## Supported sources, verified 2026-09-11

- [OpenAI List models](https://developers.openai.com/api/reference/resources/models/methods/list): GET `/v1/models`, Bearer authorization, `data[].id`. Responses and Chat Completions use this same catalog. OpenAI-compatible providers are accepted when they expose that envelope; optional `name` is used only as a display label.
- [Anthropic List Models](https://platform.claude.com/docs/en/api/models/list): GET `/v1/models`, `x-api-key`, `anthropic-version: 2023-06-01`, `data[].id`, `display_name`, `has_more`, `last_id`, and `after_id` pagination. Messages protocol selects this parser and header policy.
- Azure returns an explicit unsupported state because this editor requires deployment names. A foundation-model catalog must not be substituted for deployed model identifiers. Local and custom providers use the selected wire protocol's catalog format; opaque stores and vendor-specific discovery APIs are not inspected.

The parser version is `1`. Sanitized schema fixtures are in `apps/desktop/src-tauri/tests/fixtures/models/`. Unknown schemas, unsupported APIs and authentication failures remain explicit states. Catalog presence does not establish that a model supports chat, tools, vision, reasoning, or a particular context length. Discovered models start with unknown capabilities; existing configured capabilities are preserved.

## Request and credential boundary

`discover_connection_models` validates the unsaved URL, provider/protocol, key and header templates in Rust and performs only GET requests. No pricing, Billing or inference call is made. A bare origin uses `/v1/models`; a base path gains `/models`; a terminal `/chat/completions`, `/responses`, or `/messages` is replaced by `/models` under that same prefix. Existing `/models` is retained. Discovery rejects query strings, embedded credentials, fragments, and non-HTTPS destinations except loopback HTTP. There is no fallback to another host or guessed endpoint.

An explicitly entered key is used only in memory. A saved key can be reused only for its native-held connection ID and unchanged URL, provider, protocol and header templates. The saved configuration and credential are captured under the state mutex and cross-process managed-write lease, rechecking the persisted bytes after acquiring the lease; both locks are released before HTTP. Changed configuration requires entering a key again; choosing to remove the saved credential prevents reuse. Recovery state prevents saved-state credential access. Discovery never writes the credential store or reads client authentication material.

Requests disable redirects. Loopback HTTP bypasses ambient proxies; isolated validation also disables proxies and substitutes only its private fixture endpoint. Host, transport, cookie and proxy-authorization headers are rejected. Ordinary custom headers retain the existing `${apiKey}` template rules. Network exceptions and provider error bodies never reach the frontend; fixed status messages pass through central redaction. Model IDs/names are bounded, checked for credential reflections and unsafe delimiters, then displayed as text. Responses and raw bodies are not persisted.

Only one query can run at a time. Each query has a 15-second aggregate HTTP deadline, at most 8 pages, 4 MiB of response bytes in total, and 2,048 distinct model results. Anthropic pagination modifies only a bounded cursor on the same URL. Truncation, unknown compatible pagination or failure after a usable page returns **partial**, never complete. An empty valid response is distinct from unavailable data. Edits and modal closure invalidate in-flight UI results; existing model text is preserved on failure. Browser preview makes no provider requests and explains that discovery requires the desktop app.

## Verification

- Rust unit tests exercise URL/header validation, credential binding with a fake reader, parser fixtures, malformed/oversized data, case-insensitive deduplication, single-flight release, and real loopback HTTP authentication, pagination, redirects, empty/error/partial outcomes.
- Web tests verify merge preservation, the 128-model limit, header parsing, stale/closed request results and redacted transport failures.
- Native isolated case B27 executes the compiled WebView2 form, real Tauri IPC, Rust GET requests to a local HTTP fixture, selection, save, saved-key reuse, changed-destination rejection, provider errors and late-response invalidation. It uses only synthetic keys and isolated state.

These tests do not establish access permissions or inference support for a maintainer's real provider account; that requires using their endpoint and key in the desktop form.
