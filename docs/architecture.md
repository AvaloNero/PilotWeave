# Architecture

## Product model

PilotWeave is organized around four domain concepts rather than around an application switcher.

### Connection

A provider endpoint, protocol, headers, secret reference, and discovered or manually defined models. A connection is client-neutral.

### Client target

A concrete destination such as the default VS Code Stable profile, a named VS Code profile, the Copilot CLI user environment, or the GitHub Copilot app installation.

### Deployment plan

A deterministic preview of the operations needed to project one connection into one or more client targets. Plans contain descriptions and non-secret changes only.

### Deployment record

The local audit entry produced after an apply attempt. It records the target, outcome, timestamp, and diagnostic text, but never the provider secret.

## Backend layout

```text
src/
├── commands.rs            Tauri command boundary
├── domain.rs              client-neutral state and API types
├── error.rs               structured errors
├── secrets.rs             operating-system credential store
├── state.rs               atomic local persistence
└── adapters/
    ├── vscode.rs           profiles and chatLanguageModels.json
    ├── copilot_cli.rs      provider environment projection
    └── github_app.rs       installation detection / manual boundary
```

Adapters are the only modules allowed to know native client file layouts or environment variables.

## Current ownership markers

VS Code groups written by PilotWeave contain:

```json
{
  "vendor": "customendpoint",
  "pilotWeaveManaged": true,
  "pilotWeaveConnectionId": "..."
}
```

Only groups carrying `pilotWeaveManaged: true` are replaced or removed. Existing user groups and groups owned by other tools are retained.

## State and credentials

Non-secret state is stored below the platform configuration directory in `PilotWeave/state.json`. Each connection stores an opaque secret reference. The corresponding API key is stored using the native credential backend through `keyring-rs`.

The frontend receives `hasSecret`, never a secret value. The backend obtains the secret only while creating or applying a deployment.

## Next milestones

1. Extract the domain and deployment planner into a standalone `pilotweave-core` crate.
2. Import existing CC Switch Copilot catalogs and transfer management ownership safely.
3. Add model discovery and capability enrichment.
4. Add Skills, MCP, and Instructions as shared resources with per-client bindings.
5. Add session and token usage importers.
6. Replace the GitHub Copilot app manual boundary when a stable provider-management interface can be validated.
