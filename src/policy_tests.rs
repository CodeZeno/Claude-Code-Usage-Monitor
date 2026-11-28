//! Repository policy that a review could miss: where credentials come from,
//! what the app persists, which hosts it contacts, which programs it starts
//! and which native networking or secret-storing APIs it uses.
//!
//! Most checks read the source, so they fail on the pull request that adds
//! something new. When one fails, the fix is either to remove the addition or,
//! if it is intended, to add it to the list here with a reason, where the
//! maintainer reviews it.

use crate::providers::{CredentialSource, ProviderId};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Credentials
// ---------------------------------------------------------------------------

/// Credentials that the user copies in by hand, kept only for the providers
/// that relied on them before the rule that providers use existing sign-ins.
/// Do not add to this list: a new provider must read a sign-in that the
/// provider's own CLI, desktop app or IDE already stores.
const USER_SUPPLIED_EXCEPTIONS: &[(ProviderId, &str)] = &[
    (ProviderId::OpenCode, "OPENCODE_GO_AUTH_COOKIE"),
    (ProviderId::OpenCode, "OPENCODE_GO_WORKSPACE_ID"),
    (
        ProviderId::OpenCode,
        "%APPDATA%/opencode-go/config.json or OPENCODE_GO_CONFIG_FILE",
    ),
    (ProviderId::Cursor, "CURSOR_SESSION_TOKEN"),
];

/// Providers with no existing sign-in to read. Do not add to this list.
const USER_SUPPLIED_ONLY: &[ProviderId] = &[ProviderId::OpenCode];

