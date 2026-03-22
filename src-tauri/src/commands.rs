use crate::openclaw::{
    apply_windows_no_window, capture_command_output_cancelable, cleanup_all_mac_nvm_openclaw,
    cleanup_mac_nvm_openclaw, create_base_path_env, get_openclaw_info, parse_gateway_status,
    resolve_command_in_path, resolve_openclaw, spawn_with_streaming_logs_cancelable,
    CapturedCommandOutput, GatewayStatus, OpenclawInfo,
};
use serde::Deserialize;
use serde::Serialize;
#[cfg(target_os = "windows")]
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
#[cfg(target_os = "windows")]
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
#[cfg(target_os = "windows")]
use std::sync::OnceLock;
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

#[derive(Debug, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatus {
    running: bool,
    cancel_requested: bool,
    kind: Option<String>,
    title: Option<String>,
    can_cancel: bool,
    cancel_warning: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct TaskMeta {
    kind: String,
    title: String,
    can_cancel: bool,
    cancel_warning: Option<String>,
}

impl TaskMeta {
    fn new(kind: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            title: title.into(),
            can_cancel: true,
            cancel_warning: None,
        }
    }

    fn non_cancelable(
        kind: impl Into<String>,
        title: impl Into<String>,
        cancel_warning: impl Into<String>,
    ) -> Self {
        Self {
            kind: kind.into(),
            title: title.into(),
            can_cancel: false,
            cancel_warning: Some(cancel_warning.into()),
        }
    }
}

fn task_meta_for_openclaw_args(args: &[String]) -> TaskMeta {
    let command = args.first().map(String::as_str).unwrap_or_default();
    match command {
        "update" => TaskMeta::non_cancelable(
            "update",
            "更新 OpenClaw…",
            "更新进行中，暂不支持取消。中断更新可能导致 OpenClaw 暂时不可用，请等待完成。",
        ),
        "doctor" => TaskMeta::new("doctor", "健康检查/修复…"),
        "gateway" => match args.get(1).map(String::as_str) {
            Some("install") => TaskMeta::new("gateway_install", "安装网关服务…"),
            Some("start") => TaskMeta::new("gateway_start", "启动网关服务…"),
            Some("stop") => TaskMeta::new("gateway_stop", "停止网关服务…"),
            Some("restart") => TaskMeta::new("gateway_restart", "重启网关服务…"),
            _ => TaskMeta::new("gateway", "执行网关操作…"),
        },
        "config" => TaskMeta::new("config", "保存配置…"),
        _ => TaskMeta::new("openclaw_command", "执行中…"),
    }
}

fn emit_log(window: &Window, event: &str, message: impl Into<String>) {
    let payload = LogPayload {
        message: message.into(),
        ts: now_ms(),
    };
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
            args.iter()
                .map(|a| a.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        );
        format!(
      "$ErrorActionPreference='Stop';\n\
       $command='{cmdline}';\n\
       # Show a visible window for elevated install steps (UAC-required), so users can see progress/errors.\n\
       $p=Start-Process -FilePath 'cmd.exe' -ArgumentList @('/d','/s','/c',$command) -Verb RunAs -PassThru -Wait -WindowStyle Normal;\n\
       exit $p.ExitCode\n"
    )
    } else {
        format!(
      "$ErrorActionPreference='Stop';\n\
       # Show a visible window for elevated install steps (UAC-required), so users can see progress/errors.\n\
       $p=Start-Process -FilePath '{openclaw_path_escaped}' -ArgumentList @({arg_list_ps}) -Verb RunAs -PassThru -Wait -WindowStyle Normal;\n\
       exit $p.ExitCode\n"
    )
    };

    let mut ps = Command::new("powershell");
    apply_windows_no_window(&mut ps);
    ps.env("PATH", &resolved.path_env);
    ps.args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        &script,
    ]);

    run_logged(window, cancel, "[powershell]", ps)
}

#[cfg(target_os = "windows")]
static WINDOWS_DIRECT_GATEWAY_CHILD: OnceLock<Mutex<Option<std::process::Child>>> = OnceLock::new();

#[cfg(target_os = "windows")]
#[derive(Debug, Deserialize, Serialize)]
struct WindowsDirectGatewayRecord {
    pid: u32,
    #[serde(rename = "startTimeUtc")]
    start_time_utc: String,
}

#[cfg(target_os = "windows")]
fn windows_direct_gateway_child_state() -> &'static Mutex<Option<std::process::Child>> {
    WINDOWS_DIRECT_GATEWAY_CHILD.get_or_init(|| Mutex::new(None))
}

#[cfg(target_os = "windows")]
fn windows_direct_gateway_record_path() -> PathBuf {
    let base_dir = dirs::data_local_dir()
        .or_else(crate::openclaw::home_dir)
        .unwrap_or_else(std::env::temp_dir);
    base_dir
        .join("openclaw-helper")
        .join("windows-direct-gateway.json")
}

#[cfg(target_os = "windows")]
fn clear_windows_direct_gateway_record() {
    let path = windows_direct_gateway_record_path();
    let _ = fs::remove_file(path);
}

#[cfg(target_os = "windows")]
fn read_windows_direct_gateway_record() -> Option<WindowsDirectGatewayRecord> {
    let path = windows_direct_gateway_record_path();
    let raw = fs::read_to_string(&path).ok()?;
    match serde_json::from_str::<WindowsDirectGatewayRecord>(&raw) {
        Ok(record) => Some(record),
        Err(_) => {
            let _ = fs::remove_file(path);
            None
        }
    }
}

