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
  getTaskStatus: async () => {
    const { invoke } = assertTauri();
    return await invoke("get_task_status", {});
  },
  runOpenclaw: async (args) => {
    const { invoke } = assertTauri();
    const finalArgs = Array.isArray(args) ? args.map((x) => String(x)) : [];
    return await invoke("run_openclaw", { args: finalArgs });
  },
  updateOpenclaw: async (channel) => {
    const { invoke } = assertTauri();
    return await invoke("update_openclaw", { channel: String(channel || "stable") });
  },
  execOpenclawCollect: async (args) => {
    const { invoke } = assertTauri();
    const finalArgs = Array.isArray(args) ? args.map((x) => String(x)) : [];
    return await invoke("exec_openclaw_collect", { args: finalArgs });
  },
  loadConfigCenterData: async () => {
    const { invoke } = assertTauri();
    return await invoke("load_config_center_data", {});
  },
  loadWorkspaceMarkdowns: async () => {
    const { invoke } = assertTauri();
    return await invoke("load_workspace_markdowns", {});
  },
  saveWorkspaceMarkdown: async (name, content) => {
    const { invoke } = assertTauri();
    return await invoke("save_workspace_markdown", {
      name: String(name),
      content: String(content ?? "")
    });
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
  runWeixinConfig: async () => {
    const { invoke } = assertTauri();
    return await invoke("run_weixin_config", {});
  },
  openExternal: async (url) => {
    const { invoke } = assertTauri();
    await invoke("open_external", { url: String(url) });
    return { ok: true, url: String(url) };
  },
  onProgress: (callback) => createListener("install-progress", callback),
  onLog: (callback) => createListener("install-log", callback),
  onOpenclawLog: (callback) => createListener("openclaw-log", callback)
};