#[test]
fn providers_use_credentials_already_on_this_machine() {
    let mut problems = Vec::new();
    let mut used = BTreeSet::new();
    for provider in ProviderId::ALL {
        let descriptor = provider.descriptor();
        let key = descriptor.key;
        if descriptor.credentials.is_empty() {
            problems.push(format!(
                "{key}: list where the provider reads its sign-in in `credentials` \
                 (src/providers.rs)"
            ));
        }
        let mut existing = false;
        for source in descriptor.credentials {
            match source {
                CredentialSource::UserSupplied(name) => {
                    if USER_SUPPLIED_EXCEPTIONS.contains(&(provider, name)) {
                        used.insert((provider, *name));
                    } else {
                        problems.push(format!(
                            "{key}: {name:?} is a credential the user enters by hand. Providers \
                             must read a sign-in that the provider's own CLI, desktop app or IDE \
                             already stores on this machine; the app never asks for, stores or \
                             writes credentials"
                        ));
                    }
                }
                _ => existing = true,
            }
        }
        if !existing && !USER_SUPPLIED_ONLY.contains(&provider) {
            problems.push(format!(
                "{key}: reads no existing sign-in. Read the credentials that the provider's \
                 own CLI, desktop app or IDE stores (a sign-in file, Windows Credential Manager \
                 or an environment variable its tools already use)"
            ));
        }
    }
    for (provider, name) in USER_SUPPLIED_EXCEPTIONS {
        if !used.contains(&(*provider, *name)) {
            problems.push(format!(
                "{}: the exception for {name:?} is no longer used; remove it from \
                 USER_SUPPLIED_EXCEPTIONS",
                provider.descriptor().key
            ));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Words that mark an environment variable or persisted field as a secret.
fn looks_secret(name: &str) -> bool {
    let lower = name.to_ascii_lowercase().replace(['-', '_'], "");
    [
        "token",
        "cookie",
        "secret",
        "password",
        "passwd",
        "apikey",
        "accesskey",
        "privatekey",
        "bearer",
        "authorization",
        "credential",
    ]
    .iter()
    .any(|word| lower.contains(word))
}

#[test]
fn credential_environment_variables_are_declared() {
    let declared: BTreeSet<&str> = ProviderId::ALL
        .iter()
        .flat_map(|provider| provider.descriptor().credentials)
        .filter_map(|source| match source {
            CredentialSource::ToolEnvironment(name) | CredentialSource::UserSupplied(name) => {
                Some(*name)
            }
            _ => None,
        })
        .collect();
    let mut problems = Vec::new();
    for source in production_sources() {
        for literal in string_literals(&source.text) {
            let environment_name = !literal.is_empty()
                && literal
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                && literal.contains('_');
            if environment_name && looks_secret(&literal) && !declared.contains(literal.as_str()) {
                problems.push(format!(
                    "{}: {literal:?} looks like a credential environment variable. Declare it in \
                     the provider's `credentials` (src/providers.rs) as a ToolEnvironment that \
                     the provider's own tools already read; the app does not define its own",
                    source.path
                ));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn persisted_settings_and_cache_hold_no_credentials() {
    let mut names = BTreeSet::new();
    probe::field_names::<crate::app_settings::SettingsFile>(&mut names);
    // Prove that the probe reached nested types.
    for expected in ["poll_interval_ms", "credentials_path", "monitor_device"] {
        assert!(
            names.contains(expected),
            "the settings probe did not reach {expected:?}; found {names:?}"
        );
    }
    probe::field_names::<crate::app_settings::UsageCache>(&mut names);
    assert!(names.contains("poll_ok"), "the cache probe found {names:?}");

    // A path to a credential file the provider's own client wrote is allowed;
    // the credential itself is not.
    let problems: Vec<_> = names
        .iter()
        .filter(|name| looks_secret(name))
        .filter(|name| !(name.ends_with("_path") || name.ends_with("_dir")))
        .collect();
    assert!(
        problems.is_empty(),
        "settings.json or the usage cache would store a credential in {problems:?}. The app \
         never stores credentials: read them from where the provider's own client keeps them, \
         or store only the path to that file"
    );
}

#[test]
fn pollers_never_write_files() {
    const WRITES: &[&str] = &[
        "fs::write",
        "File::create",
        "OpenOptions",
        "fs::rename",
        "fs::copy",
        "fs::remove_file",
        "set_permissions",
        "CredWrite",
        "CryptProtectData",
    ];
    /// Reviewed exceptions, as (file, API, why).
    const EXCEPTIONS: &[(&str, &str, &str)] = &[
        (
            "src/poller/cursor.rs",
            "fs::copy",
            "reads a temporary copy of Cursor's state database while Cursor holds it locked",
        ),
        (
            "src/poller/cursor.rs",
            "fs::remove_file",
            "deletes that temporary copy straight after reading it",
        ),
    ];
    let mut problems = Vec::new();
    for source in production_sources() {
        if !source.path.starts_with("src/poller") {
            continue;
        }
        let code = without_line_comments(&source.text);
        for needle in WRITES {
            let excepted = EXCEPTIONS
                .iter()
                .any(|(file, api, _)| *file == source.path && api == needle);
            if code.contains(needle) && !excepted {
                problems.push(format!(
                    "{}: uses {needle}. Pollers read provider credentials and never write files \
                     or credentials; a provider's own CLI or app refreshes its sign-in",
                    source.path
                ));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

// ---------------------------------------------------------------------------
// Network and processes
// ---------------------------------------------------------------------------

/// Every host named in the shipped source or bundled themes, and why.
const ALLOWED_HOSTS: &[(&str, &str)] = &[
    ("api.anthropic.com", "Claude usage and rate-limit headers"),
    ("claude.ai", "theme reference example link"),
    ("chatgpt.com", "Codex usage"),
    ("oauth2.googleapis.com", "Antigravity token refresh"),
    ("cloudcode-pa.googleapis.com", "Antigravity usage"),
    ("daily-cloudcode-pa.googleapis.com", "Antigravity usage"),
    (
        "daily-cloudcode-pa.sandbox.googleapis.com",
        "Antigravity usage",
    ),
    ("opencode.ai", "OpenCode Go usage"),
    ("cursor.com", "Cursor usage"),
    ("api2.cursor.sh", "Cursor token refresh"),
    ("cli-chat-proxy.grok.com", "Grok usage"),
    ("accounts.x.ai", "Grok token refresh"),
    ("auth.x.ai", "Grok sign-in issuer in auth.json"),
    ("api.github.com", "Copilot usage and release checks"),
    (
        "github.com",
        "Copilot sign-in targets, release downloads, project links",
    ),
    ("example.com", "documentation examples"),
];

/// Hosts only in tests, which never run in the shipped app.
const TEST_HOSTS: &[&str] = &["127.0.0.1", "localhost", "example.com", "example.invalid"];
/// Host suffixes only in tests: TLS checks against badssl.com run only when
/// asked for (`#[ignore]`).
const TEST_HOST_SUFFIXES: &[&str] = &[".example.com", ".badssl.com"];

#[test]
fn outbound_hosts_are_listed() {
    let allowed: BTreeSet<&str> = ALLOWED_HOSTS.iter().map(|(host, _)| *host).collect();
    let mut seen = BTreeSet::new();
    let mut problems = Vec::new();
    let mut files = production_sources();
    files.extend(bundled_theme_sources());
    for source in &files {
        for (scheme, host) in urls(&source.text) {
            if host.is_empty() {
                continue;
            }
            seen.insert(host.clone());
            if scheme != "https" {
                problems.push(format!(
                    "{}: {scheme}://{host} is not HTTPS; the app only uses TLS",
                    source.path
                ));
            } else if !allowed.contains(host.as_str()) {
                problems.push(format!(
                    "{}: https://{host} is not in ALLOWED_HOSTS (src/policy_tests.rs). Add it \
                     with its purpose if the provider or feature needs it; the maintainer \
                     reviews every new host",
                    source.path
                ));
            }
        }
        if source.path.ends_with(".rs") && source.text.contains("://{") {
            problems.push(format!(
                "{}: builds a URL host at run time; name the host in a literal so this test \
                 can check it",
                source.path
            ));
        }
    }
    for host in &allowed {
        if !seen.contains(*host) {
            problems.push(format!(
                "{host} is no longer used; remove it from ALLOWED_HOSTS"
            ));
        }
    }
    for source in test_sources() {
        if source.path == "src/policy_tests.rs" {
            continue;
        }
        for (scheme, host) in urls(&source.text) {
            let known = allowed.contains(host.as_str())
                || TEST_HOSTS.contains(&host.as_str())
                || TEST_HOST_SUFFIXES
                    .iter()
                    .any(|suffix| host.ends_with(suffix))
                || host.is_empty();
            if !known && scheme != "file" {
                problems.push(format!(
                    "{}: tests contact {scheme}://{host}; use 127.0.0.1, example.com or a \
                     listed host",
                    source.path
                ));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Programs the app starts, as (file, the argument to `Command::new`).
const PROCESS_LAUNCHES: &[(&str, &str, &str)] = &[
    ("src/dashboard.rs", "executable", "the dashboard process"),
    (
        "src/studio_app.rs",
        "executable",
        "restart with the WARP renderer",
    ),
    ("src/window/watchdog.rs", "exe", "relaunch after a hang"),
    (
        "src/updater.rs",
        "\"powershell.exe\"",
        "run the WinGet upgrade",
    ),
    ("src/updater.rs", "&helper_path", "the update helper"),
    ("src/updater.rs", "target", "start the updated executable"),
    (
        "src/updater.rs",
        "\"winget.exe\"",
        "check the WinGet version",
    ),
    (
        "src/poller/claude.rs",
        "\"where.exe\"",
        "find the Claude CLI",
    ),
    (
        "src/poller/claude.rs",
        "\"wsl.exe\"",
        "read Claude credentials in WSL",
    ),
    ("src/poller/claude/cli.rs", "\"cmd.exe\"", "run claude.cmd"),
    (
        "src/poller/claude/cli.rs",
        "path",
        "the Claude CLI refreshes its sign-in",
    ),
    (
        "src/poller/claude/cli.rs",
        "\"wsl.exe\"",
        "the Claude CLI in WSL",
    ),
    ("src/poller/codex.rs", "\"cmd.exe\"", "run codex.cmd"),
    ("src/poller/codex.rs", "\"powershell.exe\"", "run codex.ps1"),
    (
        "src/poller/codex.rs",
        "&codex_path",
        "the Codex CLI refreshes its sign-in",
    ),
    ("src/poller/codex.rs", "name", "probe for the Codex CLI"),
    ("src/poller/codex.rs", "\"where.exe\"", "find the Codex CLI"),
    ("src/poller/grok.rs", "\"cmd.exe\"", "run grok.cmd"),
    ("src/poller/grok.rs", "\"powershell.exe\"", "run grok.ps1"),
    (
        "src/poller/grok.rs",
        "grok_path",
        "the Grok CLI refreshes its sign-in",
    ),
    ("src/poller/grok.rs", "\"where.exe\"", "find the Grok CLI"),
];

#[test]
fn process_launches_are_listed() {
    let listed: BTreeSet<(&str, &str)> = PROCESS_LAUNCHES
        .iter()
        .map(|(file, argument, _)| (*file, *argument))
        .collect();
    let mut seen = BTreeSet::new();
    let mut problems = Vec::new();
    for source in production_sources() {
        let mut rest = source.text.as_str();
        while let Some(start) = rest.find("Command::new(") {
            rest = &rest[start + "Command::new(".len()..];
            let argument = rest[..rest.find(')').unwrap_or(rest.len())]
                .trim()
                .to_string();
            if listed.contains(&(source.path.as_str(), argument.as_str())) {
                seen.insert((source.path.clone(), argument));
            } else {
                problems.push(format!(
                    "{}: starts Command::new({argument}), which is not in PROCESS_LAUNCHES \
                     (src/policy_tests.rs). Add it with its purpose if it is needed; programs \
                     such as curl or PowerShell can send data anywhere, so the maintainer \
                     reviews every new launch",
                    source.path
                ));
            }
        }
    }
    for (file, argument) in &listed {
        if !seen.contains(&(file.to_string(), argument.to_string())) {
            problems.push(format!(
                "{file}: Command::new({argument}) is no longer used; remove it from \
                 PROCESS_LAUNCHES"
            ));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// APIs that could send data or store secrets, and the files allowed to use
/// them (an entry ending in `/` covers a folder). An empty list means nowhere.
const RESTRICTED_APIS: &[(&str, &[&str], &str)] = &[
    (
        "ShellExecute",
        &["src/native_interop.rs"],
        "open links only through native_interop::open_web_url",
    ),
    (
        "ureq::",
        &[
            "src/poller.rs",
            "src/poller/",
            "src/updater.rs",
            "src/updater/",
        ],
        "HTTP stays in poller/ and updater/",
    ),
    ("std::net", &[], "raw sockets bypass the HTTP policy"),
    ("TcpStream", &[], "raw sockets bypass the HTTP policy"),
    ("UdpSocket", &[], "raw sockets bypass the HTTP policy"),
    ("TcpListener", &[], "the app accepts no network connections"),
    (
        "WinHttp",
        &[],
        "a second HTTP stack bypasses the HTTP policy",
    ),
    (
        "InternetOpen",
        &[],
        "a second HTTP stack bypasses the HTTP policy",
    ),
    (
        "URLDownloadTo",
        &[],
        "a second HTTP stack bypasses the HTTP policy",
    ),
    ("WSAStartup", &[], "raw sockets bypass the HTTP policy"),
    (
        "Invoke-WebRequest",
        &[],
        "PowerShell web requests bypass the HTTP policy",
    ),
    (
        "Invoke-RestMethod",
        &[],
        "PowerShell web requests bypass the HTTP policy",
    ),
    (
        "Net.WebClient",
        &[],
        "PowerShell web requests bypass the HTTP policy",
    ),
    (
        "Start-BitsTransfer",
        &[],
        "background downloads bypass the HTTP policy",
    ),
    (
        "bitsadmin",
        &[],
        "background downloads bypass the HTTP policy",
    ),
    ("curl", &[], "curl bypasses the HTTP policy"),
    ("CredWrite", &[], "the app never stores credentials"),
    ("CryptProtectData", &[], "the app never stores credentials"),
    (
        "link!(",
        &[],
        "declare native imports with #[link] so this test sees them",
    ),
];

/// Native libraries linked by hand, as (file, library).
const NATIVE_LIBRARIES: &[(&str, &str)] = &[
    ("src/poller/windows_credentials.rs", "advapi32"),
    ("src/poller/claude_desktop.rs", "crypt32"),
    ("src/poller/claude_desktop.rs", "bcrypt"),
    ("src/winsqlite.rs", "winsqlite3"),
];

#[test]
fn restricted_apis_stay_where_they_are_reviewed() {
    let mut problems = Vec::new();
    let mut sources = production_sources();
    sources.push(build_script());
    for source in &sources {
        let code = without_line_comments(&source.text);
        for (needle, allowed, why) in RESTRICTED_APIS {
            let permitted = allowed.iter().any(|allowed| {
                *allowed == source.path
                    || (allowed.ends_with('/') && source.path.starts_with(allowed))
            });
            if code.contains(needle) && !permitted {
                problems.push(format!(
                    "{}: uses {needle}; {why}. If it is needed, add the file to RESTRICTED_APIS \
                     (src/policy_tests.rs) for the maintainer to review",
                    source.path
                ));
            }
        }
        let mut rest = code.as_str();
        while let Some(start) = rest.find("#[link(name = \"") {
            rest = &rest[start + "#[link(name = \"".len()..];
            let library = rest[..rest.find('"').unwrap_or(0)].to_ascii_lowercase();
            if !NATIVE_LIBRARIES.contains(&(source.path.as_str(), library.as_str())) {
                problems.push(format!(
                    "{}: links {library} by hand, which is not in NATIVE_LIBRARIES \
                     (src/policy_tests.rs)",
                    source.path
                ));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn build_script_starts_no_programs_and_contacts_nothing() {
    let build = build_script();
    let mut problems = Vec::new();
    for needle in [
        "Command",
        "std::net",
        "ureq",
        "TcpStream",
        "http://",
        "https://",
    ] {
        if build.text.contains(needle) {
            problems.push(format!(
                "build.rs uses {needle}; the build script only processes files in this \
                 repository"
            ));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn windows_features_include_no_networking() {
    let manifest = std::fs::read_to_string(root().join("Cargo.toml")).unwrap();
    let forbidden = [
        "Networking",
        "NetworkManagement",
        "Win32_Web",
        "WinHttp",
        "WinInet",
        "Security_Credentials",
    ];
    let problems: Vec<_> = string_literals(&manifest)
        .into_iter()
        .filter(|feature| feature.starts_with("Win32_"))
        .filter(|feature| forbidden.iter().any(|word| feature.contains(word)))
        .collect();
    assert!(
        problems.is_empty(),
        "Cargo.toml enables Windows networking or credential-storage APIs {problems:?}. HTTP \
         goes through ureq in poller/ and updater/, and the app never stores credentials"
    );
}

// ---------------------------------------------------------------------------
// Source scanning
// ---------------------------------------------------------------------------

struct Source {
    /// Path relative to the repository, with forward slashes.
    path: String,
    text: String,
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn relative(path: &Path) -> String {
    path.strip_prefix(root())
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

fn rust_files(directory: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            out.push(path);
        }
    }
}

fn is_test_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    path.contains("/tests/")
        || name == "tests.rs"
        || name.ends_with("_tests.rs")
        || name.ends_with("_test.rs")
}

/// Every Rust file under `src/` with test code removed, plus `build.rs`
/// handled separately by `build_script`.
fn production_sources() -> Vec<Source> {
    all_sources()
        .into_iter()
        .filter(|source| !is_test_file(&source.path))
        .map(|source| Source {
            text: without_test_modules(&source.text),
            path: source.path,
        })
        .collect()
}

fn test_sources() -> Vec<Source> {
    all_sources()
        .into_iter()
        .map(|source| {
            if is_test_file(&source.path) {
                source
            } else {
                Source {
                    text: test_modules(&source.text),
                    path: source.path,
                }
            }
        })
        .collect()
}

fn all_sources() -> Vec<Source> {
    let mut files = Vec::new();
    rust_files(&root().join("src"), &mut files);
    files.sort();
    files
        .into_iter()
        .map(|path| Source {
            text: std::fs::read_to_string(&path).unwrap(),
            path: relative(&path),
        })
        .collect()
}

fn build_script() -> Source {
    Source {
        path: "build.rs".into(),
        text: std::fs::read_to_string(root().join("build.rs")).unwrap(),
    }
}

fn bundled_theme_sources() -> Vec<Source> {
    let mut sources = Vec::new();
    for entry in std::fs::read_dir(root().join("src/themes")).unwrap() {
        let path = entry.unwrap().path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            sources.push(Source {
                text: std::fs::read_to_string(&path).unwrap(),
                path: relative(&path),
            });
        }
    }
    sources
}

/// Byte ranges of `#[cfg(test)]` items: an inline `mod`, `fn`, `impl` or
/// `use`, ending at its matching brace or semicolon.
fn test_item_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut from = 0;
    while let Some(found) = text[from..].find("#[cfg(test)]") {
        let start = from + found;
        let after = start + "#[cfg(test)]".len();
        let body = &text[after..];
        let brace = body.find('{');
        let semicolon = body.find(';');
        let end = match (brace, semicolon) {
            (Some(brace), Some(semicolon)) if semicolon < brace => after + semicolon + 1,
            (Some(brace), _) => {
                let mut depth = 0_i32;
                let mut end = text.len();
                for (offset, c) in body[brace..].char_indices() {
                    match c {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                end = after + brace + offset + 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                end
            }
            (None, Some(semicolon)) => after + semicolon + 1,
            (None, None) => text.len(),
        };
        ranges.push((start, end));
        from = end;
    }
    ranges
}

/// `text` without `//` and `///` comment lines, so a comment that names an
/// API does not count as using it.
fn without_line_comments(text: &str) -> String {
    text.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join(
            "
",
        )
}

fn without_test_modules(text: &str) -> String {
    let mut out = String::new();
    let mut last = 0;
    for (start, end) in test_item_ranges(text) {
        out.push_str(&text[last..start]);
        last = end;
    }
    out.push_str(&text[last..]);
    out
}

fn test_modules(text: &str) -> String {
    test_item_ranges(text)
        .into_iter()
        .map(|(start, end)| &text[start..end])
        .collect::<Vec<_>>()
        .join("\n")
}

/// The contents of double-quoted string literals, without escape handling.
fn string_literals(text: &str) -> Vec<String> {
    let mut literals = Vec::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '"' {
            let mut literal = String::new();
            let mut escaped = false;
            for c in chars.by_ref() {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    break;
                }
                literal.push(c);
            }
            literals.push(literal);
        }
    }
    literals
}

/// `(scheme, host)` for every `scheme://host` in `text`, lowercased.
fn urls(text: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(offset) = text[from..].find("://") {
        let at = from + offset;
        let scheme: String = text[..at]
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let host: String = text[at + 3..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
            .collect();
        if !scheme.is_empty() {
            found.push((
                scheme.to_ascii_lowercase(),
                host.trim_end_matches('.').to_ascii_lowercase(),
            ));
        }
        from = at + 3;
    }
    found
}

#[test]
fn source_scanning_finds_test_modules_and_urls() {
    let text = "fn a() {}\n#[cfg(test)]\nmod tests {\n    fn b() { \"https://x.example.com/p\" }\n}\nfn c() {}\n";
    assert_eq!(without_test_modules(text), "fn a() {}\n\nfn c() {}\n");
    assert_eq!(
        urls(&test_modules(text)),
        [("https".to_string(), "x.example.com".to_string())]
    );
    assert_eq!(string_literals(r#"a("X\"Y", "Z")"#), ["X\\\"Y", "Z"]);
    assert!(looks_secret("OPENAI_API_KEY") && looks_secret("authCookie"));
    assert!(!looks_secret("poll_interval_ms") && !looks_secret("session"));
}

/// Collects the field names a type deserializes, including nested types, by
/// driving its `Deserialize` impl with a deserializer that answers every
/// request with a placeholder value.
mod probe {
    use serde::de::{self, DeserializeSeed, IntoDeserializer, Visitor};
    use std::collections::BTreeSet;

    type Error = de::value::Error;
    const MAX_DEPTH: usize = 12;

    pub(super) fn field_names<'de, T: de::Deserialize<'de>>(names: &mut BTreeSet<String>) {
        // A Deserialize impl may reject a placeholder; the names seen up to
        // then are still collected, and the caller checks coverage.
        let _ = T::deserialize(Probe { names, depth: 0 });
    }

    struct Probe<'a> {
        names: &'a mut BTreeSet<String>,
        depth: usize,
    }

    impl Probe<'_> {
        fn deeper(&mut self) -> Probe<'_> {
            Probe {
                names: self.names,
                depth: self.depth + 1,
            }
        }
    }

    impl<'de> de::Deserializer<'de> for Probe<'_> {
        type Error = Error;

        fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            visitor.visit_unit()
        }
        fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            visitor.visit_bool(false)
        }
        fn deserialize_i8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            visitor.visit_i8(0)
        }
        fn deserialize_i16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            visitor.visit_i16(0)
        }
        fn deserialize_i32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            visitor.visit_i32(0)
        }
        fn deserialize_i64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            visitor.visit_i64(0)
        }
        fn deserialize_u8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            visitor.visit_u8(0)
        }
        fn deserialize_u16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            visitor.visit_u16(0)
        }
        fn deserialize_u32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            visitor.visit_u32(0)
        }
        fn deserialize_u64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            visitor.visit_u64(0)
        }
        fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            visitor.visit_f32(0.0)
        }
        fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            visitor.visit_f64(0.0)
        }
        fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            visitor.visit_char('a')
        }
        fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            visitor.visit_str("")
        }
        fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            visitor.visit_str("")
        }
        fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            visitor.visit_bytes(&[])
        }
        fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            visitor.visit_bytes(&[])
        }
        fn deserialize_option<V: Visitor<'de>>(mut self, visitor: V) -> Result<V::Value, Error> {
            if self.depth < MAX_DEPTH {
                visitor.visit_some(self.deeper())
            } else {
                visitor.visit_none()
            }
        }
        fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            visitor.visit_unit()
        }
        fn deserialize_unit_struct<V: Visitor<'de>>(
            self,
            _name: &'static str,
            visitor: V,
        ) -> Result<V::Value, Error> {
            visitor.visit_unit()
        }
        fn deserialize_newtype_struct<V: Visitor<'de>>(
            mut self,
            _name: &'static str,
            visitor: V,
        ) -> Result<V::Value, Error> {
            visitor.visit_newtype_struct(self.deeper())
        }
        fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            let count = usize::from(self.depth < MAX_DEPTH);
            visitor.visit_seq(Elements {
                probe: self,
                left: count,
            })
        }
        fn deserialize_tuple<V: Visitor<'de>>(
            self,
            len: usize,
            visitor: V,
        ) -> Result<V::Value, Error> {
            visitor.visit_seq(Elements {
                probe: self,
                left: len,
            })
        }
        fn deserialize_tuple_struct<V: Visitor<'de>>(
            self,
            _name: &'static str,
            len: usize,
            visitor: V,
        ) -> Result<V::Value, Error> {
            visitor.visit_seq(Elements {
                probe: self,
                left: len,
            })
        }
        fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            let count = usize::from(self.depth < MAX_DEPTH);
            visitor.visit_map(Entries {
                probe: self,
                keys: Vec::new(),
                left: count,
            })
        }
        fn deserialize_struct<V: Visitor<'de>>(
            self,
            _name: &'static str,
            fields: &'static [&'static str],
            visitor: V,
        ) -> Result<V::Value, Error> {
            self.names
                .extend(fields.iter().map(|field| field.to_string()));
            let keys = if self.depth < MAX_DEPTH {
                fields.to_vec()
            } else {
                Vec::new()
            };
            visitor.visit_map(Entries {
                left: keys.len(),
                probe: self,
                keys,
            })
        }
        fn deserialize_enum<V: Visitor<'de>>(
            self,
            _name: &'static str,
            variants: &'static [&'static str],
            visitor: V,
        ) -> Result<V::Value, Error> {
            visitor.visit_enum(Variant {
                probe: self,
                name: variants.first().copied().unwrap_or(""),
            })
        }
        fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            visitor.visit_str("")
        }
        fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            visitor.visit_unit()
        }
    }

    struct Elements<'a> {
        probe: Probe<'a>,
        left: usize,
    }

    impl<'de> de::SeqAccess<'de> for Elements<'_> {
        type Error = Error;
        fn next_element_seed<T: DeserializeSeed<'de>>(
            &mut self,
            seed: T,
        ) -> Result<Option<T::Value>, Error> {
            if self.left == 0 {
                return Ok(None);
            }
            self.left -= 1;
            seed.deserialize(self.probe.deeper()).map(Some)
        }
    }

    struct Entries<'a> {
        probe: Probe<'a>,
        /// Struct field names to yield as keys; empty for a map.
        keys: Vec<&'static str>,
        left: usize,
    }

    impl<'de> de::MapAccess<'de> for Entries<'_> {
        type Error = Error;
        fn next_key_seed<K: DeserializeSeed<'de>>(
            &mut self,
            seed: K,
        ) -> Result<Option<K::Value>, Error> {
            if self.left == 0 {
                return Ok(None);
            }
            self.left -= 1;
            if self.keys.is_empty() {
                seed.deserialize(self.probe.deeper()).map(Some)
            } else {
                let key = self.keys[self.keys.len() - 1 - self.left];
                seed.deserialize(key.into_deserializer()).map(Some)
            }
        }
        fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Error> {
            seed.deserialize(self.probe.deeper())
        }
    }

    struct Variant<'a> {
        probe: Probe<'a>,
        name: &'static str,
    }

    impl<'de, 'a> de::EnumAccess<'de> for Variant<'a> {
        type Error = Error;
        type Variant = Probe<'a>;
        fn variant_seed<V: DeserializeSeed<'de>>(
            self,
            seed: V,
        ) -> Result<(V::Value, Probe<'a>), Error> {
            let value = seed.deserialize(self.name.into_deserializer())?;
            Ok((value, self.probe))
        }
    }

    impl<'de> de::VariantAccess<'de> for Probe<'_> {
        type Error = Error;
        fn unit_variant(self) -> Result<(), Error> {
            Ok(())
        }
        fn newtype_variant_seed<T: DeserializeSeed<'de>>(
            mut self,
            seed: T,
        ) -> Result<T::Value, Error> {
            seed.deserialize(self.deeper())
        }
        fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, Error> {
            visitor.visit_seq(Elements {
                probe: self,
                left: len,
            })
        }
        fn struct_variant<V: Visitor<'de>>(
            self,
            fields: &'static [&'static str],
            visitor: V,
        ) -> Result<V::Value, Error> {
            de::Deserializer::deserialize_struct(self, "", fields, visitor)
        }
    }
}