#[cfg(target_os = "windows")]
fn windows_process_start_time_utc_once(pid: u32) -> Option<String> {
    let mut cmd = Command::new("powershell");
    apply_windows_no_window(&mut cmd);
    cmd.args([
    "-NoProfile",
    "-ExecutionPolicy",
    "Bypass",
    "-Command",
    &format!(
      "$p=Get-Process -Id {pid} -ErrorAction SilentlyContinue; if ($p) {{ [Console]::Out.Write($p.StartTime.ToUniversalTime().ToString('o')) }}"
    ),
  ]);
    let output = cmd.output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

#[cfg(target_os = "windows")]
fn windows_process_start_time_utc(pid: u32) -> Option<String> {
    for attempt in 0..10 {
        if let Some(value) = windows_process_start_time_utc_once(pid) {
            return Some(value);
        }
        if attempt < 9 {
            std::thread::sleep(std::time::Duration::from_millis(80));
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn persist_windows_direct_gateway_record(pid: u32) -> Result<(), String> {
    let start_time_utc = windows_process_start_time_utc(pid)
        .ok_or_else(|| format!("无法获取 direct gateway 进程启动时间（pid={pid}）"))?;
    let path = windows_direct_gateway_record_path();
    let value = serde_json::json!({
      "pid": pid,
      "startTimeUtc": start_time_utc,
    });
    atomic_write_json(&path, &value)
}

#[cfg(target_os = "windows")]
fn tracked_windows_direct_gateway_pid() -> Option<u32> {
    let record = read_windows_direct_gateway_record()?;
    match windows_process_start_time_utc_once(record.pid) {
        Some(actual_start_time) if actual_start_time == record.start_time_utc => Some(record.pid),
        _ => {
            clear_windows_direct_gateway_record();
            None
        }
    }
}

#[cfg(target_os = "windows")]
fn windows_taskkill_tree_best_effort(pid: u32) {
    let mut cmd = Command::new("taskkill");
    apply_windows_no_window(&mut cmd);
    let _ = cmd
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(target_os = "windows")]
fn windows_extract_port_from_url(url: &str) -> Option<u16> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }

    let after_scheme = trimmed.split("://").nth(1).unwrap_or(trimmed);
    let host_port = after_scheme.split('/').next().unwrap_or(after_scheme);
    if host_port.is_empty() {
        return None;
    }

    if host_port.starts_with('[') {
        let end = host_port.find(']')?;
        let rest = host_port.get(end + 1..)?;
        let port_part = rest.strip_prefix(':')?;
        return port_part.parse::<u16>().ok();
    }

    let (_host, port_part) = host_port.rsplit_once(':')?;
    port_part.parse::<u16>().ok()
}

#[cfg(target_os = "windows")]
fn windows_gateway_status_text(resolved: &crate::openclaw::ResolvedOpenclaw) -> Option<String> {
    let mut cmd = Command::new(&resolved.command);
    apply_windows_no_window(&mut cmd);
    cmd.env("PATH", &resolved.path_env);
    cmd.args(["--no-color", "gateway", "status"]);
    let out = cmd.output().ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    Some(format!("{stdout}\n{stderr}"))
}

#[cfg(target_os = "windows")]
fn windows_find_listening_pids_by_port(port: u16) -> Vec<u32> {
    let script = format!(
        r#"$ErrorActionPreference='SilentlyContinue'
$port={port}
$pids=@()
if (Get-Command Get-NetTCPConnection -ErrorAction SilentlyContinue) {{
  $pids = Get-NetTCPConnection -LocalPort $port -ErrorAction SilentlyContinue |
    Where-Object {{ $_.State -eq 'Listen' -or $_.State -eq 2 }} |
    Select-Object -ExpandProperty OwningProcess -Unique
}}
if (-not $pids -or $pids.Count -eq 0) {{
  $lines = netstat -ano -p tcp | Select-String -Pattern (':'+$port+'\s+.*LISTENING\s+(\d+)$')
  $pids = $lines | ForEach-Object {{
    $m = [regex]::Match($_.Line, '\s+(\d+)\s*$')
    if ($m.Success) {{ [int]$m.Groups[1].Value }}
  }} | Select-Object -Unique
}}
$pids | ForEach-Object {{ [Console]::Out.WriteLine($_) }}
"#
    );

    let mut cmd = Command::new("powershell");
    apply_windows_no_window(&mut cmd);
    cmd.args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        &script,
    ]);
    let Ok(out) = cmd.output() else {
        return vec![];
    };

    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let mut pids: Vec<u32> = Vec::new();
    for line in combined.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(pid) = trimmed.parse::<u32>() {
            if pid > 0 && !pids.contains(&pid) {
                pids.push(pid);
            }
        }
    }

    pids
}

#[cfg(target_os = "windows")]
fn windows_gateway_start_direct(
    window: &Window,
    cancel: &Arc<AtomicBool>,
    resolved: &crate::openclaw::ResolvedOpenclaw,
) -> Result<(), String> {
    check_canceled(cancel)?;

    let state = windows_direct_gateway_child_state();
    let mut slot = state.lock().map_err(|_| "内部错误：锁失败")?;

    // If we already started a direct gateway and it's still running, treat as OK.
    if let Some(child) = slot.as_mut() {
        match child.try_wait() {
            Ok(None) => {
                emit_log(window, "openclaw-log", "[gateway] 已在运行（direct）");
                return Ok(());
            }
            Ok(Some(_)) => {
                *slot = None;
            }
            Err(_) => {
                *slot = None;
            }
        }
    }

    if let Some(pid) = tracked_windows_direct_gateway_pid() {
        emit_log(
            window,
            "openclaw-log",
            format!("[gateway] 已在运行（direct, pid={pid}）"),
        );
        return Ok(());
    }

    // Direct mode: run `openclaw gateway` (no Scheduled Task / no admin required).
    emit_log(
        window,
        "openclaw-log",
        "[windows] 使用 direct 模式启动网关（不依赖 Scheduled Task / 无需管理员）",
    );

    let mut cmd = Command::new(&resolved.command);
    apply_windows_no_window(&mut cmd);
    cmd.env("PATH", &resolved.path_env);
    cmd.args(["--no-color", "gateway"]);
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());

    emit_log(
        window,
        "openclaw-log",
        format!("[gateway] {}", format_command_for_log(&cmd)),
    );

    let child = cmd.spawn().map_err(|e| e.to_string())?;
    emit_log(
        window,
        "openclaw-log",
        format!("[gateway] direct started (pid={})", child.id()),
    );
    if let Err(err) = persist_windows_direct_gateway_record(child.id()) {
        emit_log(
            window,
            "openclaw-log",
            format!("[warn] 记录 direct gateway 进程失败：{err}"),
        );
    }
    *slot = Some(child);
    Ok(())
}

