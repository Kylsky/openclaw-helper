use crate::openclaw::{
  cleanup_all_mac_nvm_openclaw, cleanup_mac_nvm_openclaw, create_base_path_env, get_openclaw_info, parse_gateway_status,
  resolve_command_in_path, resolve_openclaw, spawn_with_streaming_logs_cancelable, GatewayStatus, OpenclawInfo,
};
use serde::Deserialize;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Window};

fn now_ms() -> u64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_millis() as u64
}

#[derive(Debug, Serialize, Clone)]
struct LogPayload {
  message: String,
  ts: u64,
}

#[derive(Debug, Serialize, Clone)]
struct ProgressPayload {
  stage: String,
  title: String,
  index: u32,
  total: u32,
  percent: f32,
}

fn emit_log(window: &Window, event: &str, message: impl Into<String>) {
  let payload = LogPayload { message: message.into(), ts: now_ms() };
  let _ = window.emit(event, payload);
}

fn emit_progress(window: &Window, stage: &str, title: &str, index: u32, total: u32) {
  let total_f = total.max(1) as f32;
  let percent = ((index.saturating_sub(1)) as f32 / total_f).clamp(0.0, 1.0);
  let payload = ProgressPayload {
    stage: stage.into(),
    title: title.into(),
    index,
    total,
    percent,
  };
  let _ = window.emit("install-progress", payload);
}

fn check_canceled(flag: &Arc<AtomicBool>) -> Result<(), String> {
  if flag.load(Ordering::SeqCst) {
    return Err("用户取消".into());
  }
  Ok(())
}

#[derive(Default)]
pub struct TaskState {
  running: Mutex<bool>,
  cancel_flag: Mutex<Option<Arc<AtomicBool>>>,
}

impl TaskState {
  fn start(&self) -> Result<Arc<AtomicBool>, String> {
    let mut running = self.running.lock().map_err(|_| "内部错误：锁失败")?;
    if *running {
      return Err("已有任务正在运行，请先取消或等待完成。".into());
    }
    *running = true;
    let flag = Arc::new(AtomicBool::new(false));
    let mut slot = self.cancel_flag.lock().map_err(|_| "内部错误：锁失败")?;
    *slot = Some(flag.clone());
    Ok(flag)
  }

  fn finish(&self) {
    if let Ok(mut running) = self.running.lock() {
      *running = false;
    }
    if let Ok(mut slot) = self.cancel_flag.lock() {
      *slot = None;
    }
  }

  fn cancel(&self) {
    if let Ok(slot) = self.cancel_flag.lock() {
      if let Some(flag) = slot.as_ref() {
        flag.store(true, Ordering::SeqCst);
      }
    }
  }
}

#[tauri::command]
pub async fn check_openclaw() -> OpenclawInfo {
  get_openclaw_info(false)
}

#[tauri::command]
pub async fn get_gateway_status() -> GatewayStatus {
  let resolved = resolve_openclaw();
  let Some(resolved) = resolved else {
    return parse_gateway_status("未检测到 openclaw");
  };

  let mut cmd = Command::new(&resolved.command);
  cmd.env("PATH", &resolved.path_env);
  cmd.args(["--no-color", "gateway", "status"]);
  match cmd.output() {
    Ok(output) => {
      let stdout = String::from_utf8_lossy(&output.stdout);
      let stderr = String::from_utf8_lossy(&output.stderr);
      parse_gateway_status(&format!("{stdout}\n{stderr}"))
    }
    Err(err) => parse_gateway_status(&format!("gateway status failed: {err}")),
  }
}

#[tauri::command]
pub async fn run_openclaw(window: Window, state: tauri::State<'_, TaskState>, args: Vec<String>) -> Result<(), String> {
  if args.is_empty() {
    return Err("缺少 openclaw 参数".into());
  }
  let cancel = state.start()?;
  let resolved = resolve_openclaw().ok_or("未检测到 openclaw，请先完成安装。")?;

  let result = (|| -> Result<(), String> {
    emit_log(&window, "openclaw-log", format!("openclaw {} ({})", args.join(" "), resolved.source));

    let mut cmd = Command::new(&resolved.command);
    cmd.env("PATH", &resolved.path_env);
    cmd.arg("--no-color");
    for a in args {
      cmd.arg(a);
    }

    let w = window.clone();
    let cancel2 = cancel.clone();
    let code = spawn_with_streaming_logs_cancelable(cmd, cancel2, move |line| emit_log(&w, "openclaw-log", line))?;
    if code == 0 {
      Ok(())
    } else {
      Err(format!("openclaw 退出码：{code}"))
    }
  })();

  state.finish();
  result
}

