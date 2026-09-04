from __future__ import annotations

from pathlib import Path
from textwrap import dedent
import re

ROOT = Path.cwd()


def replay_embedded_python(path: Path, *, required: bool = False) -> bool:
    if not path.exists():
        if required:
            raise RuntimeError(f"required workflow is missing: {path}")
        return False
    text = path.read_text()
    marker = "python3 - <<'PY'\n"
    if marker not in text:
        if required:
            raise RuntimeError(f"workflow has no embedded Python patch: {path}")
        return False
    body = text.split(marker, 1)[1]
    match = re.search(r"\n\s+PY\n", body)
    if not match:
        raise RuntimeError(f"could not locate embedded Python terminator: {path}")
    script = dedent(body[: match.start()])
    namespace = {"__name__": f"embedded_{path.stem}"}
    exec(compile(script, str(path), "exec"), namespace)
    return True


# A failed one-shot workflow leaves its patch in the repository. Replaying it
# makes this finalizer independent of whether the earlier run completed.
stabilizer = ROOT / ".github/workflows/stabilize-account-backend-once.yml"
if stabilizer.exists():
    try:
        replay_embedded_python(stabilizer)
    except Exception as error:  # fallback below handles the critical invariants
        print(f"warning: backend stabilizer replay was not fully applicable: {error}")

ui_workflow = ROOT / ".github/workflows/finish-account-ui-once-20260905.yml"
if ui_workflow.exists():
    replay_embedded_python(ui_workflow, required=True)

account_path = ROOT / "apps/desktop/src-tauri/src/account.rs"
native_path = ROOT / "apps/desktop/src-tauri/src/native_process.rs"
commands_path = ROOT / "apps/desktop/src-tauri/src/commands.rs"
lib_path = ROOT / "apps/desktop/src-tauri/src/lib.rs"
app_path = ROOT / "apps/desktop/web/app.js"
index_path = ROOT / "apps/desktop/web/index.html"
docs_path = ROOT / "docs/account-orchestration.md"

for path in [
    account_path,
    native_path,
    commands_path,
    lib_path,
    app_path,
    index_path,
]:
    if not path.exists():
        raise RuntimeError(f"missing required account-slice file: {path}")

account = account_path.read_text()

# Persisting a new in-progress record is transactional with the in-memory
# history. A failed disk write must restore the full previous state.
begin_start = account.index("    pub fn begin_run(")
begin_end = account.index("    pub fn finish_run(", begin_start)
begin_block = account[begin_start:begin_end]
if "let previous = self.state.clone();" not in begin_block:
    marker = "        self.ensure_writable()?;\n        let run = LoginRunRecord {"
    if marker not in begin_block:
        raise RuntimeError("could not locate LoginStore::begin_run insertion point")
    begin_block = begin_block.replace(
        marker,
        "        self.ensure_writable()?;\n"
        "        let previous = self.state.clone();\n"
        "        let run = LoginRunRecord {",
        1,
    )
if "self.state.runs.retain(|item| item.id != run.id);" in begin_block:
    begin_block = begin_block.replace(
        "self.state.runs.retain(|item| item.id != run.id);",
        "self.state = previous;",
        1,
    )
account = account[:begin_start] + begin_block + account[begin_end:]

# Diagnostics may contain multibyte text. Never slice a UTF-8 string at an
# arbitrary byte boundary.
account = account.replace(
    '        format!("{}…", &value[..MAX_ACCOUNT_TEXT_BYTES])',
    '''        let mut end = MAX_ACCOUNT_TEXT_BYTES.min(value.len());
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &value[..end])''',
    1,
)

# Bound avatar URLs before returning or persisting identity metadata.
avatar_start = account.index("fn validate_avatar_url")
avatar_end = account.index("fn bounded_detail", avatar_start)
avatar_block = account[avatar_start:avatar_end]
if "value.len() > MAX_ACCOUNT_TEXT_BYTES" not in avatar_block:
    avatar_block = avatar_block.replace(
        "fn validate_avatar_url(value: String) -> Option<String> {\n",
        "fn validate_avatar_url(value: String) -> Option<String> {\n"
        "    if value.len() > MAX_ACCOUNT_TEXT_BYTES {\n"
        "        return None;\n"
        "    }\n",
        1,
    )
    account = account[:avatar_start] + avatar_block + account[avatar_end:]

account = account.replace(
    '''    run.status = match (launched, failed) {
        (0, 0) => LoginRunStatus::Failed,
        (0, _) => LoginRunStatus::Failed,
        (_, 0) => LoginRunStatus::ActionRequired,
        _ => LoginRunStatus::Partial,
    };''',
    '''    run.status = if launched == 0 {
        LoginRunStatus::Failed
    } else if failed == 0 {
        LoginRunStatus::ActionRequired
    } else {
        LoginRunStatus::Partial
    };''',
    1,
)
account = account.replace(
    "detail: redact::redact_text(&error.to_string()),",
    "detail: bounded_detail(&error.to_string()),",
)

