use serde::Serialize;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct ResolvedOpenclaw {
  pub command: PathBuf,
  pub source: &'static str,
  pub path_env: String,
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
  text
    .replace("\r\n", "\n")
    .split('\n')
    .map(|l| l.trim().to_string())
    .filter(|l| !l.is_empty())
    .collect()
}

fn merge_path_entries(existing_path: Option<&str>, extra_entries: &[&str], extra_first: bool) -> String {
  let existing = existing_path.unwrap_or("");
  let mut seen = HashSet::<String>::new();
  let mut entries: Vec<String> = Vec::new();

  let mut push = |value: &str| {
    let trimmed = value.trim();
    if trimmed.is_empty() {
      return;
    }
    let key = trimmed.to_string();
    if seen.contains(&key) {
      return;
    }
    seen.insert(key.clone());
    entries.push(key);
  };

  if extra_first {
    for e in extra_entries {
      push(e);
    }
    for e in existing.split(':') {
      push(e);
    }
  } else {
    for e in existing.split(':') {
      push(e);
    }
    for e in extra_entries {
      push(e);
    }
  }

  entries.join(":")
}

pub fn create_base_path_env() -> String {
  let existing = env::var("PATH").ok();
  let home = env::var("HOME").unwrap_or_default();
  let local_bin = if home.is_empty() {
    None
  } else {
    Some(format!("{home}/.local/bin"))
  };

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

fn which_in_path(command: &str, path_env: &str) -> Option<PathBuf> {
  for dir in path_env.split(':') {
    if dir.trim().is_empty() {
      continue;
    }
    let candidate = Path::new(dir).join(command);
    if is_executable(&candidate) {
      return Some(candidate);
    }
  }
  None
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
    return Some(ResolvedOpenclaw { command: found, source: "path", path_env });
  }

  // 2) brew common paths (macOS)
  #[cfg(target_os = "macos")]
  {
    let brew_candidates = ["/opt/homebrew/bin/openclaw", "/usr/local/bin/openclaw"];
    for candidate in brew_candidates {
      let p = PathBuf::from(candidate);
      if is_executable(&p) {
        return Some(ResolvedOpenclaw { command: p, source: "brew_bin", path_env });
      }
    }
  }

  // 3) npm prefix -g
  {
    let mut cmd = Command::new("npm");
    cmd.env("PATH", &path_env);
    cmd.args(["prefix", "-g"]);
    if let Ok((_code, stdout, stderr)) = run_collect(cmd) {
      let combined = format!("{stdout}\n{stderr}");
      let prefix = split_lines(&combined).into_iter().next();
      if let Some(prefix) = prefix {
        let candidate = Path::new(&prefix).join("bin").join("openclaw");
        if is_executable(&candidate) {
          return Some(ResolvedOpenclaw { command: candidate, source: "npm_prefix", path_env });
        }
      }
    }
  }

  // 4) macOS backward-compat: scan ~/.nvm/versions/node/*/bin/openclaw
  #[cfg(target_os = "macos")]
  {
    let home = env::var("HOME").unwrap_or_default();
    if !home.is_empty() {
      let nvm_versions = Path::new(&home).join(".nvm").join("versions").join("node");
      if let Ok(entries) = fs::read_dir(&nvm_versions) {
        let mut best: Option<((u32, u32, u32), PathBuf)> = None;
        for entry in entries.flatten() {
          let file_name = entry.file_name();
          let name = file_name.to_string_lossy();
          if !name.starts_with('v') {
            continue;
          }
          let Some(ver) = parse_semver_tuple(&name) else { continue };
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
          return Some(ResolvedOpenclaw { command: path, source: "nvm_scan", path_env });
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
  let path_env = resolved.as_ref().map(|r| r.path_env.clone()).unwrap_or_else(create_base_path_env);
  let command = resolved
    .as_ref()
    .map(|r| r.command.clone())
    .unwrap_or_else(|| PathBuf::from("openclaw"));

  let mut version_cmd = Command::new(&command);
  version_cmd.env("PATH", &path_env);
  version_cmd.arg("--version");

  match run_collect(version_cmd) {
    Ok((_code, stdout, stderr)) => {
      let combined = format!("{stdout}\n{stderr}");
      let version = parse_version_from_output(&combined).or(Some("unknown".into()));
      let help = if with_help {
        let mut help_cmd = Command::new(&command);
        help_cmd.env("PATH", &path_env);
        help_cmd.arg("--help");
        run_collect(help_cmd)
          .ok()
          .map(|(_, out, err)| {
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
        executable: resolved.as_ref().map(|r| r.command.to_string_lossy().to_string()),
        error: None,
      }
    }
    Err(err) => OpenclawInfo {
      installed: false,
      version: None,
      help: None,
      executable: resolved.as_ref().map(|r| r.command.to_string_lossy().to_string()),
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

  let has_running =
    raw.to_lowercase().contains("active: active")
      || lower.contains("(running)")
      || (lower.contains("running") && !lower.contains("not running"));

  let has_stopped =
    lower.contains("not running")
      || lower.contains("inactive")
      || lower.contains("dead")
      || lower.contains("stopped")
      || lower.contains("exited")
      || lower.contains("failed");

  let has_loaded = raw.to_lowercase().contains("loaded: loaded") || (lower.contains("(loaded)") && !lower.contains("(not loaded)"));

  let has_not_installed =
    lower.contains("service not installed")
      || lower.contains("service unit not found")
      || (lower.contains("unit") && lower.contains("not found"))
      || lower.contains("loaded: not-found")
      || lower.contains("not-found");

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

pub fn cleanup_mac_nvm_openclaw(openclaw_path: &Path, openclaw_package: &str) -> Result<Vec<String>, String> {
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
      (node_root.join("lib").join("node_modules").join(openclaw_package), "module"),
      (node_root.join("lib").join("node_modules").join("openclaw"), "module"),
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
  let home = env::var("HOME").unwrap_or_default();
  if home.is_empty() {
    return Ok(vec![]);
  }
  let nvm_versions = Path::new(&home).join(".nvm").join("versions").join("node");
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
  let mut child = cmd.spawn().map_err(|e| e.to_string())?;
  let stdout = child.stdout.take().ok_or("stdout unavailable")?;
  let stderr = child.stderr.take().ok_or("stderr unavailable")?;

  let (tx, rx) = std::sync::mpsc::channel::<String>();
  let tx_out = tx.clone();
  let tx_err = tx.clone();

  let out_thread = std::thread::spawn(move || {
    let reader = std::io::BufReader::new(stdout);
    for line in reader.lines().flatten() {
      let _ = tx_out.send(line);
    }
  });

  let err_thread = std::thread::spawn(move || {
    let reader = std::io::BufReader::new(stderr);
    for line in reader.lines().flatten() {
      let _ = tx_err.send(format!("[stderr] {line}"));
    }
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
  let mut child = cmd.spawn().map_err(|e| e.to_string())?;
  let stdout = child.stdout.take().ok_or("stdout unavailable")?;
  let stderr = child.stderr.take().ok_or("stderr unavailable")?;

  let (tx, rx) = std::sync::mpsc::channel::<String>();
  let tx_out = tx.clone();
  let tx_err = tx.clone();

  let out_thread = std::thread::spawn(move || {
    let reader = std::io::BufReader::new(stdout);
    for line in reader.lines().flatten() {
      let _ = tx_out.send(line);
    }
  });

  let err_thread = std::thread::spawn(move || {
    let reader = std::io::BufReader::new(stderr);
    for line in reader.lines().flatten() {
      let _ = tx_err.send(format!("[stderr] {line}"));
    }
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
      let _ = child.kill();
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