#[tauri::command]
pub async fn open_dashboard(_app: AppHandle) -> Result<String, String> {
  let resolved = resolve_openclaw().ok_or("未检测到 openclaw，请先完成安装。")?;

  let mut cmd = Command::new(&resolved.command);
  cmd.env("PATH", &resolved.path_env);
  cmd.args(["dashboard", "--no-open"]);
  let output = cmd.output().map_err(|e| e.to_string())?;
  let combined = format!(
    "{}\n{}",
    String::from_utf8_lossy(&output.stdout),
    String::from_utf8_lossy(&output.stderr)
  );
  let url = combined
    .split_whitespace()
    .find(|t| t.starts_with("http://") || t.starts_with("https://"))
    .ok_or("未获取到控制台链接，请先启动网关或完成配置。")?
    .to_string();

  Ok(url)
}

#[tauri::command]
pub async fn open_wizard(_app: AppHandle) -> Result<(), String> {
  let resolved = resolve_openclaw().ok_or("未检测到 openclaw，请先完成安装。")?;
  let openclaw_cmd = resolved.command.to_string_lossy().to_string();

  #[cfg(target_os = "macos")]
  {
    let escape = |value: &str| value.replace('\\', "\\\\").replace('\"', "\\\"");
    let shell_cmd = if openclaw_cmd.contains(' ') {
      format!("\"{}\" onboard", openclaw_cmd)
    } else {
      format!("{openclaw_cmd} onboard")
    };
    let script = [
      "tell application \"Terminal\"",
      "activate",
      &format!("do script \"{}\"", escape(&shell_cmd)),
      "end tell",
    ]
    .join("\n");

    Command::new("/usr/bin/osascript")
      .arg("-e")
      .arg(script)
      .stdout(std::process::Stdio::null())
      .stderr(std::process::Stdio::null())
      .spawn()
      .map_err(|e| e.to_string())?;
    return Ok(());
  }

  #[cfg(not(target_os = "macos"))]
  {
    Err("当前平台暂不支持自动打开配置向导终端窗口。".into())
  }
}

#[tauri::command]
pub async fn open_external(app: AppHandle, url: String) -> Result<(), String> {
  let url = url.trim().to_string();
  if !(url.starts_with("http://") || url.starts_with("https://")) {
    return Err("只允许打开 http/https 链接".into());
  }

  #[allow(deprecated)]
  {
    use tauri_plugin_shell::ShellExt;
    app.shell().open(url, None).map_err(|e| e.to_string())?;
  }
  Ok(())
}

