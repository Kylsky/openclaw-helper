const path = require("node:path");
const { spawn } = require("node:child_process");
const { app, BrowserWindow, ipcMain, shell } = require("electron");
const {
  runInstall,
  getOpenclawInfo,
  resolveOpenclawExecution,
  runOpenclawCommand,
  runOpenclawCommandCollect,
  getGatewayStatus,
  uninstallOpenclaw
} = require("./runtime/install");

let mainWindow = null;
let currentTaskController = null;

function createWindow() {
  mainWindow = new BrowserWindow({
    width: 860,
    height: 640,
    minWidth: 760,
    minHeight: 560,
    show: false,
    webPreferences: {
      preload: path.join(__dirname, "..", "preload", "preload.js"),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: false
    }
  });

  mainWindow.once("ready-to-show", () => mainWindow.show());

  const indexHtmlPath = path.join(__dirname, "..", "renderer", "index.html");
  void mainWindow.loadFile(indexHtmlPath);

  mainWindow.on("closed", () => {
    mainWindow = null;
  });
}

app.whenReady().then(() => {
  createWindow();

  app.on("activate", () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow();
  });
});

app.on("window-all-closed", () => {
  if (process.platform !== "darwin") app.quit();
});

ipcMain.handle("start-install", async (event, options) => {
  if (currentTaskController) {
    throw new Error("已有任务正在运行，请先取消或等待完成。");
  }

  const controller = new AbortController();
  currentTaskController = controller;

  const sendProgress = (payload) => {
    event.sender.send("install-progress", payload);
  };

  const sendLog = (message) => {
    event.sender.send("install-log", { message, ts: Date.now() });
  };

  try {
    const result = await runInstall({
      signal: controller.signal,
      options,
      onProgress: sendProgress,
      onLog: sendLog
    });
    return result;
  } finally {
    currentTaskController = null;
  }
});

ipcMain.handle("cancel-task", async () => {
  if (!currentTaskController) return;
  currentTaskController.abort("用户取消");
});

ipcMain.handle("check-openclaw", async () => {
  return await getOpenclawInfo({ withHelp: false });
});

ipcMain.handle("get-gateway-status", async () => {
  return await getGatewayStatus();
});

function assertHttpUrl(value) {
  const url = new URL(String(value));
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new Error("只允许打开 http/https 链接");
  }
  return url.toString();
}

ipcMain.handle("open-external", async (_event, payload) => {
  const url = assertHttpUrl(payload?.url);
  await shell.openExternal(url);
  return { ok: true };
});

ipcMain.handle("open-dashboard", async () => {
  const result = await getOpenclawInfo({ withHelp: false });
  if (!result?.installed) throw new Error("未检测到 openclaw，请先完成安装。");

  const run = await runOpenclawCommandCollect({ args: ["dashboard", "--no-open"] });
  const combined = `${run.stdout ?? ""}\n${run.stderr ?? ""}`;
  const match = combined.match(/https?:\/\/\S+/i);
  if (!match) throw new Error("未获取到控制台链接，请先启动网关或完成配置。");

  const url = assertHttpUrl(match[0]);
  await shell.openExternal(url);
  return { ok: true, url };
});

ipcMain.handle("open-wizard", async () => {
  const result = await getOpenclawInfo({ withHelp: false });
  if (!result?.installed) throw new Error("未检测到 openclaw，请先完成安装。");

  const resolved = await resolveOpenclawExecution();
  const openclawCmd = resolved?.command || "openclaw";

  if (process.platform === "darwin") {
    const escapeAppleScript = (value) => String(value).replace(/\\/g, "\\\\").replace(/"/g, '\\"');
    const shellCmd = openclawCmd.includes(" ") ? `"${openclawCmd}" onboard` : `${openclawCmd} onboard`;
    const script = [
      'tell application "Terminal"',
      'activate',
      `do script "${escapeAppleScript(shellCmd)}"`,
      "end tell"
    ].join("\n");

    spawn("/usr/bin/osascript", ["-e", script], {
      windowsHide: true,
      stdio: ["ignore", "ignore", "ignore"],
      detached: true
    }).unref();
    return { ok: true };
  }

  if (process.platform === "win32") {
    const quoted = openclawCmd.includes(" ") ? `"${openclawCmd}"` : openclawCmd;
    spawn("cmd.exe", ["/d", "/s", "/c", `start "" cmd.exe /k ${quoted} onboard`], {
      windowsHide: true,
      stdio: ["ignore", "ignore", "ignore"],
      detached: true
    }).unref();
    return { ok: true };
  }

  throw new Error("当前平台暂不支持自动打开配置向导终端窗口。");
});

ipcMain.handle("run-openclaw", async (event, payload) => {
  if (currentTaskController) {
    throw new Error("已有任务正在运行，请先取消或等待完成。");
  }

  const controller = new AbortController();
  currentTaskController = controller;

  const sendLog = (message) => {
    event.sender.send("openclaw-log", { message, ts: Date.now() });
  };

  try {
    const result = await getOpenclawInfo({ withHelp: false });
    if (!result?.installed) throw new Error("未检测到 openclaw，请先完成安装。");

    const args = Array.isArray(payload?.args) ? payload.args.map((x) => String(x)) : [];
    if (args.length === 0) throw new Error("缺少 openclaw 参数");

    const run = await runOpenclawCommand({
      signal: controller.signal,
      args,
      onLog: sendLog
    });

    return run;
  } finally {
    currentTaskController = null;
  }
});

ipcMain.handle("uninstall-openclaw", async (event) => {
  if (currentTaskController) {
    throw new Error("已有任务正在运行，请先取消或等待完成。");
  }

  const controller = new AbortController();
  currentTaskController = controller;

  const sendLog = (message) => {
    event.sender.send("openclaw-log", { message, ts: Date.now() });
  };

  try {
    return await uninstallOpenclaw({ signal: controller.signal, onLog: sendLog });
  } finally {
    currentTaskController = null;
  }
});