# Prefer the official macOS CLI launchers. Electron remains a bounded fallback
# for installations whose CLI link is missing.
if "/Contents/Resources/app/bin/code" not in account:
    marker = '                "/Applications/Visual Studio Code.app/Contents/MacOS/Electron",'
    if marker in account:
        account = account.replace(
            marker,
            '                "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code",\n'
            '                "/Applications/Visual Studio Code - Insiders.app/Contents/Resources/app/bin/code-insiders",\n'
            + marker,
            1,
        )

if "home.join(\"Applications/GitHub Copilot.app/Contents/MacOS/GitHub Copilot\")" not in account:
    marker = '''    #[cfg(target_os = "macos")]
    {
        native_process::resolve_candidates([
            "/Applications/GitHub Copilot.app/Contents/MacOS/GitHub Copilot",
        ])
    }'''
    if marker in account:
        replacement = '''    #[cfg(target_os = "macos")]
    {
        let system = native_process::resolve_candidates([
            "/Applications/GitHub Copilot.app/Contents/MacOS/GitHub Copilot",
        ]);
        system.or_else(|| {
            dirs::home_dir().and_then(|home| {
                native_process::resolve_regular_file(
                    home.join("Applications/GitHub Copilot.app/Contents/MacOS/GitHub Copilot"),
                )
            })
        })
    }'''
        account = account.replace(marker, replacement, 1)

if "fn bounded_detail_never_splits_utf8" not in account:
    marker = "    #[test]\n    fn invalid_github_logins_are_rejected() {"
    if marker in account:
        test = '''    #[test]
    fn bounded_detail_never_splits_utf8() {
        let value = "🙂".repeat(MAX_ACCOUNT_TEXT_BYTES);
        let detail = bounded_detail(&value);
        assert!(detail.ends_with('…'));
        assert!(detail.len() <= MAX_ACCOUNT_TEXT_BYTES + '…'.len_utf8());
    }

'''
        account = account.replace(marker, test + marker, 1)

account_path.write_text(account)

native = native_path.read_text()

# Non-interactive probes disable prompts. Official interactive login launchers
# remove that override while still clearing inherited authentication material.
if "sanitize_noninteractive_child_environment" not in native:
    first = native.find("    sanitize_child_environment(&mut command);")
    if first < 0:
        raise RuntimeError("could not locate non-interactive child sanitization")
    native = (
        native[:first]
        + "    sanitize_noninteractive_child_environment(&mut command);"
        + native[first + len("    sanitize_child_environment(&mut command);") :]
    )
    second = native.find("    sanitize_child_environment(&mut command);")
    if second < 0:
        raise RuntimeError("could not locate interactive child sanitization")
    native = (
        native[:second]
        + "    sanitize_interactive_child_environment(&mut command);"
        + native[second + len("    sanitize_child_environment(&mut command);") :]
    )

    env_start = native.index("pub fn sanitize_child_environment")
    drain_start = native.index("fn drain_capped", env_start)
    prefix = native[:env_start]
    middle = native[env_start:drain_start]
    hash_index = middle.find("fn hash_sample")
    suffix_helpers = middle[hash_index:] if hash_index >= 0 else ""
    replacement = '''fn clear_sensitive_child_environment(command: &mut Command) {
    for name in SENSITIVE_CHILD_ENV {
        command.env_remove(name);
    }
    command.env("NO_COLOR", "1");
}

fn sanitize_noninteractive_child_environment(command: &mut Command) {
    clear_sensitive_child_environment(command);
    command.env("GH_PROMPT_DISABLED", "1");
}

fn sanitize_interactive_child_environment(command: &mut Command) {
    clear_sensitive_child_environment(command);
    command.env_remove("GH_PROMPT_DISABLED");
}

'''
    native = prefix + replacement + suffix_helpers + native[drain_start:]

# Reap detached child processes so short-lived official launchers do not leave
# zombies on Unix-like systems.
spawn_pattern = re.compile(
    r"    command\n        \.spawn\(\)\n        \.map\(\|_\| \(\)\)\n        \.map_err\(\|error\| AppError::io\(&executable, error\)\)\n\}",
    re.S,
)
if spawn_pattern.search(native):
    native = spawn_pattern.sub(
        '''    let mut child = command
        .spawn()
        .map_err(|error| AppError::io(&executable, error))?;
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}''',
        native,
        count=1,
    )

native_path.write_text(native)

required_symbols = {
    commands_path: ["get_account_status", "preview_login", "apply_login_plan"],
    lib_path: [
        "commands::get_account_status",
        "commands::preview_login",
        "commands::apply_login_plan",
    ],
    app_path: [
        "function renderAccounts()",
        "function openLoginPlan()",
        'case "get_account_status":',
        'accountSnapshot = await invoke("get_account_status")',
    ],
    index_path: ['data-route="accounts"'],
    account_path: ["pub struct LoginPlan", "pub struct LoginRunRecord"],
    native_path: [
        "sanitize_noninteractive_child_environment",
        "sanitize_interactive_child_environment",
    ],
}
for path, symbols in required_symbols.items():
    text = path.read_text()
    missing = [symbol for symbol in symbols if symbol not in text]
    if missing:
        raise RuntimeError(f"{path} is missing required symbols: {missing}")

if not docs_path.exists():
    raise RuntimeError("account orchestration documentation was not created")

print("account orchestration slice is present and normalized")