#[tauri::command]
pub async fn uninstall_openclaw(window: Window, state: tauri::State<'_, TaskState>) -> Result<(), String> {
  let cancel = state.start()?;
  let resolved = resolve_openclaw().ok_or("未检测到 openclaw，请先完成安装。")?;
  let openclaw_cmd = resolved.command.clone();

  let result = (|| -> Result<(), String> {
    emit_log(&window, "install-log", "[uninstall] start");
    emit_log(
      &window,
      "install-log",
      "openclaw uninstall --service --state --workspace --yes --non-interactive",
    );

    // 1) OpenClaw's own uninstaller (service/state/workspace).
    {
      let mut cmd = Command::new(&openclaw_cmd);
      cmd.env("PATH", &resolved.path_env);
      cmd.args([
        "uninstall",
        "--service",
        "--state",
        "--workspace",
        "--yes",
        "--non-interactive",
      ]);
      let w = window.clone();
      let cancel2 = cancel.clone();
      let code = spawn_with_streaming_logs_cancelable(cmd, cancel2, move |line| emit_log(&w, "install-log", line))?;
      if code != 0 {
        return Err(format!("openclaw uninstall 失败（退出码 {code}）"));
      }
    }

    // 2) Best-effort remove CLI from common managers.
    emit_log(&window, "install-log", "正在尝试移除 openclaw CLI（brew / npm / pnpm / nvm）…");

    #[cfg(target_os = "macos")]
    {
      let brew_candidates = ["/opt/homebrew/bin/brew", "/usr/local/bin/brew", "brew"];
      for brew in brew_candidates {
        check_canceled(&cancel)?;
        let mut cmd = Command::new(brew);
        cmd.env("PATH", &resolved.path_env);
        cmd.args(["uninstall", "openclaw"]);
        let _ = cmd.output();
      }
    }

    // npm uninstall -g openclaw (best-effort)
    {
      check_canceled(&cancel)?;
      let mut cmd = Command::new("npm");
      cmd.env("PATH", &resolved.path_env);
      cmd.args(["uninstall", "-g", "openclaw"]);
      let _ = cmd.output();
    }

    // pnpm remove -g openclaw (best-effort)
    {
      check_canceled(&cancel)?;
      let mut cmd = Command::new("pnpm");
      cmd.env("PATH", &resolved.path_env);
      cmd.args(["remove", "-g", "openclaw"]);
      let _ = cmd.output();
    }

    // nvm scan cleanup (delete ~/.nvm/... copy if that's where we resolved from)
    {
      check_canceled(&cancel)?;
      let mut removed = cleanup_mac_nvm_openclaw(&openclaw_cmd, "openclaw").unwrap_or_default();
      removed.extend(cleanup_all_mac_nvm_openclaw("openclaw").unwrap_or_default());
      if !removed.is_empty() {
        emit_log(&window, "install-log", format!("[cleanup] nvm: removed {} item(s)", removed.len()));
        for item in removed {
          emit_log(&window, "install-log", format!("[cleanup] nvm: {item}"));
        }
      }
    }

    emit_log(&window, "install-log", "卸载完成。");
    Ok(())
  })();

  state.finish();
  result
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct InstallOptions {
  #[serde(rename = "openclawPackage")]
  pub openclaw_package: Option<String>,
  #[serde(rename = "npmRegistry")]
  pub npm_registry: Option<String>,
  #[serde(rename = "customBaseUrl")]
  pub custom_base_url: Option<String>,
  #[serde(rename = "customModelId")]
  pub custom_model_id: Option<String>,
  #[serde(rename = "customApiKey")]
  pub custom_api_key: Option<String>,
}

fn validate_npm_package_name(value: &str) -> Result<String, String> {
  let name = value.trim();
  if name.is_empty() {
    return Err("openclaw 包名不能为空".into());
  }
  if name.len() > 214 {
    return Err("openclaw 包名太长".into());
  }
  if name.chars().any(|c| c.is_whitespace()) {
    return Err("openclaw 包名不能包含空格".into());
  }
  let ok = name
    .chars()
    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '/' | '.' | '_' | '-' ));
  if !ok || name.contains("..") || name.contains("//") || name.contains('\\') || name.contains('\'') || name.contains('\"') || name.contains('`') {
    return Err("openclaw 包名不合法".into());
  }
  Ok(name.to_string())
}

fn parse_node_major(output: &str) -> Option<u32> {
  let line = split_lines(output).into_iter().next()?;
  let trimmed = line.trim_start_matches('v');
  let major = trimmed.split('.').next()?.parse::<u32>().ok()?;
  Some(major)
}

fn split_lines(text: &str) -> Vec<String> {
  text
    .replace("\r\n", "\n")
    .split('\n')
    .map(|l| l.trim().to_string())
    .filter(|l| !l.is_empty())
    .collect()
}

fn find_brew(path_env: &str) -> Option<String> {
  let candidates = ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"];
  for c in candidates {
    if std::path::Path::new(c).exists() {
      return Some(c.into());
    }
  }

  let ok = command_output(path_env, "brew", &["--version"]).is_ok();
  if ok {
    Some("brew".into())
  } else {
    None
  }
}

fn resolve_program(path_env: &str, program: &str) -> Result<PathBuf, String> {
  let program_path = Path::new(program);
  if program_path.is_absolute() || program.contains(std::path::MAIN_SEPARATOR) {
    return Ok(program_path.to_path_buf());
  }

  resolve_command_in_path(program, path_env).ok_or_else(|| "program not found".to_string())
}

fn create_command(path_env: &str, program: &str, args: &[&str]) -> Result<Command, String> {
  let resolved = resolve_program(path_env, program)?;
  let mut cmd = Command::new(resolved);
  cmd.env("PATH", path_env);
  cmd.args(args);
  Ok(cmd)
}

