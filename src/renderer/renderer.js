/* global installer */

const el = (id) => document.getElementById(id);

const subtitleEl = el("subtitle");
const installerCard = el("installerCard");
const dashboardCard = el("dashboardCard");
const versionPill = el("versionPill");
const gatewayStatusPill = el("gatewayStatusPill");
const refreshBtn = el("refreshBtn");
const openDashboardBtn = el("openDashboardBtn");
const openWizardBtn = el("openWizardBtn");

const installBtn = el("installBtn");
const cancelBtn = el("cancelBtn");
const stopBtn = el("stopBtn");
const showLogsCheckbox = el("showLogs");
const logCard = el("logCard");
const logEl = el("log");

const customBaseUrlInput = el("customBaseUrl");
const customModelIdInput = el("customModelId");
const customApiKeyInput = el("customApiKey");

const stageText = el("stageText");
const progressBar = el("progressBar");
const progressText = el("progressText");

const gatewayStartBtn = el("gatewayStartBtn");
const gatewayStopBtn = el("gatewayStopBtn");
const gatewayStatusBtn = el("gatewayStatusBtn");
const doctorBtn = el("doctorBtn");
const updateBtn = el("updateBtn");
const updateChannel = el("updateChannel");
const uninstallBtn = el("uninstallBtn");

const modalOverlay = el("modalOverlay");
const modalTitle = el("modalTitle");
const modalBody = el("modalBody");
const modalCancelBtn = el("modalCancelBtn");
const modalConfirmBtn = el("modalConfirmBtn");

let taskRunning = false;
let cachedGatewayUrl = null;
let cachedGatewayState = "unknown";

function confirmModal({ title, body, confirmText = "确定", cancelText = "取消" }) {
  return new Promise((resolve) => {
    if (!modalOverlay || !modalTitle || !modalBody || !modalCancelBtn || !modalConfirmBtn) {
      // Fallback: if modal markup missing, behave as canceled.
      resolve(false);
      return;
    }

    modalTitle.textContent = String(title ?? "确认");
    modalBody.textContent = String(body ?? "");
    modalCancelBtn.textContent = String(cancelText ?? "取消");
    modalConfirmBtn.textContent = String(confirmText ?? "确定");

    modalOverlay.classList.remove("hidden");

    const cleanup = () => {
      modalOverlay.classList.add("hidden");
      modalCancelBtn.removeEventListener("click", onCancel);
      modalConfirmBtn.removeEventListener("click", onConfirm);
      window.removeEventListener("keydown", onKeydown);
      modalOverlay.removeEventListener("click", onOverlayClick);
    };

    const onCancel = () => {
      cleanup();
      resolve(false);
    };

    const onConfirm = () => {
      cleanup();
      resolve(true);
    };

    const onKeydown = (event) => {
      if (event.key === "Escape") onCancel();
      if (event.key === "Enter") onConfirm();
    };

    const onOverlayClick = (event) => {
      if (event.target === modalOverlay) onCancel();
    };

    modalCancelBtn.addEventListener("click", onCancel);
    modalConfirmBtn.addEventListener("click", onConfirm);
    modalOverlay.addEventListener("click", onOverlayClick);
    window.addEventListener("keydown", onKeydown);

    try {
      modalConfirmBtn.focus();
    } catch {
      // ignore
    }
  });
}

function applyGatewayActionAvailability() {
  const isNotInstalled = cachedGatewayState === "not_installed";

  // We auto-handle service install when the user hits "Start" (so no dedicated
  // "Install service" button is needed).
  gatewayStartBtn.disabled = taskRunning;
  gatewayStopBtn.disabled = taskRunning || isNotInstalled;
  gatewayStatusBtn.disabled = taskRunning;
}

function isMissingOpenclawError(error) {
  const message = String(error?.message || error || "");
  return /未检测到 openclaw/i.test(message) || /ENOENT/i.test(message);
}

async function rerouteIfOpenclawMissing(error) {
  if (!isMissingOpenclawError(error)) return false;
  try {
    await checkAndRoute();
  } catch {
    // ignore
  }
  return true;
}

function setStage(text) {
  stageText.textContent = text;
}

function setProgress(percent) {
  const clamped = Math.max(0, Math.min(1, percent ?? 0));
  progressBar.style.width = `${Math.round(clamped * 100)}%`;
  progressText.textContent = `${Math.round(clamped * 100)}%`;
}

function appendLog(line) {
  logEl.textContent += `${line}\n`;
  logEl.scrollTop = logEl.scrollHeight;
}

function updateLogVisibility() {
  if (showLogsCheckbox.checked) {
    logCard.classList.remove("hidden");
  } else {
    logCard.classList.add("hidden");
  }
}

