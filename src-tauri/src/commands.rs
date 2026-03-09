use crate::openclaw::{
  cleanup_mac_nvm_openclaw, get_openclaw_info, parse_gateway_status, resolve_openclaw,
  spawn_with_streaming_logs, GatewayStatus, OpenclawInfo,
};
use serde::Serialize;
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

fn emit_log(window: &Window, event: &str, message: impl Into<String>) {
  let payload = LogPayload { message: message.into(), ts: now_ms() };
  let _ = window.emit(event, payload);
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
pub async fn run_openclaw(window: Window, args: Vec<String>) -> Result<(), String> {
  if args.is_empty() {
    return Err("缺少 openclaw 参数".into());
  }
  let resolved = resolve_openclaw().ok_or("未检测到 openclaw，请先完成安装。")?;

  emit_log(&window, "openclaw-log", format!("openclaw {} ({})", args.join(" "), resolved.source));

  let mut cmd = Command::new(&resolved.command);
  cmd.env("PATH", &resolved.path_env);
  cmd.arg("--no-color");
  for a in args {
    cmd.arg(a);
  }

  let w = window.clone();
  let code = spawn_with_streaming_logs(cmd, move |line| emit_log(&w, "openclaw-log", line))?;
  if code == 0 {
    Ok(())
  } else {
    Err(format!("openclaw 退出码：{code}"))
  }
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
pub async fn uninstall_openclaw(window: Window) -> Result<(), String> {
  let resolved = resolve_openclaw().ok_or("未检测到 openclaw，请先完成安装。")?;
  let openclaw_cmd = resolved.command.clone();

  emit_log(
    &window,
    "openclaw-log",
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
    let code = spawn_with_streaming_logs(cmd, move |line| emit_log(&w, "openclaw-log", line))?;
    if code != 0 {
      return Err(format!("openclaw uninstall 失败（退出码 {code}）"));
    }
  }

  // 2) Best-effort remove CLI from common managers.
  emit_log(&window, "openclaw-log", "正在尝试移除 openclaw CLI（brew / npm / pnpm / nvm）…");

  #[cfg(target_os = "macos")]
  {
    let brew_candidates = ["/opt/homebrew/bin/brew", "/usr/local/bin/brew", "brew"];
    for brew in brew_candidates {
      let mut cmd = Command::new(brew);
      cmd.env("PATH", &resolved.path_env);
      cmd.args(["uninstall", "openclaw"]);
      let _ = cmd.output();
    }
  }

  // npm uninstall -g openclaw (best-effort)
  {
    let mut cmd = Command::new("npm");
    cmd.env("PATH", &resolved.path_env);
    cmd.args(["uninstall", "-g", "openclaw"]);
    let _ = cmd.output();
  }

  // pnpm remove -g openclaw (best-effort)
  {
    let mut cmd = Command::new("pnpm");
    cmd.env("PATH", &resolved.path_env);
    cmd.args(["remove", "-g", "openclaw"]);
    let _ = cmd.output();
  }

  // nvm scan cleanup (delete ~/.nvm/... copy if that's where we resolved from)
  {
    let removed = cleanup_mac_nvm_openclaw(&openclaw_cmd, "openclaw").unwrap_or_default();
    if !removed.is_empty() {
      emit_log(&window, "openclaw-log", format!("[cleanup] nvm: removed {} item(s)", removed.len()));
      for item in removed {
        emit_log(&window, "openclaw-log", format!("[cleanup] nvm: {item}"));
      }
    }
  }

  emit_log(&window, "openclaw-log", "卸载完成。");
  Ok(())
}

#[tauri::command]
pub async fn start_install(window: Window, options: serde_json::Value) -> Result<(), String> {
  let _ = options;
  emit_log(&window, "install-log", "Tauri 版本安装流程尚未迁移完成。");
  Err("暂不支持：Tauri 版本尚未实现自动安装流程。".into())
}

#[tauri::command]
pub async fn cancel_task() -> Result<(), String> {
  Ok(())
}