fn run_logged(window: &Window, cancel: &Arc<AtomicBool>, label: &str, cmd: Command) -> Result<i32, String> {
  check_canceled(cancel)?;
  emit_log(window, "install-log", format!("{label} {}", format_command_for_log(&cmd)));
  let w = window.clone();
  let prefix = label.to_string();
  let cancel2 = cancel.clone();
  spawn_with_streaming_logs_cancelable(cmd, cancel2, move |line| emit_log(&w, "install-log", format!("{prefix} {line}")))
}

#[cfg(target_os = "windows")]
fn run_logged_program(
  window: &Window,
  cancel: &Arc<AtomicBool>,
  label: &str,
  path_env: &str,
  program: &str,
  args: &[&str],
) -> Result<i32, String> {
  let cmd = create_command(path_env, program, args)?;
  run_logged(window, cancel, label, cmd)
}

fn format_command_for_log(cmd: &Command) -> String {
  let prog = cmd.get_program().to_string_lossy();
  let mut parts = vec![prog.to_string()];
  for arg in cmd.get_args() {
    let s = arg.to_string_lossy().to_string();
    if s.contains(' ') {
      parts.push(format!("\"{s}\""));
    } else {
      parts.push(s);
    }
  }
  parts.join(" ")
}

fn command_output(path_env: &str, program: &str, args: &[&str]) -> Result<String, String> {
  let mut cmd = create_command(path_env, program, args)?;
  let out = cmd.output().map_err(|e| e.to_string())?;
  Ok(format!(
    "{}\n{}",
    String::from_utf8_lossy(&out.stdout),
    String::from_utf8_lossy(&out.stderr)
  ))
}

#[cfg(target_os = "windows")]
fn ensure_winget(path_env: &str) -> Result<(), String> {
  if command_output(path_env, "winget", &["--version"]).is_ok() {
    Ok(())
  } else {
    Err("未检测到 winget。请先安装 Microsoft App Installer（Windows 软件包管理器）后重试。".into())
  }
}

fn redact_sensitive_args(args: &[String]) -> String {
  let mut out: Vec<String> = Vec::with_capacity(args.len());
  let mut i = 0usize;
  while i < args.len() {
    let current = &args[i];
    out.push(current.clone());
    let is_sensitive_flag = matches!(
      current.as_str(),
      "--custom-api-key" | "--api-key" | "--token" | "--password"
    );
    if is_sensitive_flag {
      if i + 1 < args.len() {
        out.push("\"***\"".into());
        i += 2;
        continue;
      }
    }
    i += 1;
  }
  out.join(" ")
}

fn parse_provider_id_from_config_value(value: &serde_json::Value) -> Option<String> {
  let primary = match value {
    serde_json::Value::String(s) => Some(s.as_str()),
    serde_json::Value::Object(obj) => obj.get("primary").and_then(|v| v.as_str()),
    _ => None,
  }?;
  let trimmed = primary.trim();
  if trimmed.is_empty() {
    return None;
  }
  let provider = trimmed.split('/').next()?.trim();
  if provider.is_empty() {
    return None;
  }
  Some(provider.to_string())
}

fn run_openclaw_collect(
  resolved: &crate::openclaw::ResolvedOpenclaw,
  args: &[&str],
) -> Result<String, String> {
  let mut cmd = Command::new(&resolved.command);
  cmd.env("PATH", &resolved.path_env);
  cmd.args(args);
  let out = cmd.output().map_err(|e| e.to_string())?;
  Ok(format!(
    "{}\n{}",
    String::from_utf8_lossy(&out.stdout),
    String::from_utf8_lossy(&out.stderr)
  ))
}

fn atomic_write_json(path: &Path, value: &serde_json::Value) -> Result<(), String> {
  let pretty = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
  let dir = path.parent().ok_or("invalid config path")?;
  std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;

  let tmp_path = dir.join(format!(
    ".openclaw-helper-{}.tmp",
    SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap_or_default()
      .as_millis()
  ));

  {
    use std::io::Write;
    let mut file = std::fs::File::create(&tmp_path).map_err(|e| e.to_string())?;
    file.write_all(pretty.as_bytes()).map_err(|e| e.to_string())?;
    file.write_all(b"\n").map_err(|e| e.to_string())?;
  }

  // Try to preserve original permissions when possible.
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
      let mode = meta.permissions().mode();
      let _ = std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(mode));
    }
  }

  std::fs::rename(&tmp_path, path).map_err(|e| e.to_string())?;
  Ok(())
}