function setTaskRunning(value) {
  taskRunning = value;
  installBtn.disabled = value;
  cancelBtn.disabled = !value;
  stopBtn.disabled = !value;

  openDashboardBtn.disabled = value;
  openWizardBtn.disabled = value;

  applyGatewayActionAvailability();
  doctorBtn.disabled = value;
  updateBtn.disabled = value;
  if (uninstallBtn) uninstallBtn.disabled = value;
  refreshBtn.disabled = value;
}

function showInstaller() {
  subtitleEl.textContent = "一键安装";
  installerCard.classList.remove("hidden");
  dashboardCard.classList.add("hidden");
}

function showDashboard(version) {
  subtitleEl.textContent = "已安装";
  versionPill.textContent = `version: ${version ?? "-"}`;
  gatewayStatusPill.textContent = "网关：检测中…";
  installerCard.classList.add("hidden");
  dashboardCard.classList.remove("hidden");
  applyGatewayActionAvailability();
}

function setGatewayStatus(status) {
  // Hide "not_installed" from the UI by normalizing it to "stopped".
  // When users hit "Start", we will attempt `openclaw gateway install` first.
  const rawState = status?.state || "unknown";
  const state = rawState === "not_installed" ? "stopped" : rawState;
  const textMap = {
    running: "运行中",
    stopped: "未运行",
    unknown: "未知"
  };

  const label = textMap[state] ?? "未知";
  gatewayStatusPill.textContent = `网关：${label}`;

  cachedGatewayState = state;
  cachedGatewayUrl = status?.dashboardUrl || null;
  applyGatewayActionAvailability();
}

async function refreshGatewayStatus() {
  try {
    const status = await installer.getGatewayStatus();
    setGatewayStatus(status);
  } catch (error) {
    cachedGatewayUrl = null;
    gatewayStatusPill.textContent = "网关：检测失败";
    appendLog(`[错误] ${error?.message || String(error)}`);
    await rerouteIfOpenclawMissing(error);
  }
}

async function checkAndRoute() {
  subtitleEl.textContent = "检测中…";
  try {
    const info = await installer.checkOpenclaw();
    if (info?.installed) {
      showDashboard(info.version);
      setStage("就绪");
      setProgress(1);
      await refreshGatewayStatus();
      return true;
    }
  } catch (error) {
    appendLog(`[错误] ${error?.message || String(error)}`);
  }

  cachedGatewayUrl = null;
  cachedGatewayState = "unknown";
  showInstaller();
  setStage("等待开始…");
  setProgress(0);
  return false;
}

async function runOpenclaw(args, { stageLabel }) {
  if (taskRunning) return;
  setTaskRunning(true);
  logEl.textContent = "";
  setStage(stageLabel || "执行中…");
  setProgress(0.05);

  try {
    await installer.runOpenclaw(args);
    setStage("完成");
    setProgress(1);
  } catch (error) {
    const message = error?.message || String(error);
    setStage("失败");
    appendLog(`[错误] ${message}`);
    showLogsCheckbox.checked = true;
    updateLogVisibility();
    await rerouteIfOpenclawMissing(error);
  } finally {
    setTaskRunning(false);
    await checkAndRoute();
  }
}

showLogsCheckbox.addEventListener("change", () => updateLogVisibility());

stopBtn.addEventListener("click", async () => {
  if (!taskRunning) return;
  await installer.cancelTask();
});

cancelBtn.addEventListener("click", async () => {
  if (!taskRunning) return;
  await installer.cancelTask();
});

installBtn.addEventListener("click", async () => {
  if (taskRunning) return;
  setTaskRunning(true);
  logEl.textContent = "";
  setStage("安装中…");
  setProgress(0);

  try {
    const customBaseUrl = customBaseUrlInput?.value ? String(customBaseUrlInput.value).trim() : "";
    const customModelId = customModelIdInput?.value ? String(customModelIdInput.value).trim() : "";
    const customApiKey = customApiKeyInput?.value ? String(customApiKeyInput.value).trim() : "";
    await installer.startInstall({
      customBaseUrl,
      customModelId,
      customApiKey
    });
    setStage("完成");
    setProgress(1);

    // Tauri 版本会在安装完成后自动执行 non-interactive onboard（如果提供了 CUSTOM_API_KEY），
    // 因此这里不再自动打开交互式向导终端窗口。
  } catch (error) {
    const message = error?.message || String(error);
    setStage("失败");
    appendLog(`[错误] ${message}`);
    showLogsCheckbox.checked = true;
    updateLogVisibility();
  } finally {
    setTaskRunning(false);
    await checkAndRoute();
  }
});

refreshBtn.addEventListener("click", async () => {
  await checkAndRoute();
});