#[cfg(target_os = "windows")]
fn windows_gateway_stop_direct_best_effort(
    window: &Window,
    resolved: &crate::openclaw::ResolvedOpenclaw,
) -> Result<bool, String> {
    let state = windows_direct_gateway_child_state();
    let mut slot = state.lock().map_err(|_| "内部错误：锁失败")?;
    if let Some(mut child) = slot.take() {
        let pid = child.id();
        emit_log(
            window,
            "openclaw-log",
            format!("[gateway] 停止 direct 进程（pid={pid}）…"),
        );
        windows_taskkill_tree_best_effort(pid);
        let _ = child.wait();
        clear_windows_direct_gateway_record();
        emit_log(window, "openclaw-log", "[gateway] direct stopped");
        return Ok(true);
    }
    drop(slot);

    let Some(pid) = tracked_windows_direct_gateway_pid() else {
        clear_windows_direct_gateway_record();
        let status_text = windows_gateway_status_text(resolved).unwrap_or_default();
        let status = parse_gateway_status(&status_text);
        let Some(dashboard_url) = status.dashboard_url.as_deref() else {
            emit_log(
                window,
                "openclaw-log",
                "[gateway] 未从 gateway status 解析到 Dashboard 端口，跳过端口强杀。",
            );
            return Ok(false);
        };
        let Some(port) = windows_extract_port_from_url(dashboard_url) else {
            emit_log(
                window,
                "openclaw-log",
                format!("[gateway] Dashboard 地址无法解析端口：{dashboard_url}"),
            );
            return Ok(false);
        };

        emit_log(
            window,
            "openclaw-log",
            format!("[gateway] 未找到 tracked pid，尝试按端口强制停止（port={port}）…"),
        );

        let current_pid = std::process::id();
        let mut pids = windows_find_listening_pids_by_port(port);
        pids.retain(|candidate| *candidate != current_pid);

        if pids.is_empty() {
            emit_log(
                window,
                "openclaw-log",
                format!("[gateway] 端口 {port} 未发现监听进程，停止完成。"),
            );
            return Ok(false);
        }

        for target_pid in &pids {
            emit_log(
                window,
                "openclaw-log",
                format!("[gateway] 停止端口 {port} 对应进程（pid={target_pid}）…"),
            );
            windows_taskkill_tree_best_effort(*target_pid);
        }

        emit_log(
            window,
            "openclaw-log",
            format!(
                "[gateway] 已按端口停止 {} 个进程（port={port}）。",
                pids.len()
            ),
        );
        return Ok(true);
    };

    emit_log(
        window,
        "openclaw-log",
        format!("[gateway] 停止 direct 进程（pid={pid}）…"),
    );
    windows_taskkill_tree_best_effort(pid);
    clear_windows_direct_gateway_record();
    emit_log(window, "openclaw-log", "[gateway] direct stopped");
    Ok(true)
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
    meta: Mutex<Option<TaskMeta>>,
}

impl TaskState {
    fn start_with_meta(&self, meta: TaskMeta) -> Result<Arc<AtomicBool>, String> {
        let mut running = self.running.lock().map_err(|_| "内部错误：锁失败")?;
        if *running {
            return Err("已有任务正在运行，请先取消或等待完成。".into());
        }
        *running = true;
        let flag = Arc::new(AtomicBool::new(false));
        let mut slot = self.cancel_flag.lock().map_err(|_| "内部错误：锁失败")?;
        *slot = Some(flag.clone());
        let mut task_meta = self.meta.lock().map_err(|_| "内部错误：锁失败")?;
        *task_meta = Some(meta);
        Ok(flag)
    }

    fn finish(&self) {
        if let Ok(mut running) = self.running.lock() {
            *running = false;
        }
        if let Ok(mut slot) = self.cancel_flag.lock() {
            *slot = None;
        }
        if let Ok(mut task_meta) = self.meta.lock() {
            *task_meta = None;
        }
    }

