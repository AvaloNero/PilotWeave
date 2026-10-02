# Deployment, Resources, installation, and account hardening

This document records implemented behavior as of 2026-10-02. The [MVP specification](mvp-implementation-spec.md) remains normative. This is not a clean-machine release acceptance claim.

## Implemented native behavior

- `state.json` v1 migrates in memory to schema v2 without losing Connections, audits, or installation identity. v2 stores bounded durable ownership proofs and authored resource metadata. Invalid primary/last-known-good state remains read-only.
- VS Code groups bind both installation owner and Connection to a canonical physical target and semantic group hash. Legacy markers, foreign owners, copied projections, duplicate claims, and changed owned content cannot authorize replacement or revoke.
- CLI user-environment/managed-shell ownership binds the complete prepared projection. Switching an owned CLI Connection replaces its physical claim; unmanaged/legacy overrides are not silently adopted. The public `providers.json` precedence and custom configuration roots are checked before legacy environment deployment. Revocation can still remove a proven owned projection when a newer public provider registry shadows it.
- Delete offers two explicit workflows: detach local metadata only, or review/revoke all proven owned projections and then delete. Journal, rollback, post-write verification, ownership/audit commit, and credential cleanup have distinct failure outcomes. A detach-only action leaves materialized credentials/client configuration intact.
- Resources have real native CRUD and publication. Saving is local-only. MCP, Markdown-only Skills, and Instructions use native-held expiring one-shot plans; all supported targets are prepared before mutation. Review shows the concrete destination, create/update/remove/no-op status, and only the owned projected content; unrelated MCP entries or credentials never enter preview DTOs. MCP preserves unknown root fields/user entries. Owned Markdown refuses external changes. Revocation removes only proven projections and retains bounded identity-only recovery tombstones.
- Installation history is prepared before launching anything and stores only typed status/result metadata. Scoped cancellation terminates PilotWeave-owned captured children; completed installations are not removed. Timeout, partial, failed verification, cancellation, and interruption remain distinct.
- Installation binds native-discovered executable bytes and a monotonic deadline. Unknown identity never authorizes replacement. VS Code uses its supported CLI layout/metadata, CLI uses a bounded version-only probe, and the app uses bounded PE version/product data without launching its GUI. Version resources are not claimed as cryptographic publisher proof.
- CLI login follow-up uses only reviewed read-only `connect`/`auth.getStatus`, supported github.com user/gh-cli authentication modes, and bounded login metadata. Matching identity, conflict, missing/unsupported evidence, timeout, and cancellation are distinct. VS Code/app accounts remain client-specific manual checks. Opening a flow is never login success.

## Reviewed package policy

The Windows x64 catalog is fixed to these inspected WinGet manifests, rather than a remote latest-version argument:

| Component | Package/version | Reviewed dependency/scope |
| --- | --- | --- |
| VS Code | Microsoft.VisualStudioCode 1.140.0 | User installation |
| CLI | GitHub.Copilot v1.0.90 | Existing PowerShell 7+, portable package |
| Copilot app | GitHub.CopilotApp 1.1.25 | User installation |

Native argument vectors select exact ID/version/source/architecture, disable automatic dependency installation, and never bypass WinGet hash checks. The source-export v1 object must match the fixed Microsoft CDN, type, data and identifier; its fingerprint is rechecked after preview and before every installation step. A custom repository named `winget` is not accepted. The installed package manager must pass Windows Authenticode with a Microsoft Corporation signer. Cached chain/revocation failures block execution; no signature bypass or arbitrary fallback command exists. The direct downloaded-asset strategy is not implemented; do not claim independent app-asset publisher/download-policy validation from the package-manager path.

Provenance inspected on 2026-10-02:

