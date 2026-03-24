use serde::Serialize;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct CapturedCommandOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub fn apply_windows_no_window(cmd: &mut Command) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // WinAPI constant: CREATE_NO_WINDOW
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = cmd;
    }
}

#[cfg(target_os = "windows")]
fn kill_process_tree_best_effort(pid: u32) {
    // On Windows, killing only the parent process is often not enough because common shims
    // (e.g. npm.cmd) spawn child processes (cmd.exe -> node.exe). Use taskkill /T to kill the tree.
    let mut cmd = Command::new("taskkill");
    apply_windows_no_window(&mut cmd);
    let _ = cmd
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(unix)]
fn configure_cancelable_process_group(cmd: &mut Command) {
    cmd.process_group(0);
}

#[cfg(not(unix))]
fn configure_cancelable_process_group(cmd: &mut Command) {
    let _ = cmd;
}

#[cfg(unix)]
fn kill_process_tree_best_effort_unix(pid: u32) {
    let group = format!("-{pid}");
    for signal in ["-TERM", "-KILL"] {
        let mut cmd = Command::new("kill");
        let _ = cmd
            .args([signal, &group])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if signal == "-TERM" {
            std::thread::sleep(std::time::Duration::from_millis(180));
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedOpenclaw {
    pub command: PathBuf,
    pub source: &'static str,
    pub path_env: String,
}

#[cfg(target_os = "windows")]
fn resolve_windows_openclaw_node_launch(
    resolved: &ResolvedOpenclaw,
) -> Option<(PathBuf, PathBuf)> {
    let extension = resolved
        .command
        .extension()
        .and_then(|value| value.to_str())?
        .to_ascii_lowercase();
    if !matches!(extension.as_str(), "cmd" | "bat") {
        return None;
    }

    let bin_dir = resolved.command.parent()?;
    let script = [
        bin_dir.join("node_modules").join("openclaw").join("openclaw.mjs"),
        bin_dir.join("node_modules").join("OpenClaw").join("openclaw.mjs"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())?;

    let node = resolve_command_in_path("node", &resolved.path_env)
        .or_else(|| {
            let candidate = bin_dir.join("node.exe");
            candidate.is_file().then_some(candidate)
        })?;

    Some((node, script))
}

pub fn create_openclaw_command(resolved: &ResolvedOpenclaw) -> Command {
    #[cfg(target_os = "windows")]
    {
        if let Some((node, script)) = resolve_windows_openclaw_node_launch(resolved) {
            let mut cmd = Command::new(node);
            apply_windows_no_window(&mut cmd);
            cmd.env("PATH", &resolved.path_env);
            cmd.arg(script);
            return cmd;
        }
    }

    let mut cmd = Command::new(&resolved.command);
    apply_windows_no_window(&mut cmd);
    cmd.env("PATH", &resolved.path_env);
    cmd
}

pub fn home_dir() -> Option<PathBuf> {
    if let Ok(home) = env::var("HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    dirs::home_dir()
}

#[derive(Debug, Serialize, Clone)]
pub struct OpenclawInfo {
    pub installed: bool,
    pub version: Option<String>,
    pub help: Option<String>,
    pub executable: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct GatewayStatus {
    pub state: String,
    #[serde(rename = "dashboardUrl")]
    pub dashboard_url: Option<String>,
    pub raw: String,
}

fn split_lines(text: &str) -> Vec<String> {
    text.replace("\r\n", "\n")
        .split('\n')
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

fn path_separator() -> char {
    if cfg!(target_os = "windows") {
        ';'
    } else {
        ':'
    }
}

#[cfg(target_os = "windows")]
fn expand_windows_env_vars(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut out = String::with_capacity(value.len());
    let mut index = 0usize;

    while index < chars.len() {
        if chars[index] == '%' {
            if let Some(end_rel) = chars[index + 1..].iter().position(|c| *c == '%') {
                let end = index + 1 + end_rel;
                let name: String = chars[index + 1..end].iter().collect();
                if !name.trim().is_empty() {
                    if let Ok(expanded) = env::var(name.trim()) {
                        out.push_str(expanded.trim());
                        index = end + 1;
                        continue;
                    }
                }
            }
        }

        out.push(chars[index]);
        index += 1;
    }

    out
}

#[cfg(target_os = "windows")]
fn parse_reg_query_value(output: &str) -> Option<String> {
    for line in split_lines(output) {
        let Some(type_index) = line.find("REG_") else {
            continue;
        };
        let type_and_value = &line[type_index..];
        let mut parts = type_and_value.splitn(2, char::is_whitespace);
        let _value_type = parts.next()?;
        let value = parts.next()?.trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn get_windows_registry_path_value(hive: &str) -> Option<String> {
    let mut cmd = Command::new("reg");
    apply_windows_no_window(&mut cmd);
    let output = cmd.args(["query", hive, "/v", "Path"]).output().ok()?;
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    parse_reg_query_value(&combined)
}

fn merge_path_entries(
    existing_path: Option<&str>,
    extra_entries: &[&str],
    extra_first: bool,
) -> String {
    let existing = existing_path.unwrap_or("");
    let separator = path_separator();
    let mut seen = HashSet::<String>::new();
    let mut entries: Vec<String> = Vec::new();

    let mut push = |value: &str| {
        #[cfg(target_os = "windows")]
        let normalized = expand_windows_env_vars(value);
        #[cfg(not(target_os = "windows"))]
        let normalized = value.to_string();

        let trimmed = normalized.trim();
        if trimmed.is_empty() {
            return;
        }
        #[cfg(target_os = "windows")]
        let key = trimmed.to_ascii_lowercase();
        #[cfg(not(target_os = "windows"))]
        let key = trimmed.to_string();

        if seen.contains(&key) {
            return;
        }
        seen.insert(key.clone());
        entries.push(trimmed.to_string());
    };

    if extra_first {
        for e in extra_entries {
            for part in e.split(separator) {
                push(part);
            }
        }
        for e in existing.split(separator) {
            push(e);
        }
    } else {
        for e in existing.split(separator) {
            push(e);
        }
        for e in extra_entries {
            for part in e.split(separator) {
                push(part);
            }
        }
    }

    entries.join(&separator.to_string())
}

pub fn create_base_path_env() -> String {
    let existing = env::var("PATH").ok();

    #[cfg(target_os = "windows")]
    {
        let mut extra: Vec<String> = Vec::new();

        if let Some(user_path) = get_windows_registry_path_value("HKCU\\Environment") {
            extra.push(user_path);
        }
        if let Some(machine_path) = get_windows_registry_path_value(
            "HKLM\\SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment",
        ) {
            extra.push(machine_path);
        }

        if let Some(home) = home_dir() {
            extra.push(
                home.join("AppData")
                    .join("Roaming")
                    .join("npm")
                    .to_string_lossy()
                    .to_string(),
            );
            extra.push(
                home.join("AppData")
                    .join("Local")
                    .join("nvm")
                    .to_string_lossy()
                    .to_string(),
            );
        }

        if let Ok(nvm_home) = env::var("NVM_HOME") {
            extra.push(nvm_home);
        }
        if let Ok(nvm_symlink) = env::var("NVM_SYMLINK") {
            extra.push(nvm_symlink);
        }

        extra.push("C:\\Program Files\\Git\\cmd".into());
        extra.push("C:\\Program Files\\nodejs".into());
        extra.push("C:\\nvm4w\\nodejs".into());

        let extra_refs: Vec<&str> = extra.iter().map(|s| s.as_str()).collect();
        return merge_path_entries(existing.as_deref(), &extra_refs, false);
    }

    #[cfg(not(target_os = "windows"))]
    {
        let local_bin =
            home_dir().map(|h| h.join(".local").join("bin").to_string_lossy().to_string());

        let mut extra: Vec<String> = vec![
            "/opt/homebrew/bin".into(),
            "/usr/local/bin".into(),
            "/usr/bin".into(),
            "/bin".into(),
            "/usr/sbin".into(),
            "/sbin".into(),
        ];
        if let Some(local) = local_bin {
            extra.insert(2, local);
        }
        let extra_refs: Vec<&str> = extra.iter().map(|s| s.as_str()).collect();
        merge_path_entries(existing.as_deref(), &extra_refs, true)
    }
}

fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(path) {
            return meta.permissions().mode() & 0o111 != 0;
        }
    }
    true
}

#[cfg(target_os = "windows")]
fn windows_command_candidates(command: &str) -> Vec<String> {
    if Path::new(command).extension().is_some() {
        return vec![command.to_string()];
    }

    vec![
        format!("{command}.cmd"),
        format!("{command}.exe"),
        format!("{command}.bat"),
        format!("{command}.com"),
        command.to_string(),
    ]
}

pub fn resolve_command_in_path(command: &str, path_env: &str) -> Option<PathBuf> {
    let separator = path_separator();
    for dir in path_env.split(separator) {
        if dir.trim().is_empty() {
            continue;
        }

        #[cfg(target_os = "windows")]
        {
            for candidate_name in windows_command_candidates(command) {
                let candidate = Path::new(dir).join(&candidate_name);
                if is_executable(&candidate) {
                    return Some(candidate);
                }
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            let candidate = Path::new(dir).join(command);
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn which_in_path(command: &str, path_env: &str) -> Option<PathBuf> {
    resolve_command_in_path(command, path_env)
}

fn parse_semver_tuple(version: &str) -> Option<(u32, u32, u32)> {
    let trimmed = version.trim_start_matches('v');
    let mut parts = trimmed.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    let patch = parts.next()?.parse::<u32>().ok()?;
    Some((major, minor, patch))
}

fn compare_semver(a: (u32, u32, u32), b: (u32, u32, u32)) -> Ordering {
    a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2))
}

fn run_collect(mut cmd: Command) -> Result<(i32, String, String), String> {
    apply_windows_no_window(&mut cmd);
    let out = cmd.output().map_err(|e| e.to_string())?;
    let code = out.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    Ok((code, stdout, stderr))
}

pub fn resolve_openclaw() -> Option<ResolvedOpenclaw> {
    let path_env = create_base_path_env();

    // 1) PATH
    if let Some(found) = which_in_path("openclaw", &path_env) {
        return Some(ResolvedOpenclaw {
            command: found,
            source: "path",
            path_env,
        });
    }

    // 2) brew common paths (macOS)
    #[cfg(target_os = "macos")]
    {
        let brew_candidates = ["/opt/homebrew/bin/openclaw", "/usr/local/bin/openclaw"];
        for candidate in brew_candidates {
            let p = PathBuf::from(candidate);
            if is_executable(&p) {
                return Some(ResolvedOpenclaw {
                    command: p,
                    source: "brew_bin",
                    path_env,
                });
            }
        }
    }

    // 3) npm prefix -g
    {
        let npm_program =
            resolve_command_in_path("npm", &path_env).unwrap_or_else(|| PathBuf::from("npm"));
        let mut cmd = Command::new(&npm_program);
        cmd.env("PATH", &path_env);
        cmd.args(["prefix", "-g"]);
        if let Ok((_code, stdout, stderr)) = run_collect(cmd) {
            let combined = format!("{stdout}\n{stderr}");
            let prefix = split_lines(&combined).into_iter().next();
            if let Some(prefix) = prefix {
                #[cfg(target_os = "windows")]
                {
                    let candidates = [
                        Path::new(&prefix).join("openclaw.cmd"),
                        Path::new(&prefix).join("openclaw"),
                    ];
                    for candidate in candidates {
                        if is_executable(&candidate) {
                            return Some(ResolvedOpenclaw {
                                command: candidate,
                                source: "npm_prefix",
                                path_env,
                            });
                        }
                    }
                }

                #[cfg(not(target_os = "windows"))]
                {
                    let candidate = Path::new(&prefix).join("bin").join("openclaw");
                    if is_executable(&candidate) {
                        return Some(ResolvedOpenclaw {
                            command: candidate,
                            source: "npm_prefix",
                            path_env,
                        });
                    }
                }
            }
        }
    }

    // 4) macOS backward-compat: scan ~/.nvm/versions/node/*/bin/openclaw
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = home_dir() {
            let nvm_versions = home.join(".nvm").join("versions").join("node");
            if let Ok(entries) = fs::read_dir(&nvm_versions) {
                let mut best: Option<((u32, u32, u32), PathBuf)> = None;
                for entry in entries.flatten() {
                    let file_name = entry.file_name();
                    let name = file_name.to_string_lossy();
                    if !name.starts_with('v') {
                        continue;
                    }
                    let Some(ver) = parse_semver_tuple(&name) else {
                        continue;
                    };
                    let candidate = entry.path().join("bin").join("openclaw");
                    if !is_executable(&candidate) {
                        continue;
                    }
                    match &best {
                        None => best = Some((ver, candidate)),
                        Some((best_ver, _)) => {
                            if compare_semver(ver, *best_ver) == Ordering::Greater {
                                best = Some((ver, candidate));
                            }
                        }
                    }
                }
                if let Some((_ver, path)) = best {
                    return Some(ResolvedOpenclaw {
                        command: path,
                        source: "nvm_scan",
                        path_env,
                    });
                }
            }
        }
    }

    None
}

fn parse_version_from_output(text: &str) -> Option<String> {
    for line in split_lines(text) {
        if line.is_empty() {
            continue;
        }
        return Some(line);
    }
    None
}

pub fn get_openclaw_info(with_help: bool) -> OpenclawInfo {
    let resolved = resolve_openclaw();
    let path_env = resolved
        .as_ref()
        .map(|r| r.path_env.clone())
        .unwrap_or_else(create_base_path_env);
    let mut version_cmd = resolved
        .as_ref()
        .map(create_openclaw_command)
        .unwrap_or_else(|| {
            let mut cmd = Command::new("openclaw");
            cmd.env("PATH", &path_env);
            cmd
        });
    version_cmd.arg("--version");

    match run_collect(version_cmd) {
        Ok((_code, stdout, stderr)) => {
            let combined = format!("{stdout}\n{stderr}");
            let version = parse_version_from_output(&combined).or(Some("unknown".into()));
            let help = if with_help {
                let mut help_cmd = resolved
                    .as_ref()
                    .map(create_openclaw_command)
                    .unwrap_or_else(|| {
                        let mut cmd = Command::new("openclaw");
                        cmd.env("PATH", &path_env);
                        cmd
                    });
                help_cmd.arg("--help");
                run_collect(help_cmd).ok().map(|(_, out, err)| {
                    let combined = format!("{out}\n{err}");
                    if combined.len() > 200_000 {
                        format!("{}\n…", &combined[..200_000])
                    } else {
                        combined.trim().to_string()
                    }
                })
            } else {
                None
            };

            OpenclawInfo {
                installed: true,
                version,
                help,
                executable: resolved
                    .as_ref()
                    .map(|r| r.command.to_string_lossy().to_string()),
                error: None,
            }
        }
        Err(err) => OpenclawInfo {
            installed: false,
            version: None,
            help: None,
            executable: resolved
                .as_ref()
                .map(|r| r.command.to_string_lossy().to_string()),
            error: Some(err),
        },
    }
}

fn extract_dashboard_url(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.to_lowercase().starts_with("dashboard:") {
            let rest = trimmed.splitn(2, ':').nth(1)?.trim();
            if rest.starts_with("http://") || rest.starts_with("https://") {
                return Some(rest.to_string());
            }
        }
    }
    None
}

pub fn parse_gateway_status(text: &str) -> GatewayStatus {
    let raw = text.trim().to_string();
    let lower = raw.to_lowercase();
    let dashboard_url = extract_dashboard_url(&raw);

    // Some platforms (notably Windows) may report "service missing" while the gateway is still
    // reachable via its RPC/WS probe. Treat a successful probe/listening line as "running".
    let has_rpc_ok = lower.contains("rpc probe: ok") || lower.contains("rpc probe ok");
    let has_listening = lower.contains("listening:") || lower.contains("listening on ");

    let has_running = lower.contains("active: active")
        || lower.contains("(running)")
        || (lower.contains("running") && !lower.contains("not running"))
        || has_rpc_ok
        || has_listening;

    let has_stopped = lower.contains("not running")
        || lower.contains("inactive")
        || lower.contains("dead")
        || lower.contains("stopped")
        || lower.contains("exited")
        || lower.contains("failed");

    let has_loaded = raw.to_lowercase().contains("loaded: loaded")
        || (lower.contains("(loaded)") && !lower.contains("(not loaded)"));

    let has_not_installed = lower.contains("service not installed")
      || lower.contains("service unit not found")
      || (lower.contains("unit") && lower.contains("not found"))
      || lower.contains("loaded: not-found")
      || lower.contains("not-found")
      // Windows Scheduled Task based service
      || lower.contains("scheduled task (missing)")
      || (lower.contains("scheduled task") && lower.contains("missing"))
      || lower.contains("gateway service missing")
      || (lower.contains("service") && lower.contains("missing") && lower.contains("gateway"));

    let state = if has_running {
        "running"
    } else if has_not_installed && !has_loaded {
        "not_installed"
    } else if has_stopped {
        "stopped"
    } else if has_not_installed {
        "stopped"
    } else {
        "unknown"
    };

    GatewayStatus {
        state: state.into(),
        dashboard_url,
        raw,
    }
}

pub fn cleanup_mac_nvm_openclaw(
    openclaw_path: &Path,
    openclaw_package: &str,
) -> Result<Vec<String>, String> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = openclaw_path;
        let _ = openclaw_package;
        return Ok(vec![]);
    }

    #[cfg(target_os = "macos")]
    {
        let openclaw_str = openclaw_path.to_string_lossy().to_string();
        if !openclaw_str.contains("/.nvm/versions/node/") {
            return Ok(vec![]);
        }

        let bin_dir = openclaw_path.parent().ok_or("invalid openclaw path")?;
        if bin_dir.file_name() != Some(OsStr::new("bin")) {
            return Ok(vec![]);
        }
        let node_root = bin_dir.parent().ok_or("invalid openclaw path")?;

        let mut removed: Vec<String> = Vec::new();
        let targets = [
            (node_root.join("bin").join("openclaw"), "bin"),
            (
                node_root
                    .join("lib")
                    .join("node_modules")
                    .join(openclaw_package),
                "module",
            ),
            (
                node_root.join("lib").join("node_modules").join("openclaw"),
                "module",
            ),
        ];

        for (target, label) in targets {
            if !target.starts_with(node_root) {
                continue;
            }
            if target.exists() {
                if target.is_dir() {
                    fs::remove_dir_all(&target).map_err(|e| e.to_string())?;
                } else {
                    fs::remove_file(&target).map_err(|e| e.to_string())?;
                }
                removed.push(format!("{label}: {}", target.to_string_lossy()));
            }
        }

        Ok(removed)
    }
}

#[cfg(target_os = "macos")]
pub fn cleanup_all_mac_nvm_openclaw(openclaw_package: &str) -> Result<Vec<String>, String> {
    let Some(home) = home_dir() else {
        return Ok(vec![]);
    };
    let nvm_versions = home.join(".nvm").join("versions").join("node");
    let entries = match fs::read_dir(&nvm_versions) {
        Ok(e) => e,
        Err(_) => return Ok(vec![]),
    };

    let mut removed: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let bin_openclaw = path.join("bin").join("openclaw");
        let module_dir = path.join("lib").join("node_modules").join(openclaw_package);
        let module_dir_fallback = path.join("lib").join("node_modules").join("openclaw");

        for (target, label) in [
            (bin_openclaw, "bin"),
            (module_dir, "module"),
            (module_dir_fallback, "module"),
        ] {
            if !target.exists() {
                continue;
            }
            if target.is_dir() {
                fs::remove_dir_all(&target).map_err(|e| e.to_string())?;
            } else {
                fs::remove_file(&target).map_err(|e| e.to_string())?;
            }
            removed.push(format!("{label}: {}", target.to_string_lossy()));
        }
    }

    Ok(removed)
}

#[cfg(not(target_os = "macos"))]
pub fn cleanup_all_mac_nvm_openclaw(_openclaw_package: &str) -> Result<Vec<String>, String> {
    Ok(vec![])
}

#[allow(dead_code)]
pub fn spawn_with_streaming_logs(
    mut cmd: Command,
    mut on_line: impl FnMut(String) + Send + 'static,
) -> Result<i32, String> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    apply_windows_no_window(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().ok_or("stdout unavailable")?;
    let stderr = child.stderr.take().ok_or("stderr unavailable")?;

    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let tx_out = tx.clone();
    let tx_err = tx.clone();

    let forward_stream = |mut input: Box<dyn Read + Send>,
                          sender: std::sync::mpsc::Sender<String>,
                          prefix: &'static str| {
        let mut buf = [0u8; 4096];
        let mut current: Vec<u8> = Vec::new();
        loop {
            let read = match input.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => break,
            };
            for &b in &buf[..read] {
                if b == b'\n' || b == b'\r' {
                    if !current.is_empty() {
                        let line = String::from_utf8_lossy(&current).to_string();
                        let _ = sender.send(if prefix.is_empty() {
                            line
                        } else {
                            format!("{prefix}{line}")
                        });
                        current.clear();
                    }
                    continue;
                }
                current.push(b);
                if current.len() > 16_384 {
                    let line = String::from_utf8_lossy(&current).to_string();
                    let _ = sender.send(if prefix.is_empty() {
                        line
                    } else {
                        format!("{prefix}{line}")
                    });
                    current.clear();
                }
            }
        }
        if !current.is_empty() {
            let line = String::from_utf8_lossy(&current).to_string();
            let _ = sender.send(if prefix.is_empty() {
                line
            } else {
                format!("{prefix}{line}")
            });
        }
    };

    let out_thread = std::thread::spawn(move || {
        forward_stream(Box::new(stdout), tx_out, "");
    });

    let err_thread = std::thread::spawn(move || {
        forward_stream(Box::new(stderr), tx_err, "[stderr] ");
    });

    drop(tx);
    let forward_thread = std::thread::spawn(move || {
        while let Ok(line) = rx.recv() {
            on_line(line);
        }
    });

    let status = child.wait().map_err(|e| e.to_string())?;
    let _ = out_thread.join();
    let _ = err_thread.join();
    let _ = forward_thread.join();
    Ok(status.code().unwrap_or(-1))
}

pub fn spawn_with_streaming_logs_cancelable(
    mut cmd: Command,
    cancel: Arc<AtomicBool>,
    mut on_line: impl FnMut(String) + Send + 'static,
) -> Result<i32, String> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    apply_windows_no_window(&mut cmd);
    configure_cancelable_process_group(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().ok_or("stdout unavailable")?;
    let stderr = child.stderr.take().ok_or("stderr unavailable")?;

    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let tx_out = tx.clone();
    let tx_err = tx.clone();

    let forward_stream = |mut input: Box<dyn Read + Send>,
                          sender: std::sync::mpsc::Sender<String>,
                          prefix: &'static str| {
        let mut buf = [0u8; 4096];
        let mut current: Vec<u8> = Vec::new();
        loop {
            let read = match input.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => break,
            };
            for &b in &buf[..read] {
                if b == b'\n' || b == b'\r' {
                    if !current.is_empty() {
                        let line = String::from_utf8_lossy(&current).to_string();
                        let _ = sender.send(if prefix.is_empty() {
                            line
                        } else {
                            format!("{prefix}{line}")
                        });
                        current.clear();
                    }
                    continue;
                }
                current.push(b);
                if current.len() > 16_384 {
                    let line = String::from_utf8_lossy(&current).to_string();
                    let _ = sender.send(if prefix.is_empty() {
                        line
                    } else {
                        format!("{prefix}{line}")
                    });
                    current.clear();
                }
            }
        }
        if !current.is_empty() {
            let line = String::from_utf8_lossy(&current).to_string();
            let _ = sender.send(if prefix.is_empty() {
                line
            } else {
                format!("{prefix}{line}")
            });
        }
    };

    let out_thread = std::thread::spawn(move || {
        forward_stream(Box::new(stdout), tx_out, "");
    });

    let err_thread = std::thread::spawn(move || {
        forward_stream(Box::new(stderr), tx_err, "[stderr] ");
    });

    drop(tx);

    let forward_cancel = cancel.clone();
    let forward_thread = std::thread::spawn(move || {
        while let Ok(line) = rx.recv() {
            if forward_cancel.load(AtomicOrdering::SeqCst) {
                continue;
            }
            on_line(line);
        }
    });

    loop {
        if cancel.load(AtomicOrdering::SeqCst) {
            #[cfg(target_os = "windows")]
            {
                kill_process_tree_best_effort(child.id());
            }
            #[cfg(not(target_os = "windows"))]
            {
                kill_process_tree_best_effort_unix(child.id());
                let _ = child.kill();
            }
            let _ = child.wait();
            let _ = out_thread.join();
            let _ = err_thread.join();
            let _ = forward_thread.join();
            return Err("用户取消".into());
        }

        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => {
                let _ = out_thread.join();
                let _ = err_thread.join();
                let _ = forward_thread.join();
                return Ok(status.code().unwrap_or(-1));
            }
            None => std::thread::sleep(std::time::Duration::from_millis(120)),
        }
    }
}