    fn cancel(&self) -> Result<(), String> {
        let task_meta = self.meta.lock().map_err(|_| "内部错误：锁失败")?.clone();
        if let Some(meta) = task_meta {
            if !meta.can_cancel {
                return Err(meta
                    .cancel_warning
                    .unwrap_or_else(|| "当前任务进行中，暂不支持取消。".into()));
            }
        }
        if let Some(flag) = self
            .cancel_flag
            .lock()
            .map_err(|_| "内部错误：锁失败")?
            .as_ref()
        {
            flag.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    fn snapshot(&self) -> Result<TaskStatus, String> {
        let running = *self.running.lock().map_err(|_| "内部错误：锁失败")?;
        let cancel_requested = self
            .cancel_flag
            .lock()
            .map_err(|_| "内部错误：锁失败")?
            .as_ref()
            .map(|flag| flag.load(Ordering::SeqCst))
            .unwrap_or(false);
        let task_meta = self.meta.lock().map_err(|_| "内部错误：锁失败")?.clone();
        let (kind, title, can_cancel, cancel_warning) = if let Some(meta) = task_meta {
            (
                Some(meta.kind),
                Some(meta.title),
                meta.can_cancel,
                meta.cancel_warning,
            )
        } else {
            (None, None, false, None)
        };
        Ok(TaskStatus {
            running,
            cancel_requested,
            kind,
            title,
            can_cancel,
            cancel_warning,
        })
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
    #[allow(unused_mut)]
    let mut status = match cmd.output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            parse_gateway_status(&format!("{stdout}\n{stderr}"))
        }
        Err(err) => parse_gateway_status(&format!("gateway status failed: {err}")),
    };

    #[cfg(target_os = "windows")]
    {
        if let Some(pid) = tracked_windows_direct_gateway_pid() {
            if status.state != "running" {
                if !status.raw.is_empty() {
                    status.raw.push('\n');
                }
                status
                    .raw
                    .push_str(&format!("[helper] tracked direct gateway pid={pid}"));
                status.state = "running".into();
            }
        }
    }

    status
}

#[tauri::command]
pub async fn run_openclaw(
    window: Window,
    state: tauri::State<'_, TaskState>,
    args: Vec<String>,
) -> Result<(), String> {
    if args.is_empty() {
        return Err("缺少 openclaw 参数".into());
    }
    let cancel = state.start_with_meta(task_meta_for_openclaw_args(&args))?;

    let w2 = window.clone();
    let cancel2 = cancel.clone();

    // Run blocking process execution on a dedicated thread so the async runtime can keep
    // processing other invocations (like cancel_task).
    let join = tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let resolved = resolve_openclaw().ok_or("未检测到 openclaw，请先完成安装。")?;

        emit_log(
            &w2,
            "openclaw-log",
            format!("openclaw {} ({})", args.join(" "), resolved.source),
        );

        // Windows: installing the gateway service needs admin privileges (schtasks create).
        // If user isn't elevated, run `openclaw gateway install` via UAC so "Start" works in-app.
        #[cfg(target_os = "windows")]
        {
            let is_gateway_install =
                args.len() >= 2 && args[0] == "gateway" && args[1] == "install";
            if is_gateway_install && !is_windows_admin(&resolved.path_env) {
                emit_log(
          &w2,
          "openclaw-log",
          "[tip] 安装网关服务需要管理员权限，将弹出 UAC 授权窗口，并打开管理员终端窗口执行安装（这是 Windows 限制）。",
        );
                let code = run_openclaw_elevated(
                    &w2,
                    &cancel2,
                    &resolved,
                    &["--no-color", "gateway", "install"],
                )?;
                if code == 0 {
                    emit_log(&w2, "openclaw-log", "[gateway] install: ok");
                    return Ok(());
                }
                return Err(format!("gateway install 失败（退出码 {code}）"));
            }

            // Windows: start/stop the gateway in direct mode by default (no Scheduled Task).
            let is_gateway_start = args.len() == 2 && args[0] == "gateway" && args[1] == "start";
            if is_gateway_start {
                windows_gateway_start_direct(&w2, &cancel2, &resolved)?;
                return Ok(());
            }

            let is_gateway_stop = args.len() == 2 && args[0] == "gateway" && args[1] == "stop";
            if is_gateway_stop {
                if !windows_gateway_stop_direct_best_effort(&w2, &resolved)? {
                    emit_log(
                        &w2,
                        "openclaw-log",
                        "[gateway] 未发现可停止的网关监听进程（已跳过 Scheduled Task 停止）。",
                    );
                }
                return Ok(());
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
        let code = spawn_with_streaming_logs_cancelable(cmd, cancel2, move |line| {
            emit_log(&w3, "openclaw-log", line)
        })?;
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
pub async fn exec_openclaw_collect(args: Vec<String>) -> Result<String, String> {
    let resolved = resolve_openclaw().ok_or("未检测到 openclaw")?;
    let s_args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    run_openclaw_collect(&resolved, &s_args)
}

#[tauri::command]
pub async fn run_weixin_config(
    window: Window,
    state: tauri::State<'_, TaskState>,
) -> Result<(), String> {
    let cancel = state.start_with_meta(TaskMeta::new("weixin", "配置微信接入…"))?;
    let w2 = window.clone();
    let cancel2 = cancel.clone();

    let join = tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let resolved = resolve_openclaw();
        let path_env = resolved
            .as_ref()
            .map(|r| r.path_env.clone())
            .unwrap_or_else(create_base_path_env);

        emit_log(&w2, "install-log", "[weixin] 开始配置微信接入...");
        emit_log(
            &w2,
            "install-log",
            ">> npx -y @tencent-weixin/openclaw-weixin-cli@latest install",
        );

        let npx_cmd = if cfg!(target_os = "windows") {
            "npx.cmd"
        } else {
            "npx"
        };

        let mut cmd = match create_command(
            &path_env,
            npx_cmd,
            &[
                "-y",
                "@tencent-weixin/openclaw-weixin-cli@latest",
                "install",
            ],
        ) {
            Ok(c) => c,
            Err(_) => {
                // Fallback to searching without path_env if create_command fails (unlikely, but safe)
                Command::new(npx_cmd)
            }
        };

        // Fallback setup args in case create_command failed and we created a fresh Command
        if cmd.get_program() == npx_cmd {
            cmd.args([
                "-y",
                "@tencent-weixin/openclaw-weixin-cli@latest",
                "install",
            ]);
            apply_windows_no_window(&mut cmd);
            cmd.env("PATH", &path_env);
        }

        cmd.env("npm_config_registry", "https://registry.npmmirror.com");

        let w3 = w2.clone();
        let cancel3 = cancel2.clone();
        let code = match spawn_with_streaming_logs_cancelable(cmd, cancel3, move |line| {
            emit_log(&w3, "install-log", line);
        }) {
            Ok(c) => c,
            Err(e) => {
                return Err(format!(
                    "运行 npx 失败：请确认 Node.js 环境已安装。\n详情：{e}"
                ))
            }
        };

        if code == 0 {
            emit_log(&w2, "install-log", "微信接入配置已退出。");
            Ok(())
        } else {
            Err(format!("执行结束（退出码 {code}）"))
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
pub async fn uninstall_openclaw(
    window: Window,
    state: tauri::State<'_, TaskState>,
) -> Result<(), String> {
    let cancel = state.start_with_meta(TaskMeta::non_cancelable(
        "uninstall",
        "卸载 OpenClaw…",
        "卸载进行中，暂不支持取消，以免系统状态停在半清理状态。请等待完成。",
    ))?;
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
            let code = spawn_with_streaming_logs_cancelable(cmd, cancel2.clone(), move |line| {
                emit_log(&w, "install-log", line)
            })?;
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
                        emit_log(
                            &w2,
                            "install-log",
                            format!("[cleanup] {label} failed (ignored): {err}"),
                        );
                    }
                },
                Err(err) => {
                    emit_log(
                        &w2,
                        "install-log",
                        format!("[cleanup] {label} skipped: {err}"),
                    );
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
                let openclaw_dir = openclaw_cmd
                    .as_ref()
                    .and_then(|c| c.parent().map(|p| p.to_path_buf()));
                let npm_next_to_openclaw = openclaw_dir
                    .map(|d| d.join("npm.cmd"))
                    .filter(|p| p.is_file());
                if let Some(npm_cmd) = npm_next_to_openclaw {
                    let npm_cmd = npm_cmd.to_string_lossy().to_string();
                    emit_log(
                        &w2,
                        "install-log",
                        format!("[cleanup] using npm: {npm_cmd}"),
                    );
                    run_best_effort(
                        "[npm]",
                        &npm_cmd,
                        &["uninstall", "-g", OPENCLAW_NPM_PACKAGE],
                    )?;
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
                emit_log(
                    &w2,
                    "install-log",
                    format!("[cleanup] nvm: removed {} item(s)", removed.len()),
                );
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
                            format!(
                                "[cleanup] failed to remove {}: {}",
                                state_dir.to_string_lossy(),
                                err
                            ),
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
                        emit_log(
                            &w2,
                            "install-log",
                            format!("[cleanup] retry npm: {npm_local}"),
                        );
                        run_best_effort(
                            "[npm]",
                            &npm_local,
                            &["uninstall", "-g", OPENCLAW_NPM_PACKAGE],
                        )?;
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
                                    format!(
                                        "[cleanup] failed to remove shim {}: {}",
                                        target.to_string_lossy(),
                                        err
                                    ),
                                );
                            }
                        }
                    }
                    if !removed.is_empty() {
                        emit_log(
                            &w2,
                            "install-log",
                            format!("[cleanup] removed {} shim(s)", removed.len()),
                        );
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
                                format!(
                                    "[cleanup] removed module dir: {}",
                                    module_dir.to_string_lossy()
                                ),
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
    matches!(
        lower.as_str(),
        "off" | "false" | "none" | "direct" | "disable" | "disabled" | "0"
    )
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

fn run_logged_capture(
    window: &Window,
    cancel: &Arc<AtomicBool>,
    label: &str,
    cmd: Command,
) -> Result<(i32, String), String> {
    check_canceled(cancel)?;
    emit_log(
        window,
        "install-log",
        format!("{label} {}", format_command_for_log(&cmd)),
    );

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
            format!(
                "已启用 GitHub 镜像：{mirror}（用于加速/绕过 GitHub 访问问题；仅本次安装生效）"
            ),
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
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '/' | '.' | '_' | '-'));
    if !ok
        || name.contains("..")
        || name.contains("//")
        || name.contains('\\')
        || name.contains('\'')
        || name.contains('\"')
        || name.contains('`')
    {
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
    text.replace("\r\n", "\n")
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

fn run_logged(
    window: &Window,
    cancel: &Arc<AtomicBool>,
    label: &str,
    cmd: Command,
) -> Result<i32, String> {
    check_canceled(cancel)?;
    emit_log(
        window,
        "install-log",
        format!("{label} {}", format_command_for_log(&cmd)),
    );
    let w = window.clone();
    let prefix = label.to_string();
    let cancel2 = cancel.clone();
    spawn_with_streaming_logs_cancelable(cmd, cancel2, move |line| {
        emit_log(&w, "install-log", format!("{prefix} {line}"))
    })
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

fn command_output_cancelable(
    path_env: &str,
    program: &str,
    args: &[&str],
    cancel: &Arc<AtomicBool>,
) -> Result<String, String> {
    check_canceled(cancel)?;
    let cmd = create_command(path_env, program, args)?;
    let out = capture_command_output_cancelable(cmd, cancel.clone())?;
    Ok(format!("{}\n{}", out.stdout, out.stderr))
}

#[cfg(target_os = "windows")]
fn ensure_winget(path_env: &str) -> Result<(), String> {
    if command_output(path_env, "winget", &["--version"]).is_ok() {
        Ok(())
    } else {
        Err(
            "未检测到 winget。请先安装 Microsoft App Installer（Windows 软件包管理器）后重试。"
                .into(),
        )
    }
}

#[cfg(target_os = "windows")]
fn ensure_winget_cancelable(path_env: &str, cancel: &Arc<AtomicBool>) -> Result<(), String> {
    if command_output_cancelable(path_env, "winget", &["--version"], cancel).is_ok() {
        Ok(())
    } else {
        Err(
            "未检测到 winget。请先安装 Microsoft App Installer（Windows 软件包管理器）后重试。"
                .into(),
        )
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

const OPENAI_RESPONSES_CONTEXT_WINDOW: u64 = 320_000;
const OPENAI_RESPONSES_MAX_TOKENS: u64 = 81_920;

fn parse_primary_model_ref_from_config_value(
    value: &serde_json::Value,
) -> Option<(String, String)> {
    let primary = match value {
        serde_json::Value::String(s) => Some(s.as_str()),
        serde_json::Value::Object(obj) => obj.get("primary").and_then(|v| v.as_str()),
        _ => None,
    }?;
    let trimmed = primary.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut parts = trimmed.splitn(2, '/');
    let provider = parts.next()?.trim();
    let model_id = parts.next()?.trim();
    if provider.is_empty() || model_id.is_empty() {
        return None;
    }
    Some((provider.to_string(), model_id.to_string()))
}

fn openclaw_command_error(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stdout.is_empty() {
        return stdout;
    }

    format!("退出码 {:?}", output.status.code())
}

fn openclaw_command_error_from_captured(output: &CapturedCommandOutput) -> String {
    let stderr = output.stderr.trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }

    let stdout = output.stdout.trim().to_string();
    if !stdout.is_empty() {
        return stdout;
    }

    format!("退出码 {}", output.code)
}

fn run_openclaw_config_get_json(
    resolved: &crate::openclaw::ResolvedOpenclaw,
    path: &str,
) -> Result<serde_json::Value, String> {
    let mut cmd = Command::new(&resolved.command);
    apply_windows_no_window(&mut cmd);
    cmd.env("PATH", &resolved.path_env);
    cmd.args(["config", "get", path, "--json"]);
    let out = cmd.output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "读取配置 {path} 失败：{}",
            openclaw_command_error(&out)
        ));
    }

    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    serde_json::from_str(&stdout).map_err(|e| format!("解析配置 {path} 失败：{e}"))
}

fn run_openclaw_config_get_json_cancelable(
    resolved: &crate::openclaw::ResolvedOpenclaw,
    cancel: &Arc<AtomicBool>,
    path: &str,
) -> Result<serde_json::Value, String> {
    check_canceled(cancel)?;
    let mut cmd = Command::new(&resolved.command);
    apply_windows_no_window(&mut cmd);
    cmd.env("PATH", &resolved.path_env);
    cmd.args(["config", "get", path, "--json"]);
    let out = capture_command_output_cancelable(cmd, cancel.clone())?;
    if out.code != 0 {
        return Err(format!(
            "读取配置 {path} 失败：{}",
            openclaw_command_error_from_captured(&out)
        ));
    }

    let stdout = out.stdout.trim().to_string();
    serde_json::from_str(&stdout).map_err(|e| format!("解析配置 {path} 失败：{e}"))
}

fn read_json_file(path: &Path) -> Result<serde_json::Value, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("读取 {} 失败：{e}", path.to_string_lossy()))?;
    serde_json::from_str(&raw).map_err(|e| format!("解析 {} 失败：{e}", path.to_string_lossy()))
}

fn resolve_openclaw_config_file_path(
    resolved: &crate::openclaw::ResolvedOpenclaw,
) -> Result<PathBuf, String> {
    let mut cmd = Command::new(&resolved.command);
    apply_windows_no_window(&mut cmd);
    cmd.env("PATH", &resolved.path_env);
    cmd.args(["config", "file"]);
    let out = cmd.output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "读取配置文件路径失败：{}",
            openclaw_command_error(&out)
        ));
    }

    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if stdout.is_empty() {
        return Err("读取配置文件路径失败：openclaw 未返回路径。".into());
    }

    Ok(expand_home_path(&stdout))
}

