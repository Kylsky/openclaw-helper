/* global window */

function assertTauri() {
  const tauri = window.__TAURI__;
  const invoke = tauri?.core?.invoke;
  const listen = tauri?.event?.listen;
  if (!invoke || !listen) {
    throw new Error("未检测到 Tauri 运行环境。请使用 `npm run dev` 启动 Tauri 应用。");
  }
  return { tauri, invoke, listen };
}

function createListener(eventName, callback) {
  const { listen } = assertTauri();
  const unlistenPromise = listen(eventName, (event) => callback(event.payload));
  return () => {
    void unlistenPromise.then((unlisten) => unlisten());
  };
}

window.installer = {
  checkOpenclaw: async () => {
    const { invoke } = assertTauri();
    return await invoke("check_openclaw", {});
  },
  getGatewayStatus: async () => {
    const { invoke } = assertTauri();
    return await invoke("get_gateway_status", {});
  },
  startInstall: async (options) => {
    const { invoke } = assertTauri();
    return await invoke("start_install", { options: options ?? {} });
  },
  cancelTask: async () => {
    const { invoke } = assertTauri();
    return await invoke("cancel_task", {});
  },
  runOpenclaw: async (args) => {
    const { invoke } = assertTauri();
    const finalArgs = Array.isArray(args) ? args.map((x) => String(x)) : [];
    return await invoke("run_openclaw", { args: finalArgs });
  },
  openDashboard: async () => {
    const { invoke } = assertTauri();
    const url = await invoke("open_dashboard", {});
    if (url) await window.installer.openExternal(url);
    return { ok: true, url };
  },
  openWizard: async () => {
    const { invoke } = assertTauri();
    return await invoke("open_wizard", {});
  },
  uninstallOpenclaw: async () => {
    const { invoke } = assertTauri();
    return await invoke("uninstall_openclaw", {});
  },
  openExternal: async (url) => {
    const { tauri } = assertTauri();
    const shell = tauri?.shell;
    if (!shell?.open) throw new Error("未启用 tauri shell 插件，无法打开外部链接。");
    await shell.open(String(url));
    return { ok: true };
  },
  onProgress: (callback) => createListener("install-progress", callback),
  onLog: (callback) => createListener("install-log", callback),
  onOpenclawLog: (callback) => createListener("openclaw-log", callback)
};
