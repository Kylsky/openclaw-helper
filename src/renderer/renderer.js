/* global installer */

const el = (id) => document.getElementById(id);

const subtitleEl = el("subtitle");
const installerCard = el("installerCard");
const dashboardCard = el("dashboardCard");
const configCard = el("configCard");

const versionPill = el("versionPill");
const gatewayStatusPill = el("gatewayStatusPill");
const refreshBtn = el("refreshBtn");
const openDashboardBtn = el("openDashboardBtn");
const openConfigBtn = el("openConfigBtn");

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
const gatewayRestartBtn = el("gatewayRestartBtn");

const configCancelBtn = el("configCancelBtn");
const configSaveBtn = el("configSaveBtn");
const configContent = el("configContent");
const configLoadingOverlay = el("configLoadingOverlay");
const configLoadingTitle = el("configLoadingTitle");
const configLoadingHint = el("configLoadingHint");
const cfgDefaultModel = el("cfgDefaultModel");
const cfgProviderId = el("cfgProviderId");
const cfgProviderApi = el("cfgProviderApi");
const cfgBaseUrl = el("cfgBaseUrl");
const cfgApiKey = el("cfgApiKey");
const cfgContextWindow = el("cfgContextWindow");
const cfgMaxTokens = el("cfgMaxTokens");
const cfgWorkspace = el("cfgWorkspace");
const cfgMaxConcurrent = el("cfgMaxConcurrent");
const cfgSubagentsMaxConcurrent = el("cfgSubagentsMaxConcurrent");
const cfgCompactionMode = el("cfgCompactionMode");
const cfgGatewayMode = el("cfgGatewayMode");
const cfgGatewayBind = el("cfgGatewayBind");
const cfgGatewayPort = el("cfgGatewayPort");
const cfgGatewayAuthMode = el("cfgGatewayAuthMode");
const cfgGatewayCustomBindHostField = el("cfgGatewayCustomBindHostField");
const cfgGatewayCustomBindHost = el("cfgGatewayCustomBindHost");
const cfgGatewayAuthTokenField = el("cfgGatewayAuthTokenField");
const cfgGatewayAuthToken = el("cfgGatewayAuthToken");
const cfgGatewayAuthPasswordField = el("cfgGatewayAuthPasswordField");
const cfgGatewayAuthPassword = el("cfgGatewayAuthPassword");
const cfgGatewayTrustedProxiesField = el("cfgGatewayTrustedProxiesField");
const cfgGatewayTrustedProxies = el("cfgGatewayTrustedProxies");
const cfgGatewayTrustedProxyUserHeaderField = el("cfgGatewayTrustedProxyUserHeaderField");
const cfgGatewayTrustedProxyUserHeader = el("cfgGatewayTrustedProxyUserHeader");
const cfgGatewayTrustedProxyRequiredHeadersField = el("cfgGatewayTrustedProxyRequiredHeadersField");
const cfgGatewayTrustedProxyRequiredHeaders = el("cfgGatewayTrustedProxyRequiredHeaders");
const cfgGatewayTrustedProxyAllowUsersField = el("cfgGatewayTrustedProxyAllowUsersField");
const cfgGatewayTrustedProxyAllowUsers = el("cfgGatewayTrustedProxyAllowUsers");
const cfgDefaultModelPresets = el("cfgDefaultModelPresets");
const workspaceMarkdownExpandBtn = el("workspaceMarkdownExpandBtn");
const workspaceMarkdownCollapseBtn = el("workspaceMarkdownCollapseBtn");
const workspaceMarkdownList = el("workspaceMarkdownList");

const doctorBtn = el("doctorBtn");
const updateBtn = el("updateBtn");
const updateChannel = el("updateChannel");
const uninstallBtn = el("uninstallBtn");

const operationOverlay = el("operationOverlay");
const operationTitle = el("operationTitle");
const operationCloseBtn = el("operationCloseBtn");
const modalOverlay = el("modalOverlay");
const modalTitle = el("modalTitle");
const modalBody = el("modalBody");
const modalCancelBtn = el("modalCancelBtn");
const modalConfirmBtn = el("modalConfirmBtn");
function getConfigInputs() {
  return Array.from(document.querySelectorAll("#configCard input, #configCard select, #configCard textarea"));
}

let taskRunning = false;
let cachedGatewayState = "unknown";
let loadedConfigValues = {};
let loadedProviderContext = { providerId: "", modelId: "", modelIndex: -1 };
let loadedProviderCatalog = {};
let loadedWorkspaceMarkdowns = [];
let workspaceMarkdownEditors = new Map();
let expandedWorkspaceMarkdownNames = null;
let configLoadToken = 0;

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

function isOperationModalOpen() {
  return Boolean(operationOverlay && !operationOverlay.classList.contains("hidden"));
}

function updateOperationModalAvailability() {
  if (!operationCloseBtn) return;
  operationCloseBtn.disabled = taskRunning;
  operationCloseBtn.title = taskRunning ? "任务执行中，暂不可关闭" : "";
}

function showOperationModal(title = "执行详情") {
  if (!operationOverlay || !operationTitle) return;
  operationTitle.textContent = String(title ?? "执行详情");
  operationOverlay.classList.remove("hidden");
  updateOperationModalAvailability();
  updateLogVisibility();
}

function hideOperationModal() {
  if (!operationOverlay || taskRunning) return;
  operationOverlay.classList.add("hidden");
}

function setConfigLoading(value, { title, hint } = {}) {
  if (configLoadingOverlay) {
    configLoadingOverlay.classList.toggle("hidden", !value);
  }
  if (configContent) {
    configContent.classList.toggle("configContentLoading", value);
  }
  if (configLoadingTitle && title) {
    configLoadingTitle.textContent = String(title);
  }
  if (configLoadingHint && hint) {
    configLoadingHint.textContent = String(hint);
  }
  if (configSaveBtn) {
    configSaveBtn.disabled = value || taskRunning;
  }
}

