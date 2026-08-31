# PilotWeave

**Local control plane for GitHub Copilot clients.**

PilotWeave keeps provider connections, model catalogs, credentials, and deployment state in one place, then projects that desired state into the Copilot surfaces installed on the machine.

> Configure once. Run across every Copilot surface.

## MVP status

The first implementation includes:

- Connection and model catalog CRUD.
- API keys stored through the operating-system credential store; plaintext keys are not written to PilotWeave state JSON.
- Detection of Visual Studio Code Stable/Insiders profiles, GitHub Copilot CLI, and the GitHub Copilot app.
- Deployment preview for all detected clients.
- Working deployment to VS Code `chatLanguageModels.json`, preserving non-PilotWeave groups and creating a rollback backup.
- Working Copilot CLI provider activation through user environment variables on Windows and managed shell environment files on macOS/Linux.
- GitHub Copilot app detection with an explicit read-only/manual deployment boundary until a stable external provider-management interface is available.
- A browser fallback mode, so the product UI can be reviewed without Tauri.

Resources and activity pages are deliberately present but marked as the next milestone rather than pretending to be complete.

## Run

Prerequisites:

- Node.js 22+
- Rust 1.88+
- Platform requirements for Tauri 2

```bash
npm install
npm run dev
```

For a frontend-only review, open `apps/desktop/web/index.html` directly. The browser fallback uses disposable demo data and never writes client configuration.

## Architecture

```text
Connection + model catalog
        │
        ├── VS Code Copilot adapter
        ├── Copilot CLI adapter
        └── GitHub Copilot app adapter
                │
                └── deployment plan / diff / result
```

The source of truth is client-neutral. Client-specific fields belong to adapters and deployment plans rather than being copied into three separate provider catalogs.

See [`docs/architecture.md`](docs/architecture.md) for the current boundaries and next extraction steps.

## Safety model

- Every supported write is previewed as a deployment plan.
- VS Code writes preserve non-managed groups and create `chatLanguageModels.json.pilotweave.bak` before the first change.
- Files are replaced atomically where supported.
- Secrets are retrieved only while applying a plan and are never returned to the frontend.
- GitHub Copilot app provider mutation remains disabled until PilotWeave can use a stable, supportable interface.

## License

MIT
