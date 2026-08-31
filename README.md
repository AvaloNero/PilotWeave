# PilotWeave

**A local control plane for GitHub Copilot clients.**

PilotWeave keeps provider connections, model catalogs, credentials, and client targets in one place, then projects that desired state into supported Copilot surfaces installed on the machine.

> Configure once. Preview every change. Apply across supported Copilot surfaces.

> [!IMPORTANT]
> PilotWeave is an independent, community-built project. It is not affiliated with, sponsored by, or endorsed by GitHub. GitHub and GitHub Copilot are trademarks of GitHub, Inc. The PilotWeave name, woven-connection icon, and interface are original project assets and do not reproduce GitHub, Octocat, or GitHub Copilot logos.

## Current implementation

The code currently provides an initial prototype for:

- Connection and model catalog CRUD.
- API keys stored through the operating-system credential store rather than `state.json`.
- Detection of Visual Studio Code Stable/Insiders profiles, GitHub Copilot CLI, and the GitHub Copilot app.
- Deployment preview for detected clients.
- VS Code `chatLanguageModels.json` projection that preserves non-PilotWeave groups and creates an original rollback backup.
- Copilot CLI provider activation through user environment variables on Windows and managed shell environment files on macOS/Linux.
- GitHub Copilot app detection with an explicit read-only/manual provider-configuration boundary.
- A browser fallback for reviewing the interface without native writes.

This is not yet the completed MVP. The prototype must still be hardened for native-held one-shot plans, stale-target detection, cross-target preflight and rollback, write-ahead audit state, verified ownership, interrupted-operation recovery, and centralized redaction.

## Required MVP

The complete behavior and acceptance criteria are defined in [the MVP implementation specification](docs/mvp-implementation-spec.md). Required scope includes:

- Install one selected missing Copilot surface or install all missing surfaces from one reviewed native plan, deduplicating a shared official package operation when it satisfies both CLI and desktop surfaces.
- Validate package identity, source, architecture, bounded download size, digest when authoritative metadata is available, Windows publisher, and the real post-install application state.
- Orchestrate the official sign-in flow for VS Code Copilot, Copilot CLI, and the GitHub Copilot app.
- Reuse an already verified account as the proposed target for other surfaces, detect conflicts, and show the verification basis without copying client tokens, cookies, SecretStorage, or credential databases.
- Keep PilotWeave's separate GitHub personal-usage authorization in the operating-system credential store.
- Query current Copilot runtime quota and authoritative personal GitHub AI-credit or legacy premium-request usage without mixing their units.
- Import observable local model usage for official GitHub routes and BYOK routes while preserving unknown fields, attribution confidence, and source coverage.
- Show input, output, cache read, cache write, cache-hit rate, route confidence, and data-quality warnings.
- Calculate a decimal API-equivalent estimate from immutable, versioned price snapshots while keeping GitHub actual amounts and calculated estimates visibly separate.
- Complete the deployment safety properties listed in the specification before using deployment from installation or login workflows.

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

## Architecture

```text
Connection + model catalog
        │
        ├── VS Code Copilot adapter
        ├── Copilot CLI adapter
        └── GitHub Copilot app adapter
                │
                └── deployment plan / diff / result

Required MVP control loops
        ├── environment discovery / reviewed installation
        ├── official sign-in orchestration / account verification
        └── official usage + local usage / attribution / pricing
```

The source of truth is client-neutral. Client-specific paths, configuration fields, login evidence, and usage parsers belong to adapters rather than being copied into three independent provider catalogs.

See [architecture](docs/architecture.md), [adversarial review](docs/adversarial-review.md), and [security policy](SECURITY.md).

## Safety model

- Frontend payloads are untrusted and may never contain executable installer commands, authoritative prices, route attribution, or a completed status.
- Provider and GitHub API secrets use separate operating-system credential-store entries and are never returned to the frontend.
- Client sign-in uses only official browser/device/application flows; PilotWeave does not copy client authentication material.
- Missing, unavailable, inferred, partial, stale, conflict, and numeric zero are distinct states.
- Raw prompts, responses, source code, tool payloads, environment dumps, cookies, and authentication headers are not retained by usage collection.
- GitHub actual monetary values and API-equivalent estimates are displayed as separate metrics.
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
