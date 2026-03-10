use crate::openclaw::{
  cleanup_all_mac_nvm_openclaw, cleanup_mac_nvm_openclaw, create_base_path_env, get_openclaw_info, parse_gateway_status,
  apply_windows_no_window, resolve_command_in_path, resolve_openclaw, spawn_with_streaming_logs_cancelable, GatewayStatus, OpenclawInfo,
};
use serde::Deserialize;
use serde::Serialize;
#[cfg(target_os = "windows")]
use std::fs;
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

#[cfg(target_os = "windows")]
fn is_windows_admin(path_env: &str) -> bool {
  // Best-effort: `net session` succeeds only when elevated (Admin).
  // It may fail for other reasons, so treat failures as "not admin".
  let Ok(mut cmd) = create_command(path_env, "net", &["session"]) else {
    return false;
  };
  cmd.stdout(std::process::Stdio::null());
  cmd.stderr(std::process::Stdio::null());
  cmd.status().map(|s| s.success()).unwrap_or(false)
}

#[cfg(target_os = "windows")]
fn powershell_escape_single_quoted(value: &str) -> String {
  value.replace('\'', "''")
}

#[cfg(target_os = "windows")]
fn run_openclaw_elevated(
  window: &Window,
  cancel: &Arc<AtomicBool>,
  resolved: &crate::openclaw::ResolvedOpenclaw,
  args: &[&str],
) -> Result<i32, String> {
  check_canceled(cancel)?;
  let openclaw_path = resolved.command.to_string_lossy().to_string();
  let openclaw_path_escaped = powershell_escape_single_quoted(&openclaw_path);

  let is_cmd = resolved
    .command
    .extension()
    .and_then(|e| e.to_str())
    .map(|e| matches!(e.to_ascii_lowercase().as_str(), "cmd" | "bat"))
    .unwrap_or(false);

  let arg_list: Vec<String> = args.iter().map(|a| a.to_string()).collect();
  let arg_list_ps = arg_list
    .iter()
    .map(|a| format!("'{}'", powershell_escape_single_quoted(a)))
    .collect::<Vec<_>>()
    .join(", ");

  // Note: Output from the elevated OpenClaw process won't be streamed back here.
  // We'll just wait for completion and return the exit code.
  let script = if is_cmd {
    let cmdline = format!(
      "\"\"{}\" {}\"",
      openclaw_path_escaped,
      args.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(" ")
    );
    format!(
      "$ErrorActionPreference='Stop';\n\
       $command='{cmdline}';\n\
       $p=Start-Process -FilePath 'cmd.exe' -ArgumentList @('/d','/s','/c',$command) -Verb RunAs -PassThru -Wait -WindowStyle Hidden;\n\
       exit $p.ExitCode\n"
    )
  } else {
    format!(
      "$ErrorActionPreference='Stop';\n\
       $p=Start-Process -FilePath '{openclaw_path_escaped}' -ArgumentList @({arg_list_ps}) -Verb RunAs -PassThru -Wait -WindowStyle Hidden;\n\
       exit $p.ExitCode\n"
    )
  };

  let mut ps = Command::new("powershell");
  apply_windows_no_window(&mut ps);
  ps.env("PATH", &resolved.path_env);
  ps.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &script]);

  run_logged(window, cancel, "[powershell]", ps)
}

#[cfg(target_os = "windows")]
fn openclaw_extract_windows_gateway_task_name_from_status(output: &str) -> Option<String> {
  // Example line (from `openclaw gateway status`):
  // "Start with: schtasks /Run /TN \"OpenClaw Gateway\""
  for line in split_lines(output) {
    let lower = line.to_ascii_lowercase();
    if !lower.contains("schtasks") || !lower.contains("/tn") {
      continue;
    }

    let Some(tn_index) = lower.find("/tn") else {
      continue;
    };

    let after = line.get(tn_index + 3..).unwrap_or_default();
    let after = after.trim_start();
    if after.is_empty() {
      continue;
    }

    if let Some(rest) = after.strip_prefix('\"') {
      if let Some(end) = rest.find('\"') {
        let name = rest[..end].trim();
        if !name.is_empty() {
          return Some(name.to_string());
        }
      }
      continue;
    }

    // No quotes: take the next token.
    let token = after.split_whitespace().next().unwrap_or_default().trim();
    if !token.is_empty() {
      return Some(token.to_string());
    }
  }
  None
}

