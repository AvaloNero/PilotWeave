# PilotWeave

**A local control plane for GitHub Copilot clients.**

PilotWeave keeps provider connections, model catalogs, credentials, and client targets in one place, then projects that desired state into supported Copilot surfaces installed on the machine.

> Configure once. Preview every change. Apply across supported Copilot surfaces.

> [!IMPORTANT]
> PilotWeave is an independent, community-built project. It is not affiliated with, sponsored by, or endorsed by GitHub. GitHub and GitHub Copilot are trademarks of GitHub, Inc. The PilotWeave name, woven-connection icon, and interface are original project assets and do not reproduce GitHub, Octocat, or GitHub Copilot logos.

## Current implementation

PilotWeave is still pre-release, but the repository now contains working vertical slices rather than only a static prototype:

- Connection and model-catalog CRUD with bounded native validation for URLs, headers, model limits, identifiers, serialized size, and persisted collection invariants.
- Provider API keys stored through the operating-system credential store rather than `state.json`; authentication headers use a deployment-time secret placeholder instead of persisted literal credentials.
- Explicit read-only recovery when `state.json`, login history, GitHub authorization metadata, or `usage.sqlite3` cannot be loaded safely.
- Discovery of Visual Studio Code Stable/Insiders profiles, GitHub Copilot CLI, and the GitHub Copilot app.
- Native-held, expiring, one-shot deployment plans with live target fingerprints, write-ahead journal state, bounded snapshots, stale-preview rejection, rollback checks, and centralized frontend error redaction.
- VS Code `chatLanguageModels.json` projection that preserves non-PilotWeave groups and creates an original rollback backup.
- Copilot CLI provider activation through user environment variables on Windows and managed shell environment files on macOS/Linux.
- GitHub Copilot app detection with an explicit read-only/manual provider-configuration boundary.
- Reviewed Windows installation plans for allowlisted WinGet packages and the exact GitHub Copilot VS Code extension, followed by real component rediscovery rather than optimistic success.
- Official client sign-in orchestration for VS Code Copilot, Copilot CLI, and the GitHub Copilot app. Plans contain backend-owned executable paths and fixed arguments; persisted run summaries never contain client credentials.
- Account observations that keep Verified, Inferred, Action required, Unknown, Unsupported, Not installed, and Conflict distinct. Where a client has no stable token-free identity interface, PilotWeave shows the limitation instead of manufacturing verification.
- A separate PilotWeave-owned GitHub authorization stored under its own operating-system credential reference. The backend validates the authenticated `github.com` identity and probes personal premium-request Billing capability without extracting or reusing any client token.
- A migration-safe `usage.sqlite3` foundation covering sources, cursors, raw normalized usage records, quota/Billing snapshots, model aliases, immutable price snapshots, and estimate fields.
- Exact decimal serialization and checked arithmetic for monetary data; binary floating-point JSON values are rejected from the money domain.
- Native Settings and Clients panels for installation, account orchestration, sign-in history, separate GitHub authorization, storage recovery, and browser-preview limitations.
- A browser fallback for reviewing the interface without native writes or real authentication.

The separate GitHub authorization slice currently validates identity and endpoint capability. It does **not** yet claim that personal Billing report items are synchronized into `usage.sqlite3`, nor that official runtime quota and local token-usage import are complete.

## Required MVP

The complete behavior and acceptance criteria are defined in [the MVP implementation specification](docs/mvp-implementation-spec.md). Remaining required work includes:

- Complete clean Windows 11 x64 installation verification, including every applicable package/product/publisher requirement and partial/cancellation behavior.
- Continue same-account verification after official client flows where stable, token-free client observations are available; preserve Action required or Unsupported elsewhere.
- Fetch, validate, persist, and render authoritative personal GitHub AI-credit or legacy premium-request snapshots without mixing incompatible units.
- Add current supported Copilot runtime quota/model observations independently from GitHub Billing.
- Import observable local model usage for official GitHub routes and BYOK routes while preserving unknown fields, attribution confidence, source coverage, and privacy boundaries.
- Show input, output, cache read, cache write, cache-hit rate, route confidence, covered period, freshness, and data-quality warnings.
- Calculate and display decimal API-equivalent estimates from immutable, versioned price snapshots while keeping GitHub authoritative amounts visibly separate.
- Implement the required MCP, Skills, and Instructions synchronization only for stable public paths with the normal preview, ownership, journal, and rollback contract.
- Finish deletion/revocation, ownership, interrupted-operation, and clean-machine regression cases that remain open in the normative specification.

The primary clean-machine acceptance environment is Windows 11 x64. Existing macOS and Linux discovery/deployment behavior must not regress, but full one-click installation on those platforms is outside the required scope.