fn expand_user_path(raw: &str) -> PathBuf {
  let trimmed = raw.trim();
  if trimmed == "~" {
    if let Some(home) = crate::openclaw::home_dir() {
      return home;
    };
  }

  if let Some(rest) = trimmed.strip_prefix("~/") {
    if let Some(home) = crate::openclaw::home_dir() {
      return home.join(rest);
    };
  }

  PathBuf::from(trimmed)
}

fn set_openai_api_mode_openai_responses(
  window: &Window,
  cancel: &Arc<AtomicBool>,
  resolved: &crate::openclaw::ResolvedOpenclaw,
) -> Result<(), String> {
  check_canceled(cancel)?;
  emit_log(window, "install-log", "[config] 设置 api=openai-responses…");

  // 1) Find the active config file path via CLI to avoid guessing.
  let config_file_out = run_openclaw_collect(resolved, &["config", "file"])?;
  let config_file = split_lines(&config_file_out)
    .into_iter()
    .next()
    .ok_or("未能获取 openclaw config file 路径")?;
  let config_path = expand_user_path(config_file.trim());
  if config_path.as_os_str().is_empty() {
    return Err("openclaw config file 返回空路径".into());
  }
  emit_log(
    window,
    "install-log",
    format!("[config] file: {}", config_path.to_string_lossy()),
  );

  // 2) Read JSON.
  let raw = std::fs::read_to_string(&config_path).map_err(|e| {
    format!(
      "{} (path: {})",
      e.to_string(),
      config_path.to_string_lossy()
    )
  })?;
  let mut json: serde_json::Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;

  // 3) Determine provider id from agents.defaults.model.
  let model_value = json
    .pointer("/agents/defaults/model")
    .ok_or("config 缺少 agents.defaults.model")?
    .clone();
  let provider_id = parse_provider_id_from_config_value(&model_value).ok_or("无法从 agents.defaults.model 推断 provider id")?;
  emit_log(window, "install-log", format!("[config] provider: {provider_id}"));

  // 4) Set models.providers.<providerId>.api = openai-responses.
  let providers = json
    .pointer_mut("/models/providers")
    .and_then(|v| v.as_object_mut());
  let providers = match providers {
    Some(p) => p,
    None => {
      // Ensure objects exist.
      if !json.get("models").is_some() {
        json["models"] = serde_json::json!({});
      }
      if !json["models"].get("providers").is_some() {
        json["models"]["providers"] = serde_json::json!({});
      }
      json["models"]["providers"].as_object_mut().ok_or("无法创建 models.providers")?
    }
  };

  if !providers.contains_key(&provider_id) {
    providers.insert(provider_id.clone(), serde_json::json!({}));
  }

  let provider_obj = providers
    .get_mut(&provider_id)
    .and_then(|v| v.as_object_mut())
    .ok_or("models.providers.<provider> 不是对象")?;
  provider_obj.insert("api".into(), serde_json::Value::String("openai-responses".into()));

  atomic_write_json(&config_path, &json)?;
  emit_log(window, "install-log", "[config] ok");
  Ok(())
}

