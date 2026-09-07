# September 2026 hardening slice

This describes the code in this slice, not a claim that the complete MVP is finished.

## Implemented

- Shared bounded file reads and safe replacement. Windows uses replace-existing/write-through, never unlink-before-rename. Unix sensitive files are created with mode 0600. Managed writes take an OS-held lease; crashes release the lease without deleting the lock file.
- A validated last-known-good application-state snapshot is available read-only when the primary is invalid. External state edits are rejected; aggregate serialized size is checked before keyring changes.
- Provider credential references are derived from the connection ID. They cannot name PilotWeave's GitHub authorization entry. Credential-store unavailability is distinct from Missing. Deletion reports credential cleanup warnings separately.
- The frontend submits only the exact reviewed deployment plan ID and explicit confirmation. Selection changes and late preview responses invalidate the plan. Native plans have 15-minute TTLs and bounded pending counts.
- Deployment snapshots contain the actual configuration resources: VS Code files, Unix CLI environment/shell files, or typed Windows environment values. Shared profile paths are deduplicated. Preparation parses every target before any target mutation; semantic no-op preserves JSON5 bytes.
- A private version-2 journal persists expected before/after data before writes. Later write or final audit failure triggers independent reverse compensation. External edits are not overwritten. Recovery previews are one-shot and restrict journal resources to locally discovered supported paths/variables.
- Slow installation, account and deployment commands run in blocking workers rather than on the Tauri UI thread. Capture limits include descendant-held pipes; interactive login and noninteractive probes have distinct environment policies.
- Authorization attempts use revision checks and disk conflict detection. Clearing invalidates in-flight responses. Revocation/network/schema failure of the stored authorization is persisted separately from its last successful validation.
- Personal Billing is compiled, registered and connected to the Official usage page. Account keys use host/user ID; queries and stale fallback are restricted to the requested month and endpoint family. Decimal strings are displayed without floating-point money arithmetic.
- CI checks fixed source and never searches/replaces business code or automatically commits repairs. Frontend plan-state regressions use Node's test runner.

## Remaining required-scope work

- Installation publisher verification, pinned asset/version plans, richer progress/cancellation and clean Windows installation acceptance.
- Verified or explicit user-confirmed three-client account reconciliation; opening an application is not authentication success.
- Persistent installation-owner evidence and conflict/adoption UX for legacy managed projections. This slice does not claim marker-only ownership is sufficient.
- Official runtime quota/model discovery, local token import, immutable price ingestion/estimation and shared resource synchronization.
- A guided last-good state restoration action. Last-good fallback is intentionally read-only and never silently overwrites the corrupt primary.
- Real Windows client end-to-end testing. CI compilation is not proof that VS Code/Copilot consumes a generated configuration successfully.

Version-1 interrupted deployment journals are rejected for automatic recovery. They remain preserved for manual review. Journal data may include materialized credentials and must never be attached to public issues.

## Regression checks

Run `npm run check:web`, `cargo fmt --all -- --check`, `cargo test --workspace --locked`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, and the Windows native Tauri build. Tests use temporary directories/fake credentials; no tests install applications or log in to real accounts.