fn default_openclaw_config_file_path() -> PathBuf {
    crate::openclaw::home_dir()
        .map(|home| home.join(".openclaw").join("openclaw.json"))
        .unwrap_or_else(|| PathBuf::from(".openclaw").join("openclaw.json"))
}

fn load_openclaw_config_root(
    resolved: &crate::openclaw::ResolvedOpenclaw,
) -> Result<serde_json::Value, String> {
    let default_path = default_openclaw_config_file_path();
    let mut first_error: Option<String> = None;

    if default_path.is_file() {
        match read_json_file(&default_path) {
            Ok(root) => return Ok(root),
            Err(error) => first_error = Some(error),
        }
    }

    let config_path = resolve_openclaw_config_file_path(resolved)
        .map_err(|error| first_error.clone().unwrap_or(error))?;
    if config_path == default_path {
        return Err(
            first_error.unwrap_or_else(|| format!("读取 {} 失败。", config_path.to_string_lossy()))
        );
    }

    read_json_file(&config_path).map_err(|error| match first_error {
        Some(first) => format!("{first}；{error}"),
        None => error,
    })
}

fn get_json_value_at_path(root: &serde_json::Value, path: &[&str]) -> Option<serde_json::Value> {
    let mut current = root;
    for segment in path {
        current = current.as_object()?.get(*segment)?;
    }
    Some(current.clone())
}