fn log_environment(window: &Window, cancel: &Arc<AtomicBool>, path_env: &str) {
  let _ = check_canceled(cancel);
  emit_log(window, "install-log", "== 环境诊断 ==");
  emit_log(window, "install-log", format!("os: {} / {}", std::env::consts::OS, std::env::consts::ARCH));
  emit_log(window, "install-log", format!("PATH: {path_env}"));

  let safe_run = |label: &str, program: &str, args: &[&str]| {
    if check_canceled(cancel).is_err() {
      return;
    }
    let out = command_output(path_env, program, args)
      .map(|t| split_lines(&t).join(" | "))
      .unwrap_or_else(|e| format!("(failed: {e})"));
    let short = if out.len() > 800 { format!("{}…", &out[..800]) } else { out };
    emit_log(window, "install-log", format!("{label}: {short}"));
  };

  safe_run("brew", "brew", &["--version"]);
  safe_run("git", "git", &["--version"]);
  safe_run("node -v", "node", &["-v"]);
  safe_run("node execPath", "node", &["-p", "process.execPath"]);
  safe_run("npm -v", "npm", &["-v"]);
  safe_run("npm prefix -g", "npm", &["prefix", "-g"]);
  safe_run("npm config get prefix", "npm", &["config", "get", "prefix"]);
  safe_run("npm config get userconfig", "npm", &["config", "get", "userconfig"]);

  // nvm presence (macOS/Linux)
  if let Some(home) = crate::openclaw::home_dir() {
    let nvm_dir = home.join(".nvm");
    emit_log(
      window,
      "install-log",
      format!("nvm dir: {}", if nvm_dir.exists() { "yes" } else { "no" }),
    );
    let versions_dir = nvm_dir.join("versions").join("node");
    if versions_dir.exists() {
      if let Ok(entries) = std::fs::read_dir(&versions_dir) {
        let mut versions: Vec<String> = entries
          .flatten()
          .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
          .collect();
        versions.sort();
        if !versions.is_empty() {
          let joined = versions.join(", ");
          emit_log(window, "install-log", format!("nvm node versions: {joined}"));
        }
      }
    }
  }

  // openclaw resolution
  match resolve_openclaw() {
    Some(resolved) => {
      emit_log(
        window,
        "install-log",
        format!("openclaw resolved: (found) ({})", resolved.source),
      );
      let mut cmd = Command::new(&resolved.command);
      cmd.env("PATH", &resolved.path_env);
      cmd.arg("--version");
      let out = cmd.output().ok().map(|o| {
        let s = format!(
          "{}\n{}",
          String::from_utf8_lossy(&o.stdout),
          String::from_utf8_lossy(&o.stderr)
        );
        split_lines(&s).into_iter().next().unwrap_or_else(|| "(no output)".into())
      });
      emit_log(window, "install-log", format!("openclaw --version: {}", out.unwrap_or_else(|| "(failed)".into())));
    }
    None => emit_log(window, "install-log", "openclaw resolved: (not found)"),
  }

  emit_log(window, "install-log", "== 诊断结束 ==");
}