#[cfg(target_os = "windows")]
fn openclaw_windows_gateway_task_name_best_effort(resolved: &crate::openclaw::ResolvedOpenclaw) -> String {
  if let Ok(name) = std::env::var("OPENCLAW_WINDOWS_TASK_NAME") {
    let trimmed = name.trim();
    if !trimmed.is_empty() {
      return trimmed.to_string();
    }
  }

  // Prefer extracting the task name from OpenClaw itself, because some builds/profiles
  // may use a different task name.
  let mut status_cmd = Command::new(&resolved.command);
  apply_windows_no_window(&mut status_cmd);
  status_cmd.env("PATH", &resolved.path_env);
  status_cmd.args(["--no-color", "gateway", "status"]);
  if let Ok(output) = status_cmd.output() {
    let combined = format!(
      "{}\n{}",
      String::from_utf8_lossy(&output.stdout),
      String::from_utf8_lossy(&output.stderr)
    );
    if let Some(name) = openclaw_extract_windows_gateway_task_name_from_status(&combined) {
      return name;
    }
  }

  "OpenClaw Gateway".into()
}

#[cfg(target_os = "windows")]
fn windows_schtasks_output_is_already_running(output: &str) -> bool {
  let lower = output.to_ascii_lowercase();
  lower.contains("already running") || lower.contains("任务已在运行") || lower.contains("正在运行")
}

#[cfg(target_os = "windows")]
fn run_openclaw_logged_capture(
  window: &Window,
  cancel: &Arc<AtomicBool>,
  label: &str,
  cmd: Command,
) -> Result<(i32, String), String> {
  check_canceled(cancel)?;
  emit_log(window, "openclaw-log", format!("{label} {}", format_command_for_log(&cmd)));

  let captured = Arc::new(Mutex::new(String::new()));
  let captured2 = captured.clone();

  let w = window.clone();
  let prefix = label.to_string();
  let cancel2 = cancel.clone();
  let code = spawn_with_streaming_logs_cancelable(cmd, cancel2, move |line| {
    emit_log(&w, "openclaw-log", format!("{prefix} {line}"));
    if let Ok(mut buf) = captured2.lock() {
      if buf.len() < 120_000 {
        buf.push_str(&line);
        buf.push('\n');
      }
    }
  })?;

  let text = captured.lock().map(|s| s.clone()).unwrap_or_default();
  Ok((code, text))
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
  apply_windows_no_window(&mut cmd);
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

  let w2 = window.clone();
  let cancel2 = cancel.clone();

  // Run blocking process execution on a dedicated thread so the async runtime can keep
  // processing other invocations (like cancel_task).
  let join = tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
    let resolved = resolve_openclaw().ok_or("未检测到 openclaw，请先完成安装。")?;

    emit_log(&w2, "openclaw-log", format!("openclaw {} ({})", args.join(" "), resolved.source));

    // Windows: installing the gateway service needs admin privileges (schtasks create).
    // If user isn't elevated, run `openclaw gateway install` via UAC so "Start" works in-app.
    #[cfg(target_os = "windows")]
    {
      let is_gateway_install = args.len() >= 2 && args[0] == "gateway" && args[1] == "install";
      if is_gateway_install && !is_windows_admin(&resolved.path_env) {
        emit_log(
          &w2,
          "openclaw-log",
          "[tip] 安装网关服务需要管理员权限，将弹出 UAC 授权窗口（点“是”继续）。",
        );
        let code = run_openclaw_elevated(&w2, &cancel2, &resolved, &["--no-color", "gateway", "install"])?;
        if code == 0 {
          emit_log(&w2, "openclaw-log", "[gateway] install: ok");
          return Ok(());
        }
        return Err(format!("gateway install 失败（退出码 {code}）"));
      }

      // Some Windows environments hang after printing "Restarted Scheduled Task: OpenClaw Gateway"
      // when running `openclaw gateway start`. To keep the UI responsive, start the scheduled task
      // directly and return once schtasks succeeds.
      let is_gateway_start = args.len() == 2 && args[0] == "gateway" && args[1] == "start";
      if is_gateway_start {
        let task_name = openclaw_windows_gateway_task_name_best_effort(&resolved);
        emit_log(
          &w2,
          "openclaw-log",
          format!("[windows] gateway start => schtasks /Run (task: {task_name})"),
        );

        // Stop any existing instance (best-effort).
        let mut end_cmd = Command::new("schtasks");
        apply_windows_no_window(&mut end_cmd);
        end_cmd.env("PATH", &resolved.path_env);
        end_cmd.args(["/End", "/TN"]);
        end_cmd.arg(&task_name);
        let (end_code, _end_out) = run_openclaw_logged_capture(&w2, &cancel2, "[schtasks]", end_cmd)?;
        if end_code != 0 {
          emit_log(&w2, "openclaw-log", "[warn] schtasks /End 失败（已忽略）");
        }

        let mut run_cmd = Command::new("schtasks");
        apply_windows_no_window(&mut run_cmd);
        run_cmd.env("PATH", &resolved.path_env);
        run_cmd.args(["/Run", "/TN"]);
        run_cmd.arg(&task_name);
        let (run_code, run_out) = run_openclaw_logged_capture(&w2, &cancel2, "[schtasks]", run_cmd)?;
        if run_code == 0 || windows_schtasks_output_is_already_running(&run_out) {
          emit_log(&w2, "openclaw-log", "[gateway] start triggered (schtasks)");
          emit_log(
            &w2,
            "openclaw-log",
            "[tip] 若状态未及时刷新，请等待几秒后点击“检查网关状态”。",
          );
          return Ok(());
        }

        return Err(format!("gateway start 失败（schtasks 退出码 {run_code}）"));
      }
    }

    let mut cmd = Command::new(&resolved.command);
    apply_windows_no_window(&mut cmd);
    cmd.env("PATH", &resolved.path_env);
    cmd.arg("--no-color");
    for a in args {
      cmd.arg(a);
    }

    let w3 = w2.clone();
    let code = spawn_with_streaming_logs_cancelable(cmd, cancel2, move |line| emit_log(&w3, "openclaw-log", line))?;
    if code == 0 {
      Ok(())
    } else {
      Err(format!("openclaw 退出码：{code}"))
    }
  });

  let result = match join.await {
    Ok(r) => r,
    Err(e) => Err(format!("内部错误：任务线程异常：{e}")),
  };
  state.finish();
  result
}