function getObjectValue(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function getNestedValue(source, path, fallback = null) {
  const segments = Array.isArray(path) ? path : String(path).split(".");
  let current = source;
  for (const segment of segments) {
    if (!current || typeof current !== "object") return fallback;
    current = current[segment];
  }
  return current === undefined ? fallback : current;
}

function waitMs(ms) {
  return new Promise((resolve) => window.setTimeout(resolve, Math.max(0, ms || 0)));
}

function setButtonLoading(button, value) {
  if (!(button instanceof HTMLButtonElement)) return;
  if (value) {
    button.classList.add("buttonLoading");
    button.setAttribute("aria-busy", "true");
    return;
  }

  button.classList.remove("buttonLoading");
  button.removeAttribute("aria-busy");
}

async function withButtonLoading(button, action, { minimumMs = 260 } = {}) {
  if (!(button instanceof HTMLButtonElement)) return await action();

  const startedAt = Date.now();
  setButtonLoading(button, true);
  try {
    return await action();
  } finally {
    const elapsed = Date.now() - startedAt;
    if (elapsed < minimumMs) {
      await waitMs(minimumMs - elapsed);
    }
    setButtonLoading(button, false);
  }
}

function applyGatewayActionAvailability() {
  const isNotInstalled = cachedGatewayState === "not_installed";
  const isRunning = cachedGatewayState === "running";

  // We auto-handle service install when the user hits "Start" (so no dedicated
  // "Install service" button is needed).
  gatewayStartBtn.disabled = taskRunning;
  gatewayStopBtn.disabled = taskRunning || isNotInstalled;
  gatewayRestartBtn.disabled = taskRunning || isNotInstalled;

  // "Open Dashboard" only works when the local gateway is running (tokenized URL).
  openDashboardBtn.disabled = taskRunning || !isRunning;
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
  const raw = String(line ?? "");

  const escapeHtml = (value) =>
    String(value)
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;")
      .replace(/'/g, "&#39;");

  const classify = (text) => {
    const trimmed = text.trim();
    if (trimmed.startsWith("[错误]") || trimmed.startsWith("[前端错误]")) return "logLine logError";
    if (trimmed.startsWith("[stderr]") || /npm warn/i.test(trimmed) || trimmed.startsWith("[警告]"))
      return "logLine logWarn";
    if (
      trimmed.startsWith("[npm]") ||
      trimmed.startsWith("[brew]") ||
      trimmed.startsWith("[openclaw]") ||
      trimmed.startsWith("[config]") ||
      trimmed.startsWith("[markdown]")
    ) {
      return "logLine logCmd";
    }
    if (/^\[ui\]/i.test(trimmed)) return "logLine";
    return "logLine";
  };

  const klass = classify(raw);
  const html = `<div class="${klass}">${escapeHtml(raw)}</div>`;
  logEl.insertAdjacentHTML("beforeend", html);
  logEl.scrollTop = logEl.scrollHeight;
}

function updateLogVisibility() {
  if (showLogsCheckbox.checked) {
    logCard.classList.remove("hidden");
  } else {
    logCard.classList.add("hidden");
  }
}

function getInputValue(input) {
  return input?.value ? String(input.value).trim() : "";
}

function setInputValue(input, value) {
  if (!input) return;
  const nextValue = value === undefined || value === null ? "" : String(value);

  if (input instanceof HTMLSelectElement && nextValue) {
    const exists = Array.from(input.options).some((option) => option.value === nextValue);
    if (!exists) {
      const dynamicOption = document.createElement("option");
      dynamicOption.value = nextValue;
      dynamicOption.textContent = `${nextValue}（当前值）`;
      dynamicOption.dataset.dynamic = "true";
      input.append(dynamicOption);
    }
  }

  input.value = nextValue;
}

function setDatalistOptions(datalist, values) {
  if (!(datalist instanceof HTMLDataListElement)) return;
  const uniqueValues = Array.from(new Set((values || []).map((value) => String(value ?? "").trim()).filter(Boolean)));
  datalist.replaceChildren(
    ...uniqueValues.map((value) => {
      const option = document.createElement("option");
      option.value = value;
      return option;
    })
  );
}


function autosizeTextarea(textarea) {
  if (!(textarea instanceof HTMLTextAreaElement)) return;
  textarea.style.height = "auto";
  textarea.style.height = `${Math.max(textarea.scrollHeight, 160)}px`;
}

function normalizeWorkspaceMarkdownDocument(document) {
  return {
    name: String(document?.name ?? "").trim(),
    path: String(document?.path ?? "").trim(),
    exists: Boolean(document?.exists),
    content: String(document?.content ?? "").replace(/\r\n/g, "\n")
  };
}

function syncWorkspaceMarkdownExpandedNames() {
  expandedWorkspaceMarkdownNames = new Set(
    Array.from(workspaceMarkdownEditors.entries())
      .filter(([, editor]) => Boolean(editor?.details?.open))
      .map(([name]) => name)
  );
}

function updateWorkspaceMarkdownToolbarAvailability() {
  const hasEditors = workspaceMarkdownEditors.size > 0;
  if (workspaceMarkdownExpandBtn) workspaceMarkdownExpandBtn.disabled = taskRunning || !hasEditors;
  if (workspaceMarkdownCollapseBtn) workspaceMarkdownCollapseBtn.disabled = taskRunning || !hasEditors;
}

function updateWorkspaceMarkdownDirtyState(name) {
  const editor = workspaceMarkdownEditors.get(name);
  const original = loadedWorkspaceMarkdowns.find((documentItem) => documentItem.name === name);
  if (!editor || !original) return;

  const nextContent = String(editor.textarea?.value ?? "").replace(/\r\n/g, "\n");
  const prevContent = String(original.content ?? "").replace(/\r\n/g, "\n");
  const isDirty = nextContent !== prevContent;

  if (editor.stateBadge) {
    editor.stateBadge.textContent = isDirty ? "已修改" : "未改动";
    editor.stateBadge.classList.toggle("dirty", isDirty);
  }
}

function renderWorkspaceMarkdownEditors(documents) {
  if (!workspaceMarkdownList) return;

  if (workspaceMarkdownEditors.size > 0) {
    syncWorkspaceMarkdownExpandedNames();
  }

  workspaceMarkdownEditors = new Map();
  workspaceMarkdownList.replaceChildren();

  const list = Array.isArray(documents) ? documents : [];
  if (list.length === 0) {
    const empty = document.createElement("div");
    empty.className = "hint";
    empty.textContent = "当前未发现可编辑的 workspace Markdown 文件。";
    workspaceMarkdownList.append(empty);
    updateWorkspaceMarkdownToolbarAvailability();
    return;
  }

  const restoredExpanded = expandedWorkspaceMarkdownNames instanceof Set ? new Set(expandedWorkspaceMarkdownNames) : null;

  list.forEach((documentItem) => {
    const details = document.createElement("details");
    details.className = "markdownEditorCard markdownEditorAccordion";
    details.open = restoredExpanded ? restoredExpanded.has(documentItem.name) : false;

    const summary = document.createElement("summary");
    summary.className = "markdownEditorSummary";

    const summaryMain = document.createElement("div");
    summaryMain.className = "markdownEditorSummaryMain";

    const title = document.createElement("div");
    title.className = "featureTitle";
    title.textContent = documentItem.name;

    const summaryMeta = document.createElement("div");
    summaryMeta.className = "markdownEditorSummaryMeta";
    summaryMeta.textContent = documentItem.path || documentItem.name;

    summaryMain.append(title, summaryMeta);

    const badges = document.createElement("div");
    badges.className = "markdownEditorSummaryBadges";

    const stateBadge = document.createElement("span");
    stateBadge.className = "markdownEditorBadge";
    stateBadge.textContent = "未改动";

    const existsBadge = document.createElement("span");
    existsBadge.className = `markdownEditorBadge${documentItem.exists ? "" : " missing"}`;
    existsBadge.textContent = documentItem.exists ? "已存在" : "待创建";

    badges.append(stateBadge, existsBadge);
    summary.append(summaryMain, badges);

    const body = document.createElement("div");
    body.className = "markdownEditorBody";

    const textarea = document.createElement("textarea");
    textarea.className = "markdownTextarea";
    textarea.value = documentItem.content;
    textarea.placeholder = documentItem.exists
      ? `编辑 ${documentItem.name} 内容…`
      : `${documentItem.name} 当前不存在，输入内容后保存会自动创建。`;
    textarea.disabled = taskRunning;
    textarea.spellcheck = false;
    textarea.dataset.markdownName = documentItem.name;
    textarea.addEventListener("input", () => {
      autosizeTextarea(textarea);
      updateWorkspaceMarkdownDirtyState(documentItem.name);
    });

    body.append(textarea);

    if (!documentItem.exists) {
      const hint = document.createElement("div");
      hint.className = "hint";
      hint.textContent = "当前文件不存在，保存后自动创建。";
      body.append(hint);
    }

    details.append(summary, body);
    details.addEventListener("toggle", () => {
      syncWorkspaceMarkdownExpandedNames();
      if (details.open) autosizeTextarea(textarea);
    });
    workspaceMarkdownList.append(details);

    autosizeTextarea(textarea);
    workspaceMarkdownEditors.set(documentItem.name, {
      textarea,
      details,
      stateBadge,
      existsBadge,
      path: documentItem.path,
      exists: documentItem.exists
    });
    updateWorkspaceMarkdownDirtyState(documentItem.name);
  });

  if (restoredExpanded === null) {
    syncWorkspaceMarkdownExpandedNames();
  }
  updateWorkspaceMarkdownToolbarAvailability();
}

async function loadWorkspaceMarkdownValues(preloadedDocuments = null) {
  if (!workspaceMarkdownList) return;

  try {
    const documents = preloadedDocuments ?? await installer.loadWorkspaceMarkdowns();
    loadedWorkspaceMarkdowns = Array.isArray(documents)
      ? documents.map((documentItem) => normalizeWorkspaceMarkdownDocument(documentItem)).filter((documentItem) => documentItem.name)
      : [];
    renderWorkspaceMarkdownEditors(loadedWorkspaceMarkdowns);
  } catch (error) {
    loadedWorkspaceMarkdowns = [];
    renderWorkspaceMarkdownEditors([]);
    appendLog(`[错误] 读取 workspace Markdown 失败：${error?.message || String(error)}`);
  }
}

function collectWorkspaceMarkdownChanges() {
  return loadedWorkspaceMarkdowns.flatMap((documentItem) => {
    const editor = workspaceMarkdownEditors.get(documentItem.name);
    if (!editor?.textarea) return [];

    const nextContent = String(editor.textarea.value ?? "").replace(/\r\n/g, "\n");
    const prevContent = String(documentItem.content ?? "").replace(/\r\n/g, "\n");
    if (nextContent === prevContent) return [];

    return [{
      name: documentItem.name,
      path: documentItem.path,
      content: nextContent,
      exists: documentItem.exists
    }];
  });
}

async function runWorkspaceMarkdownSaveSequence(changes, { stageLabel } = {}) {
  if (taskRunning) return false;
  const list = Array.isArray(changes) ? changes.filter(Boolean) : [];
  if (list.length === 0) return true;

  showOperationModal(stageLabel || "保存 Workspace Markdown…");
  setTaskRunning(true);
  setStage(stageLabel || "保存 Workspace Markdown…");
  setProgress(0.05);

  try {
    for (let i = 0; i < list.length; i += 1) {
      const item = list[i];
      setStage(`保存 ${item.name}…`);
      setProgress(Math.min(0.95, 0.1 + (i / Math.max(1, list.length)) * 0.8));
      appendLog(`[markdown] 保存 ${item.name}...`);
      await installer.saveWorkspaceMarkdown(item.name, item.content);
    }
    setStage("完成");
    setProgress(1);
    return true;
  } catch (error) {
    setStage("失败");
    appendLog(`[错误] ${error?.message || String(error)}`);
    showLogsCheckbox.checked = true;
    updateLogVisibility();
    return false;
  } finally {
    setTaskRunning(false);
  }
}

function normalizeStringList(value) {
  if (!Array.isArray(value)) return [];
  return value.map((item) => String(item ?? "").trim()).filter(Boolean);
}

function formatStringList(value) {
  return normalizeStringList(value).join("\n");
}

function parseStringListInput(value) {
  return Array.from(new Set(
    String(value ?? "")
      .split(/[\n\r,]+/)
      .map((item) => item.trim())
      .filter(Boolean)
  ));
}

function getPrimaryModelRef(value) {
  if (typeof value === "string") return value.trim();
  if (value && typeof value === "object" && typeof value.primary === "string") {
    return value.primary.trim();
  }
  return "";
}

function parseModelRef(modelRef) {
  const text = String(modelRef ?? "").trim();
  const slashIndex = text.indexOf("/");
  if (slashIndex <= 0 || slashIndex >= text.length - 1) {
    return { providerId: "", modelId: "" };
  }

  return {
    providerId: text.slice(0, slashIndex).trim(),
    modelId: text.slice(slashIndex + 1).trim()
  };
}

function resolveProviderContext(modelRef, providers = loadedProviderCatalog) {
  const { providerId, modelId } = parseModelRef(modelRef);
  const context = {
    providerId,
    modelId,
    modelIndex: -1,
    providerApi: "",
    baseUrl: "",
    hasApiKey: false,
    contextWindow: "",
    maxTokens: ""
  };

  if (!providerId) return context;

  const providerCatalog = getObjectValue(providers);
  const provider = getObjectValue(providerCatalog[providerId]);
  if (!Object.keys(provider).length) return context;

  if (typeof provider.api === "string") context.providerApi = provider.api;
  if (typeof provider.baseUrl === "string") context.baseUrl = provider.baseUrl;
  context.hasApiKey = typeof provider.apiKey === "string" && provider.apiKey.trim().length > 0;

  const models = Array.isArray(provider.models) ? provider.models : [];
  const modelIndex = models.findIndex((item) => item && typeof item.id === "string" && item.id.trim() === modelId);
  if (modelIndex >= 0) {
    context.modelIndex = modelIndex;
    const model = getObjectValue(models[modelIndex]);
    if (model.contextWindow !== undefined && model.contextWindow !== null) {
      context.contextWindow = String(model.contextWindow);
    }
    if (model.maxTokens !== undefined && model.maxTokens !== null) {
      context.maxTokens = String(model.maxTokens);
    }
  }

  return context;
}

function updateProviderPreview() {
  const { providerId } = parseModelRef(getInputValue(cfgDefaultModel));
  setInputValue(cfgProviderId, providerId || loadedProviderContext.providerId || "");
}

function buildModelPresetValues(providers) {
  if (!providers || typeof providers !== "object") return [];

  return Object.entries(providers).flatMap(([providerId, provider]) => {
    const models = Array.isArray(provider?.models) ? provider.models : [];
    return models
      .map((model) => {
        const modelId = typeof model?.id === "string" ? model.id.trim() : "";
        if (!modelId) return "";
        return `${providerId}/${modelId}`;
      })
      .filter(Boolean);
  });
}

function refreshConfigPresetLists() {
  const defaultModelValues = buildModelPresetValues(loadedProviderCatalog);
  if (loadedConfigValues.defaultModel) defaultModelValues.unshift(loadedConfigValues.defaultModel);
  setDatalistOptions(cfgDefaultModelPresets, defaultModelValues);
}

function updateGatewayBindVisibility() {
  const isCustomBind = getInputValue(cfgGatewayBind) === "custom";
  if (cfgGatewayCustomBindHostField) {
    cfgGatewayCustomBindHostField.classList.toggle("hidden", !isCustomBind);
  }
  if (cfgGatewayCustomBindHost) {
    cfgGatewayCustomBindHost.disabled = taskRunning || !isCustomBind;
  }
}

function updateGatewayAuthVisibility() {
  const authMode = getInputValue(cfgGatewayAuthMode);
  const useToken = authMode === "token" || !authMode;
  const usePassword = authMode === "password";
  const useTrustedProxy = authMode === "trusted-proxy";

  if (cfgGatewayAuthTokenField) {
    cfgGatewayAuthTokenField.classList.toggle("hidden", !useToken);
  }
  if (cfgGatewayAuthPasswordField) {
    cfgGatewayAuthPasswordField.classList.toggle("hidden", !usePassword);
  }
  if (cfgGatewayTrustedProxiesField) {
    cfgGatewayTrustedProxiesField.classList.toggle("hidden", !useTrustedProxy);
  }
  if (cfgGatewayTrustedProxyUserHeaderField) {
    cfgGatewayTrustedProxyUserHeaderField.classList.toggle("hidden", !useTrustedProxy);
  }
  if (cfgGatewayTrustedProxyRequiredHeadersField) {
    cfgGatewayTrustedProxyRequiredHeadersField.classList.toggle("hidden", !useTrustedProxy);
  }
  if (cfgGatewayTrustedProxyAllowUsersField) {
    cfgGatewayTrustedProxyAllowUsersField.classList.toggle("hidden", !useTrustedProxy);
  }
  if (cfgGatewayAuthToken) {
    cfgGatewayAuthToken.disabled = taskRunning || !useToken;
  }
  if (cfgGatewayAuthPassword) {
    cfgGatewayAuthPassword.disabled = taskRunning || !usePassword;
  }
  if (cfgGatewayTrustedProxies) {
    cfgGatewayTrustedProxies.disabled = taskRunning || !useTrustedProxy;
  }
  if (cfgGatewayTrustedProxyUserHeader) {
    cfgGatewayTrustedProxyUserHeader.disabled = taskRunning || !useTrustedProxy;
  }
  if (cfgGatewayTrustedProxyRequiredHeaders) {
    cfgGatewayTrustedProxyRequiredHeaders.disabled = taskRunning || !useTrustedProxy;
  }
  if (cfgGatewayTrustedProxyAllowUsers) {
    cfgGatewayTrustedProxyAllowUsers.disabled = taskRunning || !useTrustedProxy;
  }
}

function setSecretPlaceholder(input, configuredText, emptyText, hasConfiguredValue) {
  if (!input) return;
  input.placeholder = hasConfiguredValue ? configuredText : emptyText;
}

function queueTextConfigSet(seq, { label, path, value, currentValue = "", stageLabel, strictJson = false, secret = false }) {
  const nextValue = String(value ?? "").trim();
  const prevValue = String(currentValue ?? "").trim();

  if (secret) {
    if (!nextValue) return;
  } else if (!nextValue || nextValue === prevValue) {
    return;
  }

  seq.push({
    args: strictJson
      ? ["config", "set", "--strict-json", path, JSON.stringify(nextValue)]
      : ["config", "set", path, nextValue],
    stageLabel,
    logLine: `[config] 更新${label}...`
  });
}

function parsePositiveInteger(rawValue, label, { min = 1, max = Number.MAX_SAFE_INTEGER } = {}) {
  const text = String(rawValue ?? "").trim();
  if (!text) return { empty: true };
  if (!/^\d+$/.test(text)) {
    return { error: `${label} 必须是正整数。` };
  }

  const value = Number.parseInt(text, 10);
  if (value < min || value > max) {
    return { error: `${label} 必须在 ${min} - ${max} 之间。` };
  }

  return { value };
}

function queueIntegerConfigSet(seq, { label, path, rawValue, currentValue = "", stageLabel, min = 1, max = Number.MAX_SAFE_INTEGER }) {
  const parsed = parsePositiveInteger(rawValue, label, { min, max });
  if (parsed.error) throw new Error(parsed.error);
  if (parsed.empty) return;

  const nextValue = parsed.value;
  if (String(nextValue) === String(currentValue ?? "")) return;

  seq.push({
    args: ["config", "set", "--strict-json", path, JSON.stringify(nextValue)],
    stageLabel,
    logLine: `[config] 更新${label}...`
  });
}

function queueStringArrayConfigSet(seq, { label, path, rawValue, currentValue = [], stageLabel }) {
  const nextValue = parseStringListInput(rawValue);
  const prevValue = normalizeStringList(currentValue);
  if (JSON.stringify(nextValue) === JSON.stringify(prevValue)) return;

  seq.push({
    args: ["config", "set", "--strict-json", path, JSON.stringify(nextValue)],
    stageLabel,
    logLine: `[config] 更新${label}...`
  });
}

function setTaskRunning(value) {
  taskRunning = value;
  installBtn.disabled = value;
  cancelBtn.disabled = !value;
  stopBtn.disabled = !value;
  updateOperationModalAvailability();

  applyGatewayActionAvailability();
  doctorBtn.disabled = value;
  updateBtn.disabled = value;
  if (uninstallBtn) uninstallBtn.disabled = value;
  if (configSaveBtn) configSaveBtn.disabled = value;
  if (configCancelBtn) configCancelBtn.disabled = value;
  if (openConfigBtn) openConfigBtn.disabled = value;
  refreshBtn.disabled = value;
  getConfigInputs().forEach((input) => {
    if (input.id === "cfgProviderId") return;
    input.disabled = value;
  });
  updateGatewayBindVisibility();
  updateGatewayAuthVisibility();
  updateWorkspaceMarkdownToolbarAvailability();
}

function showInstaller() {
  if (subtitleEl) subtitleEl.textContent = "一键安装";
  installerCard.classList.remove("hidden");
  dashboardCard.classList.add("hidden");
  configCard.classList.add("hidden");
}

function showDashboard(version) {
  if (subtitleEl) subtitleEl.textContent = "已安装";
  versionPill.textContent = `version: ${version ?? "-"}`;
  gatewayStatusPill.textContent = "网关：检测中…";
  installerCard.classList.add("hidden");
  dashboardCard.classList.remove("hidden");
  configCard.classList.add("hidden");
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
  applyGatewayActionAvailability();
}

async function refreshGatewayStatus() {
  try {
    const status = await installer.getGatewayStatus();
    setGatewayStatus(status);
  } catch (error) {
    gatewayStatusPill.textContent = "网关：检测失败";
    appendLog(`[错误] ${error?.message || String(error)}`);
  }
}

async function loadConfigValues() {
  if (!cfgBaseUrl || !cfgDefaultModel) return;

  const currentToken = ++configLoadToken;
  setConfigLoading(true, {
    title: "正在加载配置中心…",
    hint: "正在读取 OpenClaw 配置与 Workspace Markdown"
  });
  setStage("读取配置中…");

  try {
    const snapshot = await installer.loadConfigCenterData();
    if (currentToken !== configLoadToken) return;

    const agentsDefaults = getObjectValue(snapshot?.agentsDefaults);
    const gateway = getObjectValue(snapshot?.gateway);
    const gatewayAuth = getObjectValue(gateway.auth);
    const modelsProviders = getObjectValue(snapshot?.modelsProviders);
    const defaultModel = getPrimaryModelRef(getNestedValue(agentsDefaults, ["model"]));
    const nextProviderContext = resolveProviderContext(defaultModel, modelsProviders);

    loadedProviderCatalog = modelsProviders;
    loadedProviderContext = nextProviderContext;

    const workspace = getNestedValue(agentsDefaults, ["workspace"]);
    const maxConcurrent = getNestedValue(agentsDefaults, ["maxConcurrent"]);
    const subagentsMaxConcurrent = getNestedValue(agentsDefaults, ["subagents", "maxConcurrent"]);
    const compactionMode = getNestedValue(agentsDefaults, ["compaction", "mode"]);
    const gatewayMode = getNestedValue(gateway, ["mode"]);
    const gatewayBind = getNestedValue(gateway, ["bind"]);
    const gatewayCustomBindHost = getNestedValue(gateway, ["customBindHost"]);
    const gatewayPort = getNestedValue(gateway, ["port"]);
    const gatewayAuthMode = getNestedValue(gatewayAuth, ["mode"]);
    const gatewayAuthToken = getNestedValue(gatewayAuth, ["token"]);
    const gatewayAuthPassword = getNestedValue(gatewayAuth, ["password"]);
    const gatewayTrustedProxies = normalizeStringList(getNestedValue(gateway, ["trustedProxies"], []));
    const gatewayTrustedProxyUserHeader = getNestedValue(gatewayAuth, ["trustedProxy", "userHeader"]);
    const gatewayTrustedProxyRequiredHeaders = normalizeStringList(getNestedValue(gatewayAuth, ["trustedProxy", "requiredHeaders"], []));
    const gatewayTrustedProxyAllowUsers = normalizeStringList(getNestedValue(gatewayAuth, ["trustedProxy", "allowUsers"], []));

    setConfigLoading(true, {
      title: "正在渲染配置中心…",
      hint: "配置已读取完成，正在填充表单和 Markdown 编辑区。"
    });

    setInputValue(cfgDefaultModel, defaultModel);
    setInputValue(cfgProviderApi, nextProviderContext.providerApi);
    setInputValue(cfgBaseUrl, nextProviderContext.baseUrl);
    setInputValue(cfgContextWindow, nextProviderContext.contextWindow);
    setInputValue(cfgMaxTokens, nextProviderContext.maxTokens);
    setInputValue(cfgWorkspace, workspace);
    setInputValue(cfgMaxConcurrent, maxConcurrent);
    setInputValue(cfgSubagentsMaxConcurrent, subagentsMaxConcurrent);
    setInputValue(cfgCompactionMode, compactionMode);
    setInputValue(cfgGatewayMode, gatewayMode);
    setInputValue(cfgGatewayBind, gatewayBind);
    setInputValue(cfgGatewayCustomBindHost, gatewayCustomBindHost);
    setInputValue(cfgGatewayPort, gatewayPort);
    setInputValue(cfgGatewayAuthMode, gatewayAuthMode);
    setInputValue(cfgGatewayTrustedProxies, formatStringList(gatewayTrustedProxies));
    setInputValue(cfgGatewayTrustedProxyUserHeader, gatewayTrustedProxyUserHeader);
    setInputValue(cfgGatewayTrustedProxyRequiredHeaders, formatStringList(gatewayTrustedProxyRequiredHeaders));
    setInputValue(cfgGatewayTrustedProxyAllowUsers, formatStringList(gatewayTrustedProxyAllowUsers));

    if (cfgApiKey) cfgApiKey.value = "";
    if (cfgGatewayAuthToken) cfgGatewayAuthToken.value = "";
    if (cfgGatewayAuthPassword) cfgGatewayAuthPassword.value = "";

    setSecretPlaceholder(
      cfgApiKey,
      "已配置 API Key；留空表示保持原值，输入则覆盖。",
      "未配置 API Key；输入后保存。",
      nextProviderContext.hasApiKey
    );
    setSecretPlaceholder(
      cfgGatewayAuthToken,
      "已配置网关令牌；留空表示保持原值，输入则覆盖。",
      "未配置网关令牌；输入后保存。",
      typeof gatewayAuthToken === "string" && gatewayAuthToken.trim().length > 0
    );
    setSecretPlaceholder(
      cfgGatewayAuthPassword,
      "已配置网关密码；留空表示保持原值，输入则覆盖。",
      "未配置网关密码；输入后保存。",
      typeof gatewayAuthPassword === "string" && gatewayAuthPassword.trim().length > 0
    );

    loadedConfigValues = {
      defaultModel,
      providerApi: nextProviderContext.providerApi,
      baseUrl: nextProviderContext.baseUrl,
      contextWindow: nextProviderContext.contextWindow,
      maxTokens: nextProviderContext.maxTokens,
      workspace: workspace === undefined || workspace === null ? "" : String(workspace),
      maxConcurrent: maxConcurrent === undefined || maxConcurrent === null ? "" : String(maxConcurrent),
      subagentsMaxConcurrent:
        subagentsMaxConcurrent === undefined || subagentsMaxConcurrent === null ? "" : String(subagentsMaxConcurrent),
      compactionMode: compactionMode === undefined || compactionMode === null ? "" : String(compactionMode),
      gatewayMode: gatewayMode === undefined || gatewayMode === null ? "" : String(gatewayMode),
      gatewayBind: gatewayBind === undefined || gatewayBind === null ? "" : String(gatewayBind),
      gatewayCustomBindHost: gatewayCustomBindHost === undefined || gatewayCustomBindHost === null ? "" : String(gatewayCustomBindHost),
      gatewayPort: gatewayPort === undefined || gatewayPort === null ? "" : String(gatewayPort),
      gatewayAuthMode: gatewayAuthMode === undefined || gatewayAuthMode === null ? "" : String(gatewayAuthMode),
      gatewayTrustedProxies,
      gatewayTrustedProxyUserHeader:
        gatewayTrustedProxyUserHeader === undefined || gatewayTrustedProxyUserHeader === null ? "" : String(gatewayTrustedProxyUserHeader),
      gatewayTrustedProxyRequiredHeaders,
      gatewayTrustedProxyAllowUsers,
      hasGatewayAuthToken: typeof gatewayAuthToken === "string" && gatewayAuthToken.trim().length > 0,
      hasGatewayAuthPassword: typeof gatewayAuthPassword === "string" && gatewayAuthPassword.trim().length > 0
    };

    refreshConfigPresetLists();
    updateProviderPreview();
    updateGatewayBindVisibility();
    updateGatewayAuthVisibility();
    await loadWorkspaceMarkdownValues(snapshot?.workspaceMarkdowns ?? []);
    if (currentToken !== configLoadToken) return;
    setStage("等待开始…");
  } catch (error) {
    appendLog(`[错误] 读取配置中心失败：${error?.message || String(error)}`);
  } finally {
    if (currentToken === configLoadToken) {
      setConfigLoading(false);
    }
  }
}

async function checkAndRoute() {
  if (subtitleEl) subtitleEl.textContent = "检测中…";
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

  cachedGatewayState = "unknown";
  showInstaller();
  setStage("等待开始…");
  setProgress(0);
  return false;
}

async function runOpenclaw(args, { stageLabel }) {
  if (taskRunning) return;
  showOperationModal(stageLabel || "执行中…");
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

async function runOpenclawSequence(steps, { stageLabel }) {
  if (taskRunning) return false;
  const list = Array.isArray(steps) ? steps.filter(Boolean) : [];
  if (list.length === 0) return false;

  showOperationModal(stageLabel || "执行中…");
  setTaskRunning(true);
  logEl.textContent = "";
  setStage(stageLabel || "执行中…");
  setProgress(0.05);

  try {
    for (let i = 0; i < list.length; i += 1) {
      const step = list[i] || {};
      const args = Array.isArray(step.args) ? step.args : [];
      const title = step.stageLabel || stageLabel || "执行中…";
      setStage(title);
      setProgress(Math.min(0.95, 0.1 + (i / Math.max(1, list.length)) * 0.8));
      if (step.logLine) appendLog(step.logLine);
      await installer.runOpenclaw(args);
    }
    setStage("完成");
    setProgress(1);
    return true;
  } catch (error) {
    const message = error?.message || String(error);
    setStage("失败");
    appendLog(`[错误] ${message}`);
    showLogsCheckbox.checked = true;
    updateLogVisibility();
    await rerouteIfOpenclawMissing(error);
    return false;
  } finally {
    setTaskRunning(false);
    await checkAndRoute();
  }
}

showLogsCheckbox.addEventListener("change", () => updateLogVisibility());

if (operationCloseBtn) {
  operationCloseBtn.addEventListener("click", () => hideOperationModal());
}

if (operationOverlay) {
  operationOverlay.addEventListener("click", (event) => {
    if (event.target === operationOverlay) hideOperationModal();
  });
}

window.addEventListener("keydown", (event) => {
  if (event.key !== "Escape") return;
  if (!isOperationModalOpen()) return;
  if (modalOverlay && !modalOverlay.classList.contains("hidden")) return;
  hideOperationModal();
});

if (cfgDefaultModel) {
  cfgDefaultModel.addEventListener("input", () => updateProviderPreview());
}

if (cfgGatewayBind) {
  cfgGatewayBind.addEventListener("change", () => updateGatewayBindVisibility());
}

if (cfgGatewayAuthMode) {
  cfgGatewayAuthMode.addEventListener("change", () => updateGatewayAuthVisibility());
}

if (workspaceMarkdownExpandBtn) {
  workspaceMarkdownExpandBtn.addEventListener("click", async () => {
    await withButtonLoading(workspaceMarkdownExpandBtn, async () => {
      expandedWorkspaceMarkdownNames = new Set(Array.from(workspaceMarkdownEditors.keys()));
      workspaceMarkdownEditors.forEach((editor) => {
        if (editor?.details) editor.details.open = true;
      });
      updateWorkspaceMarkdownToolbarAvailability();
    }, { minimumMs: 180 });
  });
}

if (workspaceMarkdownCollapseBtn) {
  workspaceMarkdownCollapseBtn.addEventListener("click", async () => {
    await withButtonLoading(workspaceMarkdownCollapseBtn, async () => {
      expandedWorkspaceMarkdownNames = new Set();
      workspaceMarkdownEditors.forEach((editor) => {
        if (editor?.details) editor.details.open = false;
      });
      updateWorkspaceMarkdownToolbarAvailability();
    }, { minimumMs: 180 });
  });
}

const requestCancelTask = async () => {
  if (!taskRunning) return;
  showOperationModal("取消当前任务…");
  showLogsCheckbox.checked = true;
  updateLogVisibility();
  appendLog("[ui] 请求取消任务…");
  setStage("取消中…");
  try {
    await installer.cancelTask();
  } catch (error) {
    appendLog(`[错误] 取消失败：${error?.message || String(error)}`);
  }
};

stopBtn.addEventListener("click", async () => {
  await withButtonLoading(stopBtn, requestCancelTask, { minimumMs: 320 });
});
cancelBtn.addEventListener("click", async () => {
  await withButtonLoading(cancelBtn, requestCancelTask, { minimumMs: 320 });
});

installBtn.addEventListener("click", async () => {
  await withButtonLoading(installBtn, async () => {
    if (taskRunning) return;
    showOperationModal("安装 OpenClaw…");
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
});

refreshBtn.addEventListener("click", async () => {
  await withButtonLoading(refreshBtn, async () => {
    await checkAndRoute();
  });
});

openDashboardBtn.addEventListener("click", async () => {
  await withButtonLoading(openDashboardBtn, async () => {
    if (taskRunning) return;
    setTaskRunning(true);
    try {
      // Always use `openclaw dashboard --no-open` because it appends the token to the URL.
      await installer.openDashboard();
    } catch (error) {
      showOperationModal("打开 Dashboard…");
      appendLog(`[错误] ${error?.message || String(error)}`);
      showLogsCheckbox.checked = true;
      updateLogVisibility();
      await rerouteIfOpenclawMissing(error);
    } finally {
      setTaskRunning(false);
    }
  });
});

if (openConfigBtn) {
  openConfigBtn.addEventListener("click", async () => {
    await withButtonLoading(openConfigBtn, async () => {
      if (taskRunning) return;
      if (subtitleEl) subtitleEl.textContent = "系统配置";
      dashboardCard.classList.add("hidden");
      configCard.classList.remove("hidden");
      await loadConfigValues();
    });
  });
}

if (configCancelBtn) {
  configCancelBtn.addEventListener("click", async () => {
    await withButtonLoading(configCancelBtn, async () => {
      if (taskRunning) return;
      configLoadToken += 1;
      setConfigLoading(false);
      await checkAndRoute();
    });
  });
}

if (configSaveBtn) {
  configSaveBtn.addEventListener("click", async () => {
    await withButtonLoading(configSaveBtn, async () => {
      if (taskRunning) return;
      showOperationModal("保存配置…");
      logEl.textContent = "";
      showLogsCheckbox.checked = true;
      updateLogVisibility();

      setStage("准备保存配置…");
      setProgress(0.05);

      try {
        const newModel = getInputValue(cfgDefaultModel);
        const newProviderApi = getInputValue(cfgProviderApi);
        const newBaseUrl = getInputValue(cfgBaseUrl);
        const newApiKey = getInputValue(cfgApiKey);
        const newContextWindow = getInputValue(cfgContextWindow);
        const newMaxTokens = getInputValue(cfgMaxTokens);
        const newWorkspace = getInputValue(cfgWorkspace);
        const newMaxConcurrent = getInputValue(cfgMaxConcurrent);
        const newSubagentsMaxConcurrent = getInputValue(cfgSubagentsMaxConcurrent);
        const newCompactionMode = getInputValue(cfgCompactionMode);
        const newGatewayMode = getInputValue(cfgGatewayMode);
        const newGatewayBind = getInputValue(cfgGatewayBind);
        const newGatewayCustomBindHost = getInputValue(cfgGatewayCustomBindHost);
        const newGatewayPort = getInputValue(cfgGatewayPort);
        const newGatewayAuthMode = getInputValue(cfgGatewayAuthMode);
        const newGatewayAuthToken = getInputValue(cfgGatewayAuthToken);
        const newGatewayAuthPassword = getInputValue(cfgGatewayAuthPassword);
        const newGatewayTrustedProxies = getInputValue(cfgGatewayTrustedProxies);
        const newGatewayTrustedProxyUserHeader = getInputValue(cfgGatewayTrustedProxyUserHeader);
        const newGatewayTrustedProxyRequiredHeaders = getInputValue(cfgGatewayTrustedProxyRequiredHeaders);
        const newGatewayTrustedProxyAllowUsers = getInputValue(cfgGatewayTrustedProxyAllowUsers);

        const targetModelRef = newModel || loadedConfigValues.defaultModel || "";
        const { providerId: targetProviderId } = parseModelRef(targetModelRef);
        const providerFieldsChanged =
          (newProviderApi && newProviderApi !== loadedConfigValues.providerApi) ||
          (newBaseUrl && newBaseUrl !== loadedConfigValues.baseUrl) ||
          Boolean(newApiKey) ||
          (newContextWindow && newContextWindow !== loadedConfigValues.contextWindow) ||
          (newMaxTokens && newMaxTokens !== loadedConfigValues.maxTokens);

        if (!targetProviderId && providerFieldsChanged) {
          throw new Error("默认模型需要是 `provider/model` 格式，才能保存 Provider 相关配置。");
        }

        const targetProviderContext = targetProviderId
          ? resolveProviderContext(targetModelRef)
          : { providerId: "", modelId: "", modelIndex: -1 };

        const seq = [];
        const deferredNotices = [];

        if (newGatewayBind === "custom" && !newGatewayCustomBindHost && !loadedConfigValues.gatewayCustomBindHost) {
          throw new Error("监听地址选择 custom 时，必须填写“自定义监听 Host”。");
        }

        if (newGatewayAuthMode === "token" && !newGatewayAuthToken && !loadedConfigValues.hasGatewayAuthToken) {
          throw new Error("鉴权模式为 token 时，当前还没有已保存令牌，请填写“鉴权令牌”。");
        }

        if (newGatewayAuthMode === "password" && !newGatewayAuthPassword && !loadedConfigValues.hasGatewayAuthPassword) {
          throw new Error("鉴权模式为 password 时，当前还没有已保存密码，请填写“鉴权密码”。");
        }

        if (newGatewayAuthMode === "trusted-proxy") {
          if (parseStringListInput(newGatewayTrustedProxies).length === 0) {
            throw new Error("鉴权模式为 trusted-proxy 时，请至少填写一个“可信代理 IP / CIDR”。");
          }
          if (!newGatewayTrustedProxyUserHeader) {
            throw new Error("鉴权模式为 trusted-proxy 时，必须填写“用户身份 Header”。");
          }
        }

        queueTextConfigSet(seq, {
          label: "默认模型",
          path: "agents.defaults.model.primary",
          value: newModel,
          currentValue: loadedConfigValues.defaultModel,
          stageLabel: "更新默认模型…"
        });

        if (targetProviderId) {
          const providerBasePath = `models.providers.${targetProviderId}`;
          queueTextConfigSet(seq, {
            label: "Provider API 模式",
            path: `${providerBasePath}.api`,
            value: newProviderApi,
            currentValue: loadedConfigValues.providerApi,
            stageLabel: "更新 Provider API 模式…"
          });
          queueTextConfigSet(seq, {
            label: "Base URL",
            path: `${providerBasePath}.baseUrl`,
            value: newBaseUrl,
            currentValue: loadedConfigValues.baseUrl,
            stageLabel: "更新 Base URL…"
          });
          queueTextConfigSet(seq, {
            label: "API Key",
            path: `${providerBasePath}.apiKey`,
            value: newApiKey,
            stageLabel: "保存 API Key…",
            strictJson: true,
            secret: true
          });

          const hasModelTuningChange =
            (newContextWindow && newContextWindow !== loadedConfigValues.contextWindow) ||
            (newMaxTokens && newMaxTokens !== loadedConfigValues.maxTokens);

          if (targetProviderContext.modelIndex >= 0) {
            const modelBasePath = `${providerBasePath}.models[${targetProviderContext.modelIndex}]`;
            queueIntegerConfigSet(seq, {
              label: "Context Window",
              path: `${modelBasePath}.contextWindow`,
              rawValue: newContextWindow,
              currentValue: loadedConfigValues.contextWindow,
              stageLabel: "更新 Context Window…"
            });
            queueIntegerConfigSet(seq, {
              label: "Max Tokens",
              path: `${modelBasePath}.maxTokens`,
              rawValue: newMaxTokens,
              currentValue: loadedConfigValues.maxTokens,
              stageLabel: "更新 Max Tokens…"
            });
          } else if (hasModelTuningChange) {
            deferredNotices.push("[警告] 当前默认模型未在 provider.models 中注册，已跳过 Context Window / Max Tokens。");
          }
        }

        queueTextConfigSet(seq, {
          label: "默认工作目录",
          path: "agents.defaults.workspace",
          value: newWorkspace,
          currentValue: loadedConfigValues.workspace,
          stageLabel: "更新默认工作目录…"
        });
        queueIntegerConfigSet(seq, {
          label: "最大并发",
          path: "agents.defaults.maxConcurrent",
          rawValue: newMaxConcurrent,
          currentValue: loadedConfigValues.maxConcurrent,
          stageLabel: "更新最大并发…"
        });
        queueIntegerConfigSet(seq, {
          label: "子代理最大并发",
          path: "agents.defaults.subagents.maxConcurrent",
          rawValue: newSubagentsMaxConcurrent,
          currentValue: loadedConfigValues.subagentsMaxConcurrent,
          stageLabel: "更新子代理并发…"
        });
        queueTextConfigSet(seq, {
          label: "压缩模式",
          path: "agents.defaults.compaction.mode",
          value: newCompactionMode,
          currentValue: loadedConfigValues.compactionMode,
          stageLabel: "更新压缩模式…"
        });
        queueTextConfigSet(seq, {
          label: "网关模式",
          path: "gateway.mode",
          value: newGatewayMode,
          currentValue: loadedConfigValues.gatewayMode,
          stageLabel: "更新网关模式…"
        });
        queueTextConfigSet(seq, {
          label: "监听地址",
          path: "gateway.bind",
          value: newGatewayBind,
          currentValue: loadedConfigValues.gatewayBind,
          stageLabel: "更新监听地址…"
        });
        queueTextConfigSet(seq, {
          label: "自定义监听 Host",
          path: "gateway.customBindHost",
          value: newGatewayCustomBindHost,
          currentValue: loadedConfigValues.gatewayCustomBindHost,
          stageLabel: "更新自定义监听 Host…"
        });
        queueIntegerConfigSet(seq, {
          label: "网关端口",
          path: "gateway.port",
          rawValue: newGatewayPort,
          currentValue: loadedConfigValues.gatewayPort,
          stageLabel: "更新网关端口…",
          max: 65535
        });
        queueTextConfigSet(seq, {
          label: "鉴权模式",
          path: "gateway.auth.mode",
          value: newGatewayAuthMode,
          currentValue: loadedConfigValues.gatewayAuthMode,
          stageLabel: "更新鉴权模式…"
        });
        queueTextConfigSet(seq, {
          label: "网关令牌",
          path: "gateway.auth.token",
          value: newGatewayAuthToken,
          stageLabel: "保存网关令牌…",
          strictJson: true,
          secret: true
        });
        queueTextConfigSet(seq, {
          label: "网关密码",
          path: "gateway.auth.password",
          value: newGatewayAuthPassword,
          stageLabel: "保存网关密码…",
          strictJson: true,
          secret: true
        });
        queueStringArrayConfigSet(seq, {
          label: "可信代理 IP / CIDR",
          path: "gateway.trustedProxies",
          rawValue: newGatewayTrustedProxies,
          currentValue: loadedConfigValues.gatewayTrustedProxies,
          stageLabel: "更新可信代理列表…"
        });
        queueTextConfigSet(seq, {
          label: "用户身份 Header",
          path: "gateway.auth.trustedProxy.userHeader",
          value: newGatewayTrustedProxyUserHeader,
          currentValue: loadedConfigValues.gatewayTrustedProxyUserHeader,
          stageLabel: "更新用户身份 Header…"
        });
        queueStringArrayConfigSet(seq, {
          label: "必需 Header",
          path: "gateway.auth.trustedProxy.requiredHeaders",
          rawValue: newGatewayTrustedProxyRequiredHeaders,
          currentValue: loadedConfigValues.gatewayTrustedProxyRequiredHeaders,
          stageLabel: "更新必需 Header…"
        });
        queueStringArrayConfigSet(seq, {
          label: "允许用户列表",
          path: "gateway.auth.trustedProxy.allowUsers",
          rawValue: newGatewayTrustedProxyAllowUsers,
          currentValue: loadedConfigValues.gatewayTrustedProxyAllowUsers,
          stageLabel: "更新允许用户列表…"
        });

        if (newGatewayMode === "remote") {
          deferredNotices.push("[警告] `gateway.mode=remote` 通常还需要补充 `gateway.remote.*`，当前页面暂未覆盖这些字段。");
        }

        const markdownChanges = collectWorkspaceMarkdownChanges();
        deferredNotices.forEach((line) => appendLog(line));

        if (seq.length === 0 && markdownChanges.length === 0) {
          setStage("等待开始…");
          setProgress(0);
          appendLog("[ui] 没有检测到新的配置或 Markdown 变更。");
          return;
        }

        let configSaved = false;
        if (seq.length > 0) {
          const ok = await runOpenclawSequence(seq, { stageLabel: "保存配置" });
          if (!ok) return;
          configSaved = true;
        }

        let markdownSaved = false;
        if (markdownChanges.length > 0) {
          const ok = await runWorkspaceMarkdownSaveSequence(markdownChanges, { stageLabel: "保存 Workspace Markdown…" });
          if (!ok) return;
          markdownSaved = true;
        }

        if (cfgApiKey) cfgApiKey.value = "";
        if (cfgGatewayAuthToken) cfgGatewayAuthToken.value = "";
        if (cfgGatewayAuthPassword) cfgGatewayAuthPassword.value = "";
        if (!configCard.classList.contains("hidden")) {
          await loadConfigValues();
        }

        if (configSaved) {
          appendLog(markdownSaved ? "[ui] 配置和 Workspace Markdown 已成功写入。网关可能需要重启以应用新规则。" : "[ui] 配置已成功写入。网关可能需要重启以应用新规则。");
          confirmModal({
            title: "配置已保存",
            body: markdownSaved
              ? "配置项与 Workspace Markdown 已成功保存。要使网关相关配置实时生效，你可能需要重启本地网关服务。是否立即执行？"
              : "配置项已成功保存。要使其实时生效（如模型推理接口），你需要重启本地网关联接服务。是否立即执行？",
            confirmText: "尝试重启网关",
            cancelText: "稍后"
          }).then(async (confirmed) => {
            if (confirmed && gatewayRestartBtn && !gatewayRestartBtn.disabled) {
              gatewayRestartBtn.click();
            }
          });
        } else if (markdownSaved) {
          appendLog("[ui] Workspace Markdown 已成功保存。新会话或后续上下文加载时会生效。");
        }
      } catch (error) {
        setStage("失败");
        appendLog(`[错误] ${error?.message || String(error)}`);
      }
    });
  });
}

async function buildGatewayEnsureStartedSteps({ includeStop = false } = {}) {
  const isWindows = /windows/i.test(navigator.userAgent || "");

  const steps = [];
  let status = null;
  let looksMissing = false;

  if (!isWindows) {
    try {
      status = await installer.getGatewayStatus();
      const raw = String(status?.raw || "");
      looksMissing = /scheduled task\s*\(missing\)|gateway service missing|service missing/i.test(raw);
    } catch {
      // ignore and let the actual commands attempt run
    }
  }

  const requiresInstall = !isWindows && (status?.state === "not_installed" || looksMissing);

  if (includeStop && !requiresInstall) {
    steps.push({ args: ["gateway", "stop"], stageLabel: "停止网关服务…" });
  }
  if (requiresInstall) {
    steps.push({ args: ["gateway", "install"], stageLabel: "安装网关服务…" });
  }
  steps.push({ args: ["gateway", "start"], stageLabel: "启动网关服务…" });

  return steps;
}

gatewayStartBtn.addEventListener("click", async () => {
  await withButtonLoading(gatewayStartBtn, async () => {
    const steps = await buildGatewayEnsureStartedSteps();
    await runOpenclawSequence(steps, { stageLabel: "启动网关服务…" });
    await refreshGatewayStatus();
  });
});

gatewayStopBtn.addEventListener("click", async () => {
  await withButtonLoading(gatewayStopBtn, async () => {
    await runOpenclaw(["gateway", "stop"], { stageLabel: "停止网关服务…" });
  });
});

gatewayRestartBtn.addEventListener("click", async () => {
  await withButtonLoading(gatewayRestartBtn, async () => {
    const steps = await buildGatewayEnsureStartedSteps({ includeStop: true });
    await runOpenclawSequence(steps, { stageLabel: "重启网关服务…" });
    await refreshGatewayStatus();
  });
});

doctorBtn.addEventListener("click", async () => {
  await withButtonLoading(doctorBtn, async () => {
    await runOpenclaw(["doctor", "--fix", "--yes", "--non-interactive"], {
      stageLabel: "健康检查/修复…"
    });
  });
});

updateBtn.addEventListener("click", async () => {
  await withButtonLoading(updateBtn, async () => {
    const channel = updateChannel.value || "stable";
    await runOpenclaw(["update", "--channel", channel, "--yes"], { stageLabel: "更新中…" });
  });
});

if (uninstallBtn) {
  uninstallBtn.addEventListener("click", async () => {
    await withButtonLoading(uninstallBtn, async () => {
      if (taskRunning) return;
      showLogsCheckbox.checked = true;
      updateLogVisibility();
      appendLog("[ui] 点击卸载");
      const confirmed = await confirmModal({
        title: "确认卸载",
        body: "将执行 openclaw 卸载（service/state/workspace），并尝试自动移除 CLI（brew / npm / pnpm / nvm；Windows: nvm-windows / npm shim）。是否继续？",
        confirmText: "继续卸载",
        cancelText: "取消"
      });
      if (!confirmed) {
        appendLog("[ui] 用户取消卸载");
        return;
      }

      showOperationModal("卸载 OpenClaw…");
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
  });
}

let lastProgressMarker = null;
const escapeMini = (value) =>
  String(value ?? "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
installer.onProgress((payload) => {
  const percent = payload?.percent;
  if (percent != null) setProgress(percent);

  if (payload?.stage === "done") {
    setStage("完成");
    return;
  }

  if (payload?.index && payload?.total) {
    setStage(`安装中…（${payload.index}/${payload.total}）`);

    const title = payload?.title ? String(payload.title) : "进行中…";
    const marker = `${payload.index}/${payload.total}:${payload.stage || ""}:${title}`;
    if (marker !== lastProgressMarker) {
      lastProgressMarker = marker;
      const stepLine = `==> [${payload.index}/${payload.total}] ${title}`;
      logEl.insertAdjacentHTML("beforeend", `<div class="logStep">${escapeMini(stepLine)}</div>`);
      logEl.scrollTop = logEl.scrollHeight;
    }
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