Read [AGENTS.md](AGENTS.md) before implementation. The active development branch is `main` while the project is pre-release.

## Explicit non-goals

The required milestone does not include organization or enterprise Billing administration, BYOK-provider invoice APIs, cloud usage synchronization, multi-user allocation, overage purchasing/settings changes, full macOS/Linux one-click installation, automatic upload of local logs, or parsing private Copilot application stores to manufacture authentication or usage support.

## Run

Prerequisites:

- Node.js 22+
- Rust 1.88+
- Platform requirements for Tauri 2

```bash
npm install
npm run dev
```

For a frontend-only review, open `apps/desktop/web/index.html` directly. Browser fallback data is disposable and must never be presented as native state.

## Tests

`npm run check` runs the local gate: web syntax checks, `cargo fmt --check`, `cargo test --workspace`, and `cargo clippy -- -D warnings`. CI runs the same checks on Ubuntu, Windows, and macOS for every push to `main` and every pull request; the Windows job also builds the native Tauri application.

Current regression coverage includes:

- **Deployment lifecycle** — one-shot/expiring plans, stale targets, journals, rollback refusal after external changes, and recovery-safe state handling.
- **VS Code Copilot** — enabled models are projected into a temporary `chatLanguageModels.json`; switching models replaces only the PilotWeave-owned group, foreign groups survive, and the original backup remains distinct from per-deployment snapshots.
- **Copilot CLI** — `COPILOT_PROVIDER_*` projection is verified per provider kind and protocol; switching the default model rewrites every model variable, and a failing store write restores the previous fake snapshot.
- **GitHub Copilot app** — deployment remains intentionally read-only; tests pin the manual-configuration boundary and runbook preview.
- **Installation and sign-in** — backend-owned component/surface allowlists, one-shot plans, bounded native-process output, interrupted login-run recovery, and UTF-8-safe redacted diagnostics.
- **Separate GitHub authorization** — fake credential-store tests prove token/metadata separation, corrupt metadata recovery, rollback of a previous secret after metadata-write failure, bounded token/scope validation, and clear behavior.
- **Usage persistence and money** — SQLite migrations, foreign keys, idempotent transactional upserts/cursors, bounded batches, reopen behavior, and exact decimal round trips.

Tests use temporary directories, fake stores/runners, and sanitized data. They never modify real VS Code profiles, registry keys, environment variables, shell files, client sessions, or GitHub credentials.

## Architecture

```text
Connection + model catalog
        │
        ├── VS Code Copilot adapter
        ├── Copilot CLI adapter
        └── GitHub Copilot app adapter
                │
                └── native plan / preview / journal / rollback / result

Account and usage control loops
        ├── environment discovery / reviewed installation
        ├── official client sign-in orchestration / qualified identity evidence
        ├── separate PilotWeave GitHub authorization
        └── runtime quota + personal Billing + local usage + pricing
                         │
                         └── usage.sqlite3 (raw observations before derived views)
```

The source of truth is client-neutral. Client-specific paths, configuration fields, login evidence, and usage parsers belong to adapters rather than being copied into independent provider catalogs. Client login credentials and PilotWeave's own GitHub authorization are separate trust domains.

See [architecture](docs/architecture.md), [adversarial review](docs/adversarial-review.md), and [security policy](SECURITY.md).

## Safety model

- Frontend payloads are untrusted and may never contain executable installer commands, authoritative prices, route attribution, or a completed status.
- Provider secrets and PilotWeave's GitHub API authorization use separate operating-system credential-store references and are never returned to the frontend.
- Client sign-in uses fixed official browser/device/application flows; PilotWeave does not copy client authentication material.
- Authentication helper processes run with known sensitive GitHub/Copilot/provider environment variables removed, bounded output, fixed arguments, and timeouts where output is captured.
- Missing, unavailable, inferred, partial, stale, conflict, and numeric zero are distinct states.
- Raw prompts, responses, source code, tool payloads, environment dumps, cookies, and authentication headers are not retained by usage collection.
- GitHub authoritative monetary values and API-equivalent estimates remain separate metrics and use exact decimal representations.
- GitHub Copilot app provider mutation remains disabled until a stable, supportable interface is available.

## Repository layout

```text
apps/desktop/src-tauri/          Native Tauri backend and client adapters
apps/desktop/web/                Desktop frontend
AGENTS.md                        Coding-agent implementation entry point
docs/mvp-implementation-spec.md Normative MVP contract
docs/architecture.md             Current and required architecture
docs/adversarial-review.md       Threat and failure analysis
SECURITY.md                      Disclosure and sensitive-data policy
```

## License

MIT