#[tauri::command]
pub async fn open_dashboard(_app: AppHandle) -> Result<String, String> {
  let resolved = resolve_openclaw().ok_or("未检测到 openclaw，请先完成安装。")?;

  let mut cmd = Command::new(&resolved.command);
  apply_windows_no_window(&mut cmd);
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
  let w2 = window.clone();
  let cancel2 = cancel.clone();

  let join = tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
    let resolved = resolve_openclaw();
    let path_env = resolved
      .as_ref()
      .map(|r| r.path_env.clone())
      .unwrap_or_else(create_base_path_env);
    let openclaw_cmd = resolved.as_ref().map(|r| r.command.clone());

    emit_log(&w2, "install-log", "[uninstall] start");
    if let Some(resolved) = resolved.as_ref() {
      emit_log(
        &w2,
        "install-log",
        format!(
          "[uninstall] openclaw resolved: {} ({})",
          resolved.command.to_string_lossy(),
          resolved.source
        ),
      );
      emit_log(
        &w2,
        "install-log",
        "openclaw uninstall --service --state --workspace --yes --non-interactive",
      );
    } else {
      emit_log(
        &w2,
        "install-log",
        "[warn] 未检测到 openclaw 命令，跳过 openclaw uninstall，直接尝试清理…",
      );
    }

    // 1) OpenClaw's own uninstaller (service/state/workspace).
    if let Some(resolved) = resolved.as_ref() {
      let mut cmd = Command::new(&resolved.command);
      apply_windows_no_window(&mut cmd);
      cmd.env("PATH", &resolved.path_env);
      cmd.args([
        "uninstall",
        "--service",
        "--state",
        "--workspace",
        "--yes",
        "--non-interactive",
      ]);
      let w = w2.clone();
      let code = spawn_with_streaming_logs_cancelable(cmd, cancel2.clone(), move |line| emit_log(&w, "install-log", line))?;
      if code != 0 {
        // If the CLI is partially broken (common when npm shims remain but module files are gone),
        // continue with manual cleanup so the user can still recover.
        emit_log(
          &w2,
          "install-log",
          format!(
            "[warn] openclaw uninstall 失败（退出码 {code}），将继续尝试通过 npm/pnpm 等方式清理…"
          ),
        );
      }
    }

    // 2) Best-effort remove CLI from common managers.
    const OPENCLAW_NPM_PACKAGE: &str = "openclaw";
    emit_log(
      &w2,
      "install-log",
      "正在尝试移除 openclaw CLI（brew / npm / pnpm / nvm；Windows: nvm-windows / npm shim）…",
    );

    let run_best_effort = |label: &str, program: &str, args: &[&str]| {
      check_canceled(&cancel2)?;
      match create_command(&path_env, program, args) {
        Ok(cmd) => match run_logged(&w2, &cancel2, label, cmd) {
          Ok(code) => {
            if code != 0 {
              emit_log(
                &w2,
                "install-log",
                format!("[cleanup] {label} exited with code {code} (ignored)"),
              );
            }
          }
          Err(err) => {
            emit_log(&w2, "install-log", format!("[cleanup] {label} failed (ignored): {err}"));
          }
        },
        Err(err) => {
          emit_log(&w2, "install-log", format!("[cleanup] {label} skipped: {err}"));
        }
      }
      Ok::<(), String>(())
    };

    #[cfg(target_os = "macos")]
    {
      let brew_candidates = ["/opt/homebrew/bin/brew", "/usr/local/bin/brew", "brew"];
      for brew in brew_candidates {
        check_canceled(&cancel2)?;
        run_best_effort("[brew]", brew, &["uninstall", "openclaw"])?;
      }
    }

    // npm uninstall -g openclaw (best-effort)
    {
      #[cfg(target_os = "windows")]
      {
        // Prefer the npm that lives next to the resolved openclaw shim when using nvm-windows,
        // otherwise we might run a different Node installation's npm and not remove the right CLI.
        let openclaw_dir = openclaw_cmd.as_ref().and_then(|c| c.parent().map(|p| p.to_path_buf()));
        let npm_next_to_openclaw = openclaw_dir.map(|d| d.join("npm.cmd")).filter(|p| p.is_file());
        if let Some(npm_cmd) = npm_next_to_openclaw {
          let npm_cmd = npm_cmd.to_string_lossy().to_string();
          emit_log(&w2, "install-log", format!("[cleanup] using npm: {npm_cmd}"));
          run_best_effort("[npm]", &npm_cmd, &["uninstall", "-g", OPENCLAW_NPM_PACKAGE])?;
        } else {
          run_best_effort("[npm]", "npm", &["uninstall", "-g", OPENCLAW_NPM_PACKAGE])?;
        }
      }
      #[cfg(not(target_os = "windows"))]
      {
        run_best_effort("[npm]", "npm", &["uninstall", "-g", OPENCLAW_NPM_PACKAGE])?;
      }
    }

    // pnpm remove -g openclaw (best-effort)
    {
      run_best_effort("[pnpm]", "pnpm", &["remove", "-g", OPENCLAW_NPM_PACKAGE])?;
    }

    // nvm scan cleanup (delete ~/.nvm/... copy if that's where we resolved from)
    {
      check_canceled(&cancel2)?;
      let mut removed = openclaw_cmd
        .as_ref()
        .and_then(|cmd| cleanup_mac_nvm_openclaw(cmd, OPENCLAW_NPM_PACKAGE).ok())
        .unwrap_or_default();
      removed.extend(cleanup_all_mac_nvm_openclaw(OPENCLAW_NPM_PACKAGE).unwrap_or_default());
      if !removed.is_empty() {
        emit_log(&w2, "install-log", format!("[cleanup] nvm: removed {} item(s)", removed.len()));
        for item in removed {
          emit_log(&w2, "install-log", format!("[cleanup] nvm: {item}"));
        }
      }
    }

    // 2.5) Best-effort remove OpenClaw state/workspace dir when CLI uninstall fails.
    // If `openclaw uninstall --state/--workspace` ran successfully, this is a no-op.
    {
      check_canceled(&cancel2)?;
      if let Some(home) = crate::openclaw::home_dir() {
        let state_dir = home.join(".openclaw");
        if state_dir.exists() {
          match std::fs::remove_dir_all(&state_dir) {
            Ok(()) => emit_log(
              &w2,
              "install-log",
              format!("[cleanup] removed: {}", state_dir.to_string_lossy()),
            ),
            Err(err) => emit_log(
              &w2,
              "install-log",
              format!("[cleanup] failed to remove {}: {}", state_dir.to_string_lossy(), err),
            ),
          }
        }
      }
    }

    // 3) Verify CLI is gone; otherwise try Windows shim removal (common with nvm-windows).
    check_canceled(&cancel2)?;
    if let Some(still) = resolve_openclaw() {
      emit_log(
        &w2,
        "install-log",
        format!(
          "[cleanup] openclaw 仍存在：{} ({})",
          still.command.to_string_lossy(),
          still.source
        ),
      );

      #[cfg(target_os = "windows")]
      {
        let still_dir = still.command.parent().map(|p| p.to_path_buf());
        if let Some(dir) = still_dir {
          // Try again with npm.cmd next to the shim (common for nvm4w symlink dir).
          let npm_local = dir.join("npm.cmd");
          if npm_local.is_file() {
            let npm_local = npm_local.to_string_lossy().to_string();
            emit_log(&w2, "install-log", format!("[cleanup] retry npm: {npm_local}"));
            run_best_effort("[npm]", &npm_local, &["uninstall", "-g", OPENCLAW_NPM_PACKAGE])?;
          }

          let shim_names = ["openclaw.cmd", "openclaw", "openclaw.ps1"];
          let mut removed: Vec<String> = Vec::new();
          for name in shim_names {
            check_canceled(&cancel2)?;
            let target = dir.join(name);
            if !target.is_file() {
              continue;
            }
            match fs::remove_file(&target) {
              Ok(()) => {
                removed.push(target.to_string_lossy().to_string());
              }
              Err(err) => {
                emit_log(
                  &w2,
                  "install-log",
                  format!("[cleanup] failed to remove shim {}: {}", target.to_string_lossy(), err),
                );
              }
            }
          }
          if !removed.is_empty() {
            emit_log(&w2, "install-log", format!("[cleanup] removed {} shim(s)", removed.len()));
            for item in removed {
              emit_log(&w2, "install-log", format!("[cleanup] shim: {item}"));
            }
          }

          // If openclaw shims are present but the module is broken/missing (e.g. MODULE_NOT_FOUND),
          // remove the global module dir next to the shim as a last resort.
          check_canceled(&cancel2)?;
          let module_dir = dir.join("node_modules").join(OPENCLAW_NPM_PACKAGE);
          if module_dir.is_dir() {
            match fs::remove_dir_all(&module_dir) {
              Ok(()) => emit_log(
                &w2,
                "install-log",
                format!("[cleanup] removed module dir: {}", module_dir.to_string_lossy()),
              ),
              Err(err) => emit_log(
                &w2,
                "install-log",
                format!(
                  "[cleanup] failed to remove module dir {}: {}",
                  module_dir.to_string_lossy(),
                  err
                ),
              ),
            }
          }
        }
      }

      check_canceled(&cancel2)?;
      if let Some(still2) = resolve_openclaw() {
        return Err(format!(
          "已执行卸载，但系统中仍能找到 openclaw：{}（{}）。\n可能原因：权限不足/多个 Node 环境。\n建议：在终端运行 `npm uninstall -g {OPENCLAW_NPM_PACKAGE}`（必要时以管理员权限），然后重启终端再试。",
          still2.command.to_string_lossy(),
          still2.source
        ));
      }
    }

    emit_log(&w2, "install-log", "卸载完成：已找不到 openclaw 命令。");
    Ok(())
  });

  let result = match join.await {
    Ok(r) => r,
    Err(e) => Err(format!("内部错误：任务线程异常：{e}")),
  };
  state.finish();
  result
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct InstallOptions {
  #[serde(rename = "openclawPackage")]
  pub openclaw_package: Option<String>,
  #[serde(rename = "npmRegistry")]
  pub npm_registry: Option<String>,
  #[serde(rename = "githubMirror")]
  pub github_mirror: Option<String>,
  #[serde(rename = "customBaseUrl")]
  pub custom_base_url: Option<String>,
  #[serde(rename = "customModelId")]
  pub custom_model_id: Option<String>,
  #[serde(rename = "customApiKey")]
  pub custom_api_key: Option<String>,
}