fn run_openclaw_config_set(
    window: &Window,
    cancel: &Arc<AtomicBool>,
    resolved: &crate::openclaw::ResolvedOpenclaw,
    path: &str,
    value: &str,
    strict_json: bool,
) -> Result<(), String> {
    check_canceled(cancel)?;
    emit_log(
        window,
        "install-log",
        format!("[config] set {path} = {value}"),
    );

    let mut cmd = Command::new(&resolved.command);
    apply_windows_no_window(&mut cmd);
    cmd.env("PATH", &resolved.path_env);
    cmd.arg("config").arg("set");
    if strict_json {
        cmd.arg("--strict-json");
    }
    cmd.arg(path).arg(value);

    let out = capture_command_output_cancelable(cmd, cancel.clone())?;
    if out.code != 0 {
        return Err(format!(
            "写入配置 {path} 失败：{}",
            openclaw_command_error_from_captured(&out)
        ));
    }

    Ok(())
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

#[cfg(target_os = "windows")]
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
        file.write_all(pretty.as_bytes())
            .map_err(|e| e.to_string())?;
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

fn set_openai_api_mode_openai_responses(
    window: &Window,
    cancel: &Arc<AtomicBool>,
    resolved: &crate::openclaw::ResolvedOpenclaw,
) -> Result<(), String> {
    check_canceled(cancel)?;
    emit_log(window, "install-log", "[config] 设置 api=openai-responses…");

    let model_value =
        run_openclaw_config_get_json_cancelable(resolved, cancel, "agents.defaults.model")?;
    let (provider_id, model_id) = parse_primary_model_ref_from_config_value(&model_value)
        .ok_or("无法从 agents.defaults.model 推断 provider/model id")?;
    emit_log(
        window,
        "install-log",
        format!("[config] provider: {provider_id}"),
    );
    emit_log(window, "install-log", format!("[config] model: {model_id}"));

    let provider_api_path = format!("models.providers.{provider_id}.api");
    run_openclaw_config_set(
        window,
        cancel,
        resolved,
        &provider_api_path,
        "openai-responses",
        false,
    )?;

    let provider_models_path = format!("models.providers.{provider_id}.models");
    let models_value =
        run_openclaw_config_get_json_cancelable(resolved, cancel, &provider_models_path)?;
    let models = models_value
        .as_array()
        .ok_or("models.providers.<provider>.models 不是数组")?;
    let model_index = models
        .iter()
        .position(|model| {
            model
                .get("id")
                .and_then(|value| value.as_str())
                .map(|value| value.trim() == model_id)
                .unwrap_or(false)
        })
        .ok_or_else(|| format!("未在 {provider_models_path} 中找到 model={model_id}"))?;

    let context_window_path =
        format!("models.providers.{provider_id}.models[{model_index}].contextWindow");
    run_openclaw_config_set(
        window,
        cancel,
        resolved,
        &context_window_path,
        &OPENAI_RESPONSES_CONTEXT_WINDOW.to_string(),
        true,
    )?;

    let max_tokens_path = format!("models.providers.{provider_id}.models[{model_index}].maxTokens");
    run_openclaw_config_set(
        window,
        cancel,
        resolved,
        &max_tokens_path,
        &OPENAI_RESPONSES_MAX_TOKENS.to_string(),
        true,
    )?;

    emit_log(window, "install-log", "[config] ok");
    Ok(())
}

fn log_environment(window: &Window, cancel: &Arc<AtomicBool>, path_env: &str) {
    let _ = check_canceled(cancel);
    emit_log(window, "install-log", "== 环境诊断 ==");
    emit_log(
        window,
        "install-log",
        format!("os: {} / {}", std::env::consts::OS, std::env::consts::ARCH),
    );
    emit_log(window, "install-log", format!("PATH: {path_env}"));

    let safe_run = |label: &str, program: &str, args: &[&str]| {
        if check_canceled(cancel).is_err() {
            return;
        }
        let out = command_output_cancelable(path_env, program, args, cancel)
            .map(|t| split_lines(&t).join(" | "))
            .unwrap_or_else(|e| format!("(failed: {e})"));
        let short = if out.len() > 800 {
            format!("{}…", &out[..800])
        } else {
            out
        };
        emit_log(window, "install-log", format!("{label}: {short}"));
    };

    safe_run("brew", "brew", &["--version"]);
    safe_run("git", "git", &["--version"]);
    safe_run("node -v", "node", &["-v"]);
    safe_run("node execPath", "node", &["-p", "process.execPath"]);
    safe_run("npm -v", "npm", &["-v"]);
    safe_run("npm prefix -g", "npm", &["prefix", "-g"]);
    safe_run("npm config get prefix", "npm", &["config", "get", "prefix"]);
    safe_run(
        "npm config get userconfig",
        "npm",
        &["config", "get", "userconfig"],
    );

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
                    emit_log(
                        window,
                        "install-log",
                        format!("nvm node versions: {joined}"),
                    );
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
                split_lines(&s)
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| "(no output)".into())
            });
            emit_log(
                window,
                "install-log",
                format!(
                    "openclaw --version: {}",
                    out.unwrap_or_else(|| "(failed)".into())
                ),
            );
        }
        None => emit_log(window, "install-log", "openclaw resolved: (not found)"),
    }

    emit_log(window, "install-log", "== 诊断结束 ==");
}