#[tauri::command]
pub async fn start_install(window: Window, state: tauri::State<'_, TaskState>, options: InstallOptions) -> Result<(), String> {
  const MIN_NODE_MAJOR: u32 = 22;
  let cancel = state.start()?;
  let result = (|| -> Result<(), String> {
    let openclaw_package = validate_npm_package_name(options.openclaw_package.as_deref().unwrap_or("openclaw"))?;
    #[allow(unused_mut)]
    let mut path_env = create_base_path_env();

    let needs_onboard = options
      .custom_api_key
      .as_deref()
      .map(|v| !v.trim().is_empty())
      .unwrap_or(false);
    let total = if needs_onboard { 8u32 } else { 5u32 };
    emit_progress(&window, "prepare", "准备环境…", 1, total);
    emit_log(&window, "install-log", format!("平台：{} / {}", std::env::consts::OS, std::env::consts::ARCH));
    emit_log(&window, "install-log", format!("openclaw 包名：{openclaw_package}"));
    if let Some(reg) = options.npm_registry.as_ref() {
      emit_log(&window, "install-log", format!("npm registry: {reg}"));
    }

    log_environment(&window, &cancel, &path_env);

    check_canceled(&cancel)?;

    emit_progress(&window, "git", "检测 Git…", 2, total);
    let git_ok = command_output(&path_env, "git", &["--version"]).is_ok();
    if !git_ok {
      #[cfg(target_os = "macos")]
      {
        if let Some(brew) = find_brew(&path_env) {
          let mut cmd = Command::new(brew);
          cmd.env("PATH", &path_env);
          cmd.args(["install", "git"]);
          let _ = run_logged(&window, &cancel, "[brew]", cmd)?;
        }
      }
      #[cfg(target_os = "windows")]
      {
        ensure_winget(&path_env)?;
        emit_log(&window, "install-log", "未检测到 Git，尝试通过 winget 安装 Git…");
        let code = run_logged_program(
          &window,
          &cancel,
          "[winget]",
          &path_env,
          "winget",
          &[
            "install",
            "-e",
            "--id",
            "Git.Git",
            "--accept-package-agreements",
            "--accept-source-agreements",
          ],
        )?;
        if code != 0 {
          return Err(format!("winget 安装 Git 失败（退出码 {code}）"));
        }
        path_env = create_base_path_env();
      }
      let git_ok2 = command_output(&path_env, "git", &["--version"]).is_ok();
      if !git_ok2 {
        return Err("未检测到 git。请先安装 git（推荐：Homebrew 安装 git，或安装 Xcode Command Line Tools）。".into());
      }
    }

    emit_progress(&window, "node", "检测 Node.js…", 3, total);
    let node_version_out = command_output(&path_env, "node", &["-v"]).ok();
    let node_major = node_version_out.as_deref().and_then(parse_node_major);
    let npm_ok = command_output(&path_env, "npm", &["-v"]).is_ok();
    let need_node = node_major.map(|m| m < MIN_NODE_MAJOR).unwrap_or(true) || !npm_ok;
    if need_node {
      #[cfg(target_os = "macos")]
      {
        let brew = find_brew(&path_env).ok_or("未检测到 Node.js，且未检测到 brew。请先安装 Homebrew 或手动安装 Node.js。")?;
        let mut cmd = Command::new(brew);
        cmd.env("PATH", &path_env);
        cmd.args(["install", "node"]);
        let _ = run_logged(&window, &cancel, "[brew]", cmd)?;
      }
      #[cfg(target_os = "windows")]
      {
        ensure_winget(&path_env)?;
        emit_log(&window, "install-log", "Node.js / npm 不满足要求，尝试通过 winget 修复…");

        let upgrade_code = run_logged_program(
          &window,
          &cancel,
          "[winget]",
          &path_env,
          "winget",
          &[
            "upgrade",
            "-e",
            "--id",
            "OpenJS.NodeJS.LTS",
            "--accept-package-agreements",
            "--accept-source-agreements",
          ],
        )?;
        if upgrade_code != 0 {
          emit_log(
            &window,
            "install-log",
            format!("[winget] upgrade 返回退出码 {upgrade_code}，继续检查是否需要 install…"),
          );
        }

        path_env = create_base_path_env();
        let node_after_upgrade = command_output(&path_env, "node", &["-v"]).ok();
        let node_major_after_upgrade = node_after_upgrade.as_deref().and_then(parse_node_major);
        let npm_after_upgrade = command_output(&path_env, "npm", &["-v"]).is_ok();
        let still_need_install = node_major_after_upgrade.map(|m| m < MIN_NODE_MAJOR).unwrap_or(true) || !npm_after_upgrade;

        if still_need_install {
          let install_code = run_logged_program(
            &window,
            &cancel,
            "[winget]",
            &path_env,
            "winget",
            &[
              "install",
              "-e",
              "--id",
              "OpenJS.NodeJS.LTS",
              "--accept-package-agreements",
              "--accept-source-agreements",
            ],
          )?;
          if install_code != 0 {
            return Err(format!("winget 安装 Node.js 失败（退出码 {install_code}）"));
          }
          path_env = create_base_path_env();
        }
      }
      #[cfg(not(any(target_os = "macos", target_os = "windows")))]
      {
        return Err(format!(
          "未检测到可用 Node.js（需要 >= {MIN_NODE_MAJOR}）。请先安装 Node.js 后重试。"
        ));
      }
    }

    let node_version_out2 = command_output(&path_env, "node", &["-v"]).map_err(|_| "安装后仍未检测到 node".to_string())?;
    let node_major2 = parse_node_major(&node_version_out2).ok_or("无法解析 node 版本")?;
    if node_major2 < MIN_NODE_MAJOR {
      return Err(format!("Node.js 版本过低：{node_version_out2}（需要 >= {MIN_NODE_MAJOR}）"));
    }

    // Ensure npm exists (some Node installations might be incomplete).
    let npm_ok_final = command_output(&path_env, "npm", &["-v"]).is_ok();
    if !npm_ok_final {
      return Err("未检测到 npm。请确认 Node.js 安装完整，或重新安装 Node.js 后重试。".into());
    }

    emit_progress(&window, "openclaw", "全局安装 openclaw…", 4, total);
    check_canceled(&cancel)?;
    let mut npm_cmd = create_command(&path_env, "npm", &["install", "-g", &openclaw_package])?;
    npm_cmd.env("npm_config_progress", "false");
    npm_cmd.env("npm_config_fund", "false");
    npm_cmd.env("npm_config_audit", "false");
    if let Some(reg) = options.npm_registry.as_ref() {
      npm_cmd.env("npm_config_registry", reg);
    }
    let code = run_logged(&window, &cancel, "[npm]", npm_cmd)?;
    if code != 0 {
      return Err("npm install -g 失败，请查看日志。".into());
    }

    emit_progress(&window, "verify", "验证 openclaw 命令…", 5, total);
    check_canceled(&cancel)?;
    let info = get_openclaw_info(false);
    if !info.installed {
      let err = info.error.unwrap_or_else(|| "unknown".into());
      return Err(format!("openclaw 安装完成，但无法执行 openclaw：{err}"));
    }

    let version = info.version.unwrap_or_else(|| "unknown".into());
    emit_log(&window, "install-log", format!("openclaw --version => {version}"));

    if needs_onboard {
      emit_progress(&window, "onboard", "自动配置 openclaw…", 6, total);
      check_canceled(&cancel)?;

      let base_url = options
        .custom_base_url
        .as_deref()
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .unwrap_or("https://sub.yeelovo.com/v1");
      let model_id = options
        .custom_model_id
        .as_deref()
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .unwrap_or("gpt-5.4");
      let api_key = options.custom_api_key.as_deref().unwrap_or("").trim().to_string();
      if api_key.is_empty() {
        return Err("缺少 CUSTOM_API_KEY（用于 openclaw onboard）。".into());
      }

      let resolved = resolve_openclaw().ok_or("未检测到 openclaw，请先完成安装。")?;
      let mut cmd = Command::new(&resolved.command);
      cmd.env("PATH", &resolved.path_env);
      cmd.args([
        "onboard",
        "--non-interactive",
        "--accept-risk",
        "--install-daemon",
        "--skip-ui",
        "--skip-health",
        "--skip-skills",
        "--skip-search",
        "--skip-channels",
        "--auth-choice",
        "custom-api-key",
        "--custom-base-url",
        base_url,
        "--custom-model-id",
        model_id,
        "--custom-api-key",
        &api_key,
        "--secret-input-mode",
        "plaintext",
        "--custom-compatibility",
        "openai",
      ]);

      let args_for_log: Vec<String> = std::iter::once("openclaw".to_string())
        .chain(cmd.get_args().map(|a| a.to_string_lossy().to_string()))
        .collect();
      emit_log(&window, "install-log", format!("[openclaw] {}", redact_sensitive_args(&args_for_log)));

      let w = window.clone();
      let cancel2 = cancel.clone();
      let code = spawn_with_streaming_logs_cancelable(cmd, cancel2, move |line| {
        emit_log(&w, "install-log", format!("[openclaw] {line}"));
      })?;
      if code != 0 {
        // On failure, print gateway status to help debugging common "gateway closed" cases.
        emit_log(&window, "install-log", "[openclaw] onboard 失败，尝试输出 gateway status 以便排查…");
        if let Ok(status_text) = run_openclaw_collect(&resolved, &["gateway", "status", "--no-color"]) {
          for line in split_lines(&status_text).into_iter().take(60) {
            emit_log(&window, "install-log", format!("[gateway] {line}"));
          }
        }
        return Err(format!("openclaw onboard 失败（退出码 {code}）"));
      }

      emit_progress(&window, "config", "写入 openai-responses 配置…", 7, total);
      set_openai_api_mode_openai_responses(&window, &cancel, &resolved)?;

      emit_progress(&window, "gateway", "重启网关服务…", 8, total);
      check_canceled(&cancel)?;
      let mut restart = Command::new(&resolved.command);
      restart.env("PATH", &resolved.path_env);
      restart.args(["gateway", "restart"]);
      let w2 = window.clone();
      let cancel3 = cancel.clone();
      let code2 = spawn_with_streaming_logs_cancelable(restart, cancel3, move |line| {
        emit_log(&w2, "install-log", format!("[openclaw] {line}"));
      })?;
      if code2 != 0 {
        return Err(format!("网关重启失败（退出码 {code2}）。你可以手动运行：openclaw gateway restart"));
      }
    } else {
      emit_log(&window, "install-log", "跳过自动配置：未提供 CUSTOM_API_KEY。");
    }
    let _ = window.emit("install-progress", ProgressPayload {
      stage: "done".into(),
      title: "完成".into(),
      index: total,
      total,
      percent: 1.0,
    });
    Ok(())
  })();

  state.finish();
  result
}

#[tauri::command]
pub async fn cancel_task(state: tauri::State<'_, TaskState>) -> Result<(), String> {
  state.cancel();
  Ok(())
}