- [VS Code manifest](https://github.com/microsoft/winget-pkgs/blob/master/manifests/m/Microsoft/VisualStudioCode/1.140.0/Microsoft.VisualStudioCode.installer.yaml), [CLI manifest](https://github.com/microsoft/winget-pkgs/blob/master/manifests/g/GitHub/Copilot/v1.0.90/GitHub.Copilot.installer.yaml), [app manifest](https://github.com/microsoft/winget-pkgs/blob/master/manifests/g/GitHub/CopilotApp/1.1.25/GitHub.CopilotApp.installer.yaml).
- [WinGet installation options](https://learn.microsoft.com/en-us/windows/package-manager/winget/install), [source export](https://github.com/microsoft/winget-cli/blob/master/src/AppInstallerCLICore/Workflows/SourceFlow.cpp), [exported source fields](https://github.com/microsoft/winget-cli/blob/master/src/AppInstallerSharedLib/GroupPolicy.cpp), [WinVerifyTrust](https://learn.microsoft.com/en-us/windows/win32/api/wintrust/nf-wintrust-winverifytrust), [version-resource API](https://learn.microsoft.com/en-us/windows/win32/api/winver/nf-winver-verqueryvaluew), [registered package query](https://learn.microsoft.com/en-us/windows/win32/api/appmodel/nf-appmodel-getpackagesbypackagefamily).
- [CLI public configuration roots and registry precedence](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference), [CLI MCP](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-mcp-servers), [VS Code MCP](https://code.visualstudio.com/docs/agent-customization/mcp-servers), [VS Code Skills](https://code.visualstudio.com/docs/agent-customization/agent-skills).
- [Official SDK auth method](https://github.com/github/copilot-sdk/blob/main/nodejs/src/client.ts), [auth response fields](https://github.com/github/copilot-sdk/blob/main/nodejs/src/types.ts). Only protocol 3 is accepted. Unknown authType/protocol is Unsupported, never best-effort identity verification.

Windows API support is enabled through the existing `windows-sys` dependency to avoid shell signature/version probes. No new package dependency is added.

## Security and regression coverage

Temporary-directory tests cover owner/projection tampering, semantic no-op preservation, native one-shot/revision checks, stale apply, prepare-all behavior, compensation, refusal to overwrite external changes, state reopen, resource tombstones, typed status aggregation, cancellation before spawn/while waiting for capture, invalid publisher rejection, unsupported version output, and login history compare-before-replace.

The isolated native suite additionally checks actual WebView2-to-Tauri IPC for resource publication/revoke, persisted installation history, CLI identity follow-up, cancellation persistence, and reviewed Connection revocation. It uses inert executables, fake credential/registry/signature/process adapters, sanitized usage data, and loopback HTTP fixtures. It never installs a real application or opens real credential/usage stores.

On Windows without symlink privileges, the sensitive-source symlink regression reports an explicit local skip for error 1314/permission denial. CI sets `PILOTWEAVE_TEST_REQUIRE_LINKS=1` so inability to exercise that case fails CI instead of silently reducing coverage.

## Required-scope acceptance boundaries

1. **Clean Windows 11 x64 acceptance:** not established by temporary fixtures or the existing developer workstation. Use the dedicated-user/VM workflow in [local validation](local-validation-plan.md); real install and official account actions require maintainer participation. Do not install/uninstall on the developer workstation as a test substitute.
2. **Package and publisher acceptance:** exercise registered WinGet resolution, cached signature validation, pinned manifests, package-manager download/identity policy, elevation/cancellation, and actual app product metadata on the supported clean machine. The direct GitHub app asset fallback and independent expected asset-publisher policy remain unimplemented. Unknown observations remain Unknown.
3. **Client-specific identity:** CLI protocol-3 public auth is supported; VS Code/app do not expose a verified token-free account interface in this implementation. Manual checks retain manual evidence, not Verified. Do not inspect cookies, SecretStorage, OAuth tokens, or opaque app databases.
4. **Resource consumption:** public-path publication is not universal client synchronization. Named VS Code profiles, overridden roots, MCP local commands/credentials, supporting script assets, and app-specific resource configuration remain manual. Validate actual supported client discovery/precedence before broadening adapters.
5. **Usage semantics:** existing safe importer/runtime/Billing/pricing behavior remains intact. Missing CLI fresh-vs-total semantics, omitted VS Code cache writes, unsupported app-local usage, and ambiguous route/Connection attribution remain Unknown/Partial/Unsupported. No timeline heuristic is added without sufficient stable native evidence.

No organization Billing, provider invoices, cloud usage sync, overage settings, or conversation-content collection is introduced.