fn start_install_blocking(
    window: &Window,
    cancel: &Arc<AtomicBool>,
    options: InstallOptions,
) -> Result<(), String> {
    const MIN_NODE_MAJOR: u32 = 22;
    const DEFAULT_NPM_REGISTRY: &str = "https://registry.npmmirror.com";
    const DEFAULT_GITHUB_MIRROR: &str = "https://gitclone.com/github.com/";

    let openclaw_package =
        validate_npm_package_name(options.openclaw_package.as_deref().unwrap_or("openclaw"))?;
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
    let github_mirror_disabled = github_mirror_user
        .map(|v| is_disable_keyword(v))
        .unwrap_or(false);
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
    let total = if needs_onboard { 7u32 } else { 5u32 };
    emit_progress(window, "prepare", "准备环境…", 1, total);
    emit_log(
        window,
        "install-log",
        format!(
            "平台：{} / {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
    );
    emit_log(
        window,
        "install-log",
        format!("openclaw 包名：{openclaw_package}"),
    );
    emit_log(
        window,
        "install-log",
        format!("npm registry: {npm_registry}"),
    );
    if let Some(mirror) = github_mirror.as_deref() {
        emit_log(window, "install-log", format!("GitHub mirror: {mirror}"));
    } else {
        emit_log(window, "install-log", "GitHub mirror: (none)");
    }

    log_environment(window, cancel, &path_env);

    check_canceled(cancel)?;

    emit_progress(window, "git", "检测 Git…", 2, total);
    let git_ok = command_output_cancelable(&path_env, "git", &["--version"], cancel).is_ok();
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
            ensure_winget_cancelable(&path_env, cancel)?;
            emit_log(
                window,
                "install-log",
                "未检测到 Git，尝试通过 winget 安装 Git…",
            );
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
        let git_ok2 = command_output_cancelable(&path_env, "git", &["--version"], cancel).is_ok();
        if !git_ok2 {
            return Err("未检测到 git。请先安装 git（推荐：Homebrew 安装 git，或安装 Xcode Command Line Tools）。".into());
        }
    }

    emit_progress(window, "node", "检测 Node.js…", 3, total);
    let node_version_out = command_output_cancelable(&path_env, "node", &["-v"], cancel).ok();
    let node_major = node_version_out.as_deref().and_then(parse_node_major);
    let npm_ok = command_output_cancelable(&path_env, "npm", &["-v"], cancel).is_ok();
    let need_node = node_major.map(|m| m < MIN_NODE_MAJOR).unwrap_or(true) || !npm_ok;
    if need_node {
        #[cfg(target_os = "macos")]
        {
            let brew = find_brew(&path_env).ok_or(
                "未检测到 Node.js，且未检测到 brew。请先安装 Homebrew 或手动安装 Node.js。",
            )?;
            let mut cmd = Command::new(brew);
            apply_windows_no_window(&mut cmd);
            cmd.env("PATH", &path_env);
            cmd.args(["install", "node"]);
            let _ = run_logged(window, cancel, "[brew]", cmd)?;
        }
        #[cfg(target_os = "windows")]
        {
            ensure_winget_cancelable(&path_env, cancel)?;
            emit_log(
                window,
                "install-log",
                "Node.js / npm 不满足要求，尝试通过 winget 修复…",
            );

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
                    format!(
                        "[winget] upgrade 返回退出码 {upgrade_code}，继续检查是否需要 install…"
                    ),
                );
            }

            path_env = create_base_path_env();
            let node_after_upgrade =
                command_output_cancelable(&path_env, "node", &["-v"], cancel).ok();
            let node_major_after_upgrade = node_after_upgrade.as_deref().and_then(parse_node_major);
            let npm_after_upgrade =
                command_output_cancelable(&path_env, "npm", &["-v"], cancel).is_ok();
            let still_need_install = node_major_after_upgrade
                .map(|m| m < MIN_NODE_MAJOR)
                .unwrap_or(true)
                || !npm_after_upgrade;

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

    let node_version_out2 = command_output_cancelable(&path_env, "node", &["-v"], cancel)
        .map_err(|_| "安装后仍未检测到 node".to_string())?;
    let node_major2 = parse_node_major(&node_version_out2).ok_or("无法解析 node 版本")?;
    if node_major2 < MIN_NODE_MAJOR {
        return Err(format!(
            "Node.js 版本过低：{node_version_out2}（需要 >= {MIN_NODE_MAJOR}）"
        ));
    }

    // Ensure npm exists (some Node installations might be incomplete).
    let npm_ok_final = command_output_cancelable(&path_env, "npm", &["-v"], cancel).is_ok();
    if !npm_ok_final {
        return Err("未检测到 npm。请确认 Node.js 安装完整，或重新安装 Node.js 后重试。".into());
    }

    emit_progress(window, "openclaw", "全局安装 openclaw…", 4, total);
    check_canceled(cancel)?;
    emit_log(
        window,
        "install-log",
        format!("[npm] registry: {npm_registry}"),
    );
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
        if output_looks_like_github_connectivity_issue(&output)
            && github_mirror.is_none()
            && !github_mirror_disabled
        {
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
    emit_log(
        window,
        "install-log",
        format!("openclaw --version => {version}"),
    );

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
        let api_key = options
            .custom_api_key
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_string();
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
        emit_log(
            window,
            "install-log",
            format!("[openclaw] {}", redact_sensitive_args(&args_for_log)),
        );

        let w = window.clone();
        let code = spawn_with_streaming_logs_cancelable(cmd, cancel.clone(), move |line| {
            emit_log(&w, "install-log", format!("[openclaw] {line}"));
        })?;
        if code != 0 {
            // On failure, print gateway status to help debugging common "gateway closed" cases.
            emit_log(
                window,
                "install-log",
                "[openclaw] onboard 失败，尝试输出 gateway status 以便排查…",
            );
            if let Ok(status_text) =
                run_openclaw_collect(&resolved, &["gateway", "status", "--no-color"])
            {
                for line in split_lines(&status_text).into_iter().take(60) {
                    emit_log(window, "install-log", format!("[gateway] {line}"));
                }
            }
            return Err(format!("openclaw onboard 失败（退出码 {code}）"));
        }

        emit_progress(window, "config", "写入 openai-responses 配置…", 7, total);
        set_openai_api_mode_openai_responses(window, cancel, &resolved)?;
        emit_log(
            window,
            "install-log",
            "安装完成：未自动重启网关服务，你可以稍后手动启动。",
        );
    } else {
        emit_log(
            window,
            "install-log",
            "跳过自动配置：未提供 CUSTOM_API_KEY。",
        );
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
pub async fn start_install(
    window: Window,
    state: tauri::State<'_, TaskState>,
    options: InstallOptions,
) -> Result<(), String> {
    let cancel = state.start_with_meta(TaskMeta::new("install", "安装 OpenClaw…"))?;
    let w2 = window.clone();
    let cancel2 = cancel.clone();
    let options2 = options.clone();

    let join = tauri::async_runtime::spawn_blocking(move || {
        start_install_blocking(&w2, &cancel2, options2)
    });
    let result = match join.await {
        Ok(r) => r,
        Err(e) => Err(format!("内部错误：任务线程异常：{e}")),
    };
    state.finish();
    result
}

#[tauri::command]
pub async fn cancel_task(state: tauri::State<'_, TaskState>) -> Result<(), String> {
    state.cancel()
}

#[tauri::command]
pub async fn get_task_status(state: tauri::State<'_, TaskState>) -> Result<TaskStatus, String> {
    state.snapshot()
}

const PREFERRED_WORKSPACE_MARKDOWN_FILES: [&str; 8] = [
    "SOUL.md",
    "USER.md",
    "AGENTS.md",
    "BOOTSTRAP.md",
    "IDENTITY.md",
    "TOOLS.md",
    "HEARTBEAT.md",
    "MEMORY.md",
];

#[derive(Debug, Serialize, Clone)]
pub struct WorkspaceMarkdownDocument {
    pub name: String,
    pub path: String,
    pub exists: bool,
    pub content: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ConfigCenterData {
    pub agents_defaults: serde_json::Value,
    pub gateway: serde_json::Value,
    pub models_providers: serde_json::Value,
    pub workspace_markdowns: Vec<WorkspaceMarkdownDocument>,
}

fn default_workspace_dir() -> PathBuf {
    dirs::home_dir()
        .map(|home| home.join(".openclaw").join("workspace"))
        .unwrap_or_else(|| PathBuf::from(".openclaw").join("workspace"))
}

fn expand_home_path(value: &str) -> PathBuf {
    let trimmed = value.trim();
    if trimmed == "~" {
        return crate::openclaw::home_dir().unwrap_or_else(|| PathBuf::from(trimmed));
    }
    if let Some(rest) = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("~\\"))
    {
        return crate::openclaw::home_dir()
            .map(|home| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(trimmed));
    }
    PathBuf::from(trimmed)
}

fn resolve_workspace_dir_from_value(value: Option<&serde_json::Value>) -> PathBuf {
    if let Some(raw) = value.and_then(|value| value.as_str()) {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return expand_home_path(trimmed);
        }
    }
    default_workspace_dir()
}

fn resolve_workspace_dir_from_agents_defaults(agents_defaults: &serde_json::Value) -> PathBuf {
    let workspace_value = agents_defaults
        .as_object()
        .and_then(|value| value.get("workspace"));
    resolve_workspace_dir_from_value(workspace_value)
}

fn resolve_workspace_dir() -> PathBuf {
    if let Some(resolved) = resolve_openclaw() {
        if let Ok(value) = run_openclaw_config_get_json(&resolved, "agents.defaults.workspace") {
            return resolve_workspace_dir_from_value(Some(&value));
        }
    }
    default_workspace_dir()
}

fn normalize_workspace_markdown_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Markdown 文件名不能为空。".into());
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err("Markdown 文件名不能包含路径分隔符。".into());
    }
    if !trimmed.to_ascii_lowercase().ends_with(".md") {
        return Err("仅支持编辑 .md 文件。".into());
    }
    if trimmed == "." || trimmed == ".." || trimmed.contains("..") {
        return Err("Markdown 文件名不合法。".into());
    }
    Ok(trimmed.to_string())
}