openDashboardBtn.addEventListener("click", async () => {
  if (taskRunning) return;
  setTaskRunning(true);
  try {
    // Prefer the Dashboard URL from gateway status when available, since it's
    // fast and avoids extra command parsing. Fall back to `openclaw dashboard`
    // lookup when we don't have a cached URL yet.
    if (!cachedGatewayUrl) {
      try {
        const status = await installer.getGatewayStatus();
        setGatewayStatus(status);
      } catch {
        // ignore
      }
    }

    if (cachedGatewayUrl) {
      await installer.openExternal(cachedGatewayUrl);
      return;
    }

    await installer.openDashboard();
  } catch (error) {
    appendLog(`[错误] ${error?.message || String(error)}`);
    showLogsCheckbox.checked = true;
    updateLogVisibility();
    await rerouteIfOpenclawMissing(error);
  } finally {
    setTaskRunning(false);
  }
});

openWizardBtn.addEventListener("click", async () => {
  if (taskRunning) return;
  setTaskRunning(true);
  try {
    await installer.openWizard();
  } catch (error) {
    appendLog(`[错误] ${error?.message || String(error)}`);
    showLogsCheckbox.checked = true;
    updateLogVisibility();
    await rerouteIfOpenclawMissing(error);
  } finally {
    setTaskRunning(false);
  }
});

gatewayStartBtn.addEventListener("click", async () => {
  // If the service wasn't installed (or got removed), installing on-demand keeps
  // the UI simpler while still being resilient.
  try {
    const status = await installer.getGatewayStatus();
    if (status?.state === "not_installed") {
      await runOpenclaw(["gateway", "install"], { stageLabel: "安装网关服务…" });
    }
  } catch {
    // ignore and let start attempt run
  }
  await runOpenclaw(["gateway", "start"], { stageLabel: "启动网关服务…" });
  await refreshGatewayStatus();
});

gatewayStopBtn.addEventListener("click", async () => {
  await runOpenclaw(["gateway", "stop"], { stageLabel: "停止网关服务…" });
});

gatewayStatusBtn.addEventListener("click", async () => {
  await runOpenclaw(["gateway", "status"], { stageLabel: "检查网关状态…" });
  await refreshGatewayStatus();
});

doctorBtn.addEventListener("click", async () => {
  await runOpenclaw(["doctor", "--fix", "--yes", "--non-interactive"], { stageLabel: "健康检查/修复…" });
});

updateBtn.addEventListener("click", async () => {
  const channel = updateChannel.value || "stable";
  await runOpenclaw(["update", "--channel", channel, "--yes"], { stageLabel: "更新中…" });
});

if (uninstallBtn) {
  uninstallBtn.addEventListener("click", async () => {
    if (taskRunning) return;
    showLogsCheckbox.checked = true;
    updateLogVisibility();
    appendLog("[ui] 点击卸载");
    const confirmed = await confirmModal({
      title: "确认卸载",
      body: "将执行 openclaw 卸载（service/state/workspace），并尝试自动移除 CLI（brew / npm / pnpm / nvm）。是否继续？",
      confirmText: "继续卸载",
      cancelText: "取消"
    });
    if (!confirmed) {
      appendLog("[ui] 用户取消卸载");
      return;
    }

    setTaskRunning(true);
    logEl.textContent = "";
    appendLog("[ui] 开始卸载…");
    setStage("卸载中…");
    setProgress(0.05);

    let routed = false;
    try {
      await installer.uninstallOpenclaw();
      setStage("检测 openclaw 命令是否仍存在…");
      routed = await checkAndRoute();
      if (!routed) {
        appendLog("openclaw 命令已移除，已切换到安装界面。");
      } else {
        setStage("完成");
        setProgress(1);
      }
    } catch (error) {
      setStage("失败");
      appendLog(`[错误] ${error?.message || String(error)}`);
      showLogsCheckbox.checked = true;
      updateLogVisibility();
    } finally {
      setTaskRunning(false);
      if (!routed) await checkAndRoute();
    }
  });
}

installer.onProgress((payload) => {
  const percent = payload?.percent;
  if (percent != null) setProgress(percent);
  if (payload?.stage === "done") {
    setStage("完成");
    return;
  }
  if (payload?.index && payload?.total) {
    setStage(`安装中…（${payload.index}/${payload.total}）`);
    return;
  }
  setStage("安装中…");
});

installer.onLog((payload) => {
  if (!payload?.message) return;
  appendLog(payload.message);
});

installer.onOpenclawLog((payload) => {
  if (!payload?.message) return;
  appendLog(payload.message);
});

updateLogVisibility();
setTaskRunning(false);
void checkAndRoute();