fn is_disable_keyword(value: &str) -> bool {
  let lower = value.trim().to_ascii_lowercase();
  matches!(lower.as_str(), "off" | "false" | "none" | "direct" | "disable" | "disabled" | "0")
}

fn validate_github_mirror(value: &str) -> Result<String, String> {
  let trimmed = value.trim();
  if trimmed.is_empty() {
    return Err("GitHub mirror 不能为空".into());
  }
  if trimmed.chars().any(|c| c.is_whitespace()) {
    return Err("GitHub mirror 不能包含空格".into());
  }
  if trimmed.contains('\'') || trimmed.contains('\"') || trimmed.contains('`') {
    return Err("GitHub mirror 不合法".into());
  }
  if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
    return Err("GitHub mirror 必须以 http:// 或 https:// 开头".into());
  }
  let mut out = trimmed.to_string();
  if !out.ends_with('/') {
    out.push('/');
  }
  Ok(out)
}

fn run_logged_capture(window: &Window, cancel: &Arc<AtomicBool>, label: &str, cmd: Command) -> Result<(i32, String), String> {
  check_canceled(cancel)?;
  emit_log(window, "install-log", format!("{label} {}", format_command_for_log(&cmd)));

  let captured = Arc::new(Mutex::new(String::new()));
  let captured2 = captured.clone();

  let w = window.clone();
  let prefix = label.to_string();
  let cancel2 = cancel.clone();
  let code = spawn_with_streaming_logs_cancelable(cmd, cancel2, move |line| {
    emit_log(&w, "install-log", format!("{prefix} {line}"));
    if let Ok(mut buf) = captured2.lock() {
      if buf.len() < 120_000 {
        buf.push_str(&line);
        buf.push('\n');
      }
    }
  })?;

  let text = captured.lock().map(|s| s.clone()).unwrap_or_default();
  Ok((code, text))
}