pub fn capture_command_output_cancelable(
    mut cmd: Command,
    cancel: Arc<AtomicBool>,
) -> Result<CapturedCommandOutput, String> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    apply_windows_no_window(&mut cmd);
    configure_cancelable_process_group(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().ok_or("stdout unavailable")?;
    let stderr = child.stderr.take().ok_or("stderr unavailable")?;

    let out_thread = std::thread::spawn(move || {
        let mut reader = stdout;
        let mut buf = Vec::new();
        let _ = reader.read_to_end(&mut buf);
        buf
    });

    let err_thread = std::thread::spawn(move || {
        let mut reader = stderr;
        let mut buf = Vec::new();
        let _ = reader.read_to_end(&mut buf);
        buf
    });

    loop {
        if cancel.load(AtomicOrdering::SeqCst) {
            #[cfg(target_os = "windows")]
            {
                kill_process_tree_best_effort(child.id());
            }
            #[cfg(not(target_os = "windows"))]
            {
                kill_process_tree_best_effort_unix(child.id());
                let _ = child.kill();
            }
            let _ = child.wait();
            let _ = out_thread.join();
            let _ = err_thread.join();
            return Err("用户取消".into());
        }

        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => {
                let stdout = out_thread.join().unwrap_or_default();
                let stderr = err_thread.join().unwrap_or_default();
                return Ok(CapturedCommandOutput {
                    code: status.code().unwrap_or(-1),
                    stdout: String::from_utf8_lossy(&stdout).to_string(),
                    stderr: String::from_utf8_lossy(&stderr).to_string(),
                });
            }
            None => std::thread::sleep(std::time::Duration::from_millis(120)),
        }
    }
}