fn collect_workspace_markdown_names(workspace_dir: &Path) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for name in PREFERRED_WORKSPACE_MARKDOWN_FILES {
        let normalized = normalize_workspace_markdown_name(name)?;
        if seen.insert(normalized.clone()) {
            names.push(normalized);
        }
    }

    if !workspace_dir.exists() {
        return Ok(names);
    }

    let mut discovered = Vec::new();
    for entry in
        std::fs::read_dir(workspace_dir).map_err(|e| format!("读取 workspace 目录失败：{e}"))?
    {
        let entry = entry.map_err(|e| format!("读取 workspace 条目失败：{e}"))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };

        let Ok(normalized) = normalize_workspace_markdown_name(file_name) else {
            continue;
        };

        if seen.insert(normalized.clone()) {
            discovered.push(normalized);
        }
    }

    discovered.sort();
    names.extend(discovered);
    Ok(names)
}

fn read_workspace_markdown_document(
    workspace_dir: &Path,
    name: &str,
) -> Result<WorkspaceMarkdownDocument, String> {
    let normalized = normalize_workspace_markdown_name(name)?;
    let path = workspace_dir.join(&normalized);
    let exists = path.exists();
    let content = if exists {
        std::fs::read_to_string(&path).map_err(|e| format!("读取 {} 失败：{e}", normalized))?
    } else {
        String::new()
    };

    Ok(WorkspaceMarkdownDocument {
        name: normalized,
        path: path.to_string_lossy().to_string(),
        exists,
        content,
    })
}

#[tauri::command]
pub async fn load_config_center_data() -> Result<ConfigCenterData, String> {
    let join = tauri::async_runtime::spawn_blocking(move || -> Result<ConfigCenterData, String> {
        let resolved = resolve_openclaw().ok_or("未检测到 openclaw，请先完成安装。")?;
        let config_root = load_openclaw_config_root(&resolved).ok();

        let agents_defaults = config_root
            .as_ref()
            .and_then(|root| get_json_value_at_path(root, &["agents", "defaults"]))
            .or_else(|| run_openclaw_config_get_json(&resolved, "agents.defaults").ok())
            .unwrap_or_else(|| serde_json::json!({}));
        let gateway = config_root
            .as_ref()
            .and_then(|root| get_json_value_at_path(root, &["gateway"]))
            .or_else(|| run_openclaw_config_get_json(&resolved, "gateway").ok())
            .unwrap_or_else(|| serde_json::json!({}));
        let models_providers = config_root
            .as_ref()
            .and_then(|root| get_json_value_at_path(root, &["models", "providers"]))
            .or_else(|| run_openclaw_config_get_json(&resolved, "models.providers").ok())
            .unwrap_or_else(|| serde_json::json!({}));

        let workspace_dir = resolve_workspace_dir_from_agents_defaults(&agents_defaults);
        let names = collect_workspace_markdown_names(&workspace_dir)?;
        let workspace_markdowns = names
            .iter()
            .map(|name| read_workspace_markdown_document(&workspace_dir, name))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(ConfigCenterData {
            agents_defaults,
            gateway,
            models_providers,
            workspace_markdowns,
        })
    });

    match join.await {
        Ok(result) => result,
        Err(error) => Err(format!("内部错误：配置加载线程异常：{error}")),
    }
}

#[tauri::command]
pub async fn load_workspace_markdowns() -> Result<Vec<WorkspaceMarkdownDocument>, String> {
    let workspace_dir = resolve_workspace_dir();
    let names = collect_workspace_markdown_names(&workspace_dir)?;
    names
        .iter()
        .map(|name| read_workspace_markdown_document(&workspace_dir, name))
        .collect()
}

#[tauri::command]
pub async fn save_workspace_markdown(
    name: String,
    content: String,
) -> Result<WorkspaceMarkdownDocument, String> {
    let normalized = normalize_workspace_markdown_name(&name)?;
    let workspace_dir = resolve_workspace_dir();
    std::fs::create_dir_all(&workspace_dir).map_err(|e| format!("创建 workspace 目录失败：{e}"))?;

    let path = workspace_dir.join(&normalized);
    let normalized_content = content.replace("\r\n", "\n");
    std::fs::write(&path, normalized_content)
        .map_err(|e| format!("写入 {} 失败：{e}", normalized))?;

    read_workspace_markdown_document(&workspace_dir, &normalized)
}