fn output_looks_like_github_connectivity_issue(output: &str) -> bool {
  let lower = output.to_ascii_lowercase();
  if !lower.contains("github.com") {
    return false;
  }
  let patterns = [
    "failed to connect to github.com",
    "could not connect to server",
    "couldn't connect to server",
    "could not resolve host: github.com",
    "unable to access 'https://github.com",
    "connection timed out",
    "operation timed out",
    "timed out",
  ];
  patterns.iter().any(|p| lower.contains(p))
}

fn apply_github_git_rewrite_env(window: &Window, cmd: &mut Command, github_mirror: Option<&str>) {
  if let Some(mirror) = github_mirror {
    emit_log(
      window,
      "install-log",
      format!("已启用 GitHub 镜像：{mirror}（用于加速/绕过 GitHub 访问问题；仅本次安装生效）"),
    );
    cmd.env("GIT_CONFIG_COUNT", "3");
    cmd.env("GIT_CONFIG_KEY_0", format!("url.{mirror}.insteadOf"));
    cmd.env("GIT_CONFIG_VALUE_0", "ssh://git@github.com/");
    cmd.env("GIT_CONFIG_KEY_1", format!("url.{mirror}.insteadOf"));
    cmd.env("GIT_CONFIG_VALUE_1", "git@github.com:");
    cmd.env("GIT_CONFIG_KEY_2", format!("url.{mirror}.insteadOf"));
    cmd.env("GIT_CONFIG_VALUE_2", "https://github.com/");
    return;
  }

  // Default: rewrite GitHub SSH URLs to HTTPS (avoid requiring SSH keys).
  emit_log(
    window,
    "install-log",
    "已启用 GitHub SSH -> HTTPS 重写（避免 git@github.com 权限问题；仅本次安装生效）",
  );
  cmd.env("GIT_CONFIG_COUNT", "2");
  cmd.env("GIT_CONFIG_KEY_0", "url.https://github.com/.insteadOf");
  cmd.env("GIT_CONFIG_VALUE_0", "ssh://git@github.com/");
  cmd.env("GIT_CONFIG_KEY_1", "url.https://github.com/.insteadOf");
  cmd.env("GIT_CONFIG_VALUE_1", "git@github.com:");
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
  apply_windows_no_window(&mut cmd);
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
  apply_windows_no_window(&mut cmd);
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
  if trimmed.is_empty() {
    return PathBuf::new();
  }

  let trimmed = trimmed.trim_matches(|c| c == '"' || c == '\'');

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

  if let Some(rest) = trimmed.strip_prefix("~\\") {
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
      apply_windows_no_window(&mut cmd);
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

fn start_install_blocking(window: &Window, cancel: &Arc<AtomicBool>, options: InstallOptions) -> Result<(), String> {
  const MIN_NODE_MAJOR: u32 = 22;
  const DEFAULT_NPM_REGISTRY: &str = "https://registry.npmmirror.com";
  const DEFAULT_GITHUB_MIRROR: &str = "https://gitclone.com/github.com/";

  let openclaw_package = validate_npm_package_name(options.openclaw_package.as_deref().unwrap_or("openclaw"))?;
  let npm_registry = options
    .npm_registry
    .as_deref()
    .map(|v| v.trim().to_string())
    .filter(|v| !v.is_empty())
    .unwrap_or_else(|| DEFAULT_NPM_REGISTRY.to_string());
  let github_mirror_user = options
    .github_mirror
    .as_deref()
    .map(|v| v.trim())
    .filter(|v| !v.is_empty());
  let github_mirror_disabled = github_mirror_user.map(|v| is_disable_keyword(v)).unwrap_or(false);
  let mut github_mirror = match github_mirror_user {
    Some(v) if is_disable_keyword(v) => None,
    Some(v) => Some(validate_github_mirror(v)?),
    None => None,
  };
  #[cfg(target_os = "windows")]
  {
    if github_mirror.is_none() && !github_mirror_disabled {
      github_mirror = Some(DEFAULT_GITHUB_MIRROR.to_string());
    }
  }

  #[allow(unused_mut)]
  let mut path_env = create_base_path_env();

  let needs_onboard = options
    .custom_api_key
    .as_deref()
    .map(|v| !v.trim().is_empty())
    .unwrap_or(false);
  let total = if needs_onboard { 8u32 } else { 5u32 };
  emit_progress(window, "prepare", "准备环境…", 1, total);
  emit_log(window, "install-log", format!("平台：{} / {}", std::env::consts::OS, std::env::consts::ARCH));
  emit_log(window, "install-log", format!("openclaw 包名：{openclaw_package}"));
  emit_log(window, "install-log", format!("npm registry: {npm_registry}"));
  if let Some(mirror) = github_mirror.as_deref() {
    emit_log(window, "install-log", format!("GitHub mirror: {mirror}"));
  } else {
    emit_log(window, "install-log", "GitHub mirror: (none)");
  }

  log_environment(window, cancel, &path_env);

  check_canceled(cancel)?;

  emit_progress(window, "git", "检测 Git…", 2, total);
  let git_ok = command_output(&path_env, "git", &["--version"]).is_ok();
  if !git_ok {
    #[cfg(target_os = "macos")]
    {
      if let Some(brew) = find_brew(&path_env) {
        let mut cmd = Command::new(brew);
        apply_windows_no_window(&mut cmd);
        cmd.env("PATH", &path_env);
        cmd.args(["install", "git"]);
        let _ = run_logged(window, cancel, "[brew]", cmd)?;
      }
    }
    #[cfg(target_os = "windows")]
    {
      ensure_winget(&path_env)?;
      emit_log(window, "install-log", "未检测到 Git，尝试通过 winget 安装 Git…");
      let code = run_logged_program(
        window,
        cancel,
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

  emit_progress(window, "node", "检测 Node.js…", 3, total);
  let node_version_out = command_output(&path_env, "node", &["-v"]).ok();
  let node_major = node_version_out.as_deref().and_then(parse_node_major);
  let npm_ok = command_output(&path_env, "npm", &["-v"]).is_ok();
  let need_node = node_major.map(|m| m < MIN_NODE_MAJOR).unwrap_or(true) || !npm_ok;
  if need_node {
    #[cfg(target_os = "macos")]
    {
      let brew = find_brew(&path_env).ok_or("未检测到 Node.js，且未检测到 brew。请先安装 Homebrew 或手动安装 Node.js。")?;
      let mut cmd = Command::new(brew);
      apply_windows_no_window(&mut cmd);
      cmd.env("PATH", &path_env);
      cmd.args(["install", "node"]);
      let _ = run_logged(window, cancel, "[brew]", cmd)?;
    }
    #[cfg(target_os = "windows")]
    {
      ensure_winget(&path_env)?;
      emit_log(window, "install-log", "Node.js / npm 不满足要求，尝试通过 winget 修复…");

      let upgrade_code = run_logged_program(
        window,
        cancel,
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
          window,
          "install-log",
          format!("[winget] upgrade 返回退出码 {upgrade_code}，继续检查是否需要 install…"),
        );
      }

      path_env = create_base_path_env();
      let node_after_upgrade = command_output(&path_env, "node", &["-v"]).ok();
      let node_major_after_upgrade = node_after_upgrade.as_deref().and_then(parse_node_major);
      let npm_after_upgrade = command_output(&path_env, "npm", &["-v"]).is_ok();
      let still_need_install =
        node_major_after_upgrade.map(|m| m < MIN_NODE_MAJOR).unwrap_or(true) || !npm_after_upgrade;

      if still_need_install {
        let install_code = run_logged_program(
          window,
          cancel,
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

  let node_version_out2 =
    command_output(&path_env, "node", &["-v"]).map_err(|_| "安装后仍未检测到 node".to_string())?;
  let node_major2 = parse_node_major(&node_version_out2).ok_or("无法解析 node 版本")?;
  if node_major2 < MIN_NODE_MAJOR {
    return Err(format!("Node.js 版本过低：{node_version_out2}（需要 >= {MIN_NODE_MAJOR}）"));
  }

  // Ensure npm exists (some Node installations might be incomplete).
  let npm_ok_final = command_output(&path_env, "npm", &["-v"]).is_ok();
  if !npm_ok_final {
    return Err("未检测到 npm。请确认 Node.js 安装完整，或重新安装 Node.js 后重试。".into());
  }

  emit_progress(window, "openclaw", "全局安装 openclaw…", 4, total);
  check_canceled(cancel)?;
  emit_log(window, "install-log", format!("[npm] registry: {npm_registry}"));
  let npm_args = [
    "install",
    "-g",
    openclaw_package.as_str(),
    "--registry",
    npm_registry.as_str(),
  ];
  let mut npm_cmd = create_command(&path_env, "npm", &npm_args)?;
  npm_cmd.env("npm_config_progress", "false");
  npm_cmd.env("npm_config_fund", "false");
  npm_cmd.env("npm_config_audit", "false");
  #[cfg(target_os = "windows")]
  {
    // OpenClaw currently declares `node-llama-cpp` as a peer dependency. On npm (v7+),
    // peers are auto-installed and `node-llama-cpp` runs a heavy postinstall that may crash
    // or hang on Windows (and often needs GitHub access / build tools).
    //
    // We skip the postinstall download/compile during installation to keep the core CLI install reliable.
    // Users who need local embeddings can rebuild/download later.
    emit_log(
      window,
      "install-log",
      "[npm] NODE_LLAMA_CPP_SKIP_DOWNLOAD=1（跳过 node-llama-cpp 安装期下载/编译，提升 Windows 安装成功率）",
    );
    npm_cmd.env("NODE_LLAMA_CPP_SKIP_DOWNLOAD", "1");
  }
  // Workaround: some npm dependencies may use GitHub SSH URLs (e.g. ssh://git@github.com/...)
  // which fails on machines without SSH keys (and can be slow/blocked on some networks).
  apply_github_git_rewrite_env(window, &mut npm_cmd, github_mirror.as_deref());
  // Provide registry via CLI args for visibility and determinism.
  let (code, output) = run_logged_capture(window, cancel, "[npm]", npm_cmd)?;
  if code != 0 {
    if output_looks_like_github_connectivity_issue(&output) && github_mirror.is_none() && !github_mirror_disabled {
      emit_log(
        window,
        "install-log",
        format!(
          "[warn] 检测到 GitHub 连接失败，尝试启用 GitHub 镜像：{DEFAULT_GITHUB_MIRROR}（仅本次安装生效）"
        ),
      );
      github_mirror = Some(DEFAULT_GITHUB_MIRROR.to_string());

      let mut npm_cmd2 = create_command(&path_env, "npm", &npm_args)?;
      npm_cmd2.env("npm_config_progress", "false");
      npm_cmd2.env("npm_config_fund", "false");
      npm_cmd2.env("npm_config_audit", "false");
      apply_github_git_rewrite_env(window, &mut npm_cmd2, github_mirror.as_deref());
      let (code2, _output2) = run_logged_capture(window, cancel, "[npm]", npm_cmd2)?;
      if code2 != 0 {
        return Err("npm install -g 失败，请查看日志。".into());
      }
    } else {
      return Err("npm install -g 失败，请查看日志。".into());
    }
  }

  emit_progress(window, "verify", "验证 openclaw 命令…", 5, total);
  check_canceled(cancel)?;
  let info = get_openclaw_info(false);
  if !info.installed {
    let err = info.error.unwrap_or_else(|| "unknown".into());
    return Err(format!("openclaw 安装完成，但无法执行 openclaw：{err}"));
  }

  let version = info.version.unwrap_or_else(|| "unknown".into());
  emit_log(window, "install-log", format!("openclaw --version => {version}"));

  if needs_onboard {
    emit_progress(window, "onboard", "自动配置 openclaw…", 6, total);
    check_canceled(cancel)?;

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
    apply_windows_no_window(&mut cmd);
    cmd.env("PATH", &resolved.path_env);
    cmd.args(["onboard", "--non-interactive", "--accept-risk"]);
    #[cfg(target_os = "windows")]
    {
      if is_windows_admin(&resolved.path_env) {
        cmd.arg("--install-daemon");
      } else {
        emit_log(
          window,
          "install-log",
          "[tip] Windows 上安装网关服务需要管理员权限；当前未以管理员运行，已跳过 daemon 安装。稍后可在“网关服务 → 启动”触发安装。",
        );
      }
    }
    #[cfg(not(target_os = "windows"))]
    {
      cmd.arg("--install-daemon");
    }
    cmd.args([
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
    emit_log(window, "install-log", format!("[openclaw] {}", redact_sensitive_args(&args_for_log)));

    let w = window.clone();
    let code = spawn_with_streaming_logs_cancelable(cmd, cancel.clone(), move |line| {
      emit_log(&w, "install-log", format!("[openclaw] {line}"));
    })?;
    if code != 0 {
      // On failure, print gateway status to help debugging common "gateway closed" cases.
      emit_log(window, "install-log", "[openclaw] onboard 失败，尝试输出 gateway status 以便排查…");
      if let Ok(status_text) = run_openclaw_collect(&resolved, &["gateway", "status", "--no-color"]) {
        for line in split_lines(&status_text).into_iter().take(60) {
          emit_log(window, "install-log", format!("[gateway] {line}"));
        }
      }
      return Err(format!("openclaw onboard 失败（退出码 {code}）"));
    }

    emit_progress(window, "config", "写入 openai-responses 配置…", 7, total);
    set_openai_api_mode_openai_responses(window, cancel, &resolved)?;

    emit_progress(window, "gateway", "重启网关服务…", 8, total);
    check_canceled(cancel)?;
    let mut restart = Command::new(&resolved.command);
    apply_windows_no_window(&mut restart);
    restart.env("PATH", &resolved.path_env);
    restart.args(["gateway", "restart"]);
    let w2 = window.clone();
    let code2 = spawn_with_streaming_logs_cancelable(restart, cancel.clone(), move |line| {
      emit_log(&w2, "install-log", format!("[openclaw] {line}"));
    })?;
    if code2 != 0 {
      #[cfg(target_os = "windows")]
      {
        emit_log(
          window,
          "install-log",
          format!(
            "[warn] 网关重启失败（退出码 {code2}）。Windows 上安装/启动网关服务可能需要管理员权限。\n你可以稍后在管理员 PowerShell 运行：openclaw gateway install && openclaw gateway start"
          ),
        );
      }
      #[cfg(not(target_os = "windows"))]
      {
        return Err(format!("网关重启失败（退出码 {code2}）。你可以手动运行：openclaw gateway restart"));
      }
    }
  } else {
    emit_log(window, "install-log", "跳过自动配置：未提供 CUSTOM_API_KEY。");
  }

  let _ = window.emit(
    "install-progress",
    ProgressPayload {
      stage: "done".into(),
      title: "完成".into(),
      index: total,
      total,
      percent: 1.0,
    },
  );
  Ok(())
}

#[tauri::command]
pub async fn start_install(window: Window, state: tauri::State<'_, TaskState>, options: InstallOptions) -> Result<(), String> {
  let cancel = state.start()?;
  let w2 = window.clone();
  let cancel2 = cancel.clone();
  let options2 = options.clone();

  let join = tauri::async_runtime::spawn_blocking(move || start_install_blocking(&w2, &cancel2, options2));
  let result = match join.await {
    Ok(r) => r,
    Err(e) => Err(format!("内部错误：任务线程异常：{e}")),
  };
  state.finish();
  result
}

#[tauri::command]
pub async fn cancel_task(state: tauri::State<'_, TaskState>) -> Result<(), String> {
  state.cancel();
  Ok(())
}
