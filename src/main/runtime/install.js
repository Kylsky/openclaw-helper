const { spawn } = require("node:child_process");
const fssync = require("node:fs");
const fs = require("node:fs/promises");
const path = require("node:path");
const os = require("node:os");

const DEFAULT_OPENCLAW_PACKAGE = "openclaw";
const MIN_NODE_MAJOR = 22;

function assertNotAborted(signal) {
  if (!signal) return;
  if (!signal.aborted) return;
  const reason = signal.reason ?? "安装已取消";
  throw new Error(typeof reason === "string" ? reason : "安装已取消");
}

function splitLines(text) {
  return String(text)
    .replace(/\r\n/g, "\n")
    .split("\n")
    .filter((line) => line.length > 0);
}

function mergePathEntries({ existingPath, extraEntries, separator, extraFirst = false }) {
  const existing = String(existingPath ?? "");
  const incoming = Array.isArray(extraEntries) ? extraEntries.filter(Boolean) : [];

  const seen = new Set();
  const normalized = [];

  const push = (entry) => {
    const trimmed = String(entry).trim();
    if (!trimmed) return;
    const key = process.platform === "win32" ? trimmed.toLowerCase() : trimmed;
    if (seen.has(key)) return;
    seen.add(key);
    normalized.push(trimmed);
  };

  if (extraFirst) {
    for (const entry of incoming) push(entry);
    for (const entry of existing.split(separator)) push(entry);
  } else {
    for (const entry of existing.split(separator)) push(entry);
    for (const entry of incoming) push(entry);
  }

  return normalized.join(separator);
}

function createBaseEnv() {
  const env = { ...process.env };

  delete env.npm_config_prefix;

  if (process.platform !== "win32") {
    const extra = [
      "/opt/homebrew/bin",
      "/usr/local/bin",
      path.join(os.homedir(), ".local", "bin"),
      "/usr/bin",
      "/bin",
      "/usr/sbin",
      "/sbin"
    ];
    // Prefer well-known system/brew locations over user shell shims (e.g. nvm),
    // to make the installer deterministic even when the parent shell modifies PATH.
    env.PATH = mergePathEntries({ existingPath: env.PATH, extraEntries: extra, separator: ":", extraFirst: true });
  }

  return env;
}

function safeUrl(url) {
  const parsed = new URL(url);
  if (parsed.protocol !== "https:" && parsed.protocol !== "http:") {
    throw new Error("只允许 http/https 链接");
  }
  return parsed.toString();
}

function validateNpmPackageName(name) {
  const value = String(name ?? "").trim();
  if (!value) throw new Error("openclaw 包名不能为空");
  if (value.length > 214) throw new Error("openclaw 包名太长");
  if (/\s/.test(value)) throw new Error("openclaw 包名不能包含空格");
  if (!/^[a-z0-9@/._-]+$/i.test(value)) {
    throw new Error("openclaw 包名包含不支持的字符");
  }
  if (value.includes("..")) throw new Error("openclaw 包名不合法");
  if (value.includes("//")) throw new Error("openclaw 包名不合法");
  if (value.includes("\\")) throw new Error("openclaw 包名不合法");
  if (value.includes("'") || value.includes("\"") || value.includes("`")) {
    throw new Error("openclaw 包名不合法");
  }
  return value;
}

function validateOptions(options) {
  const autoInstall = options?.autoInstall !== false;
  const openclawPackage = validateNpmPackageName(options?.openclawPackage ?? DEFAULT_OPENCLAW_PACKAGE);

  const nodeChannel = String(options?.nodeChannel ?? "lts").trim().toLowerCase();
  if (!["lts", "latest"].includes(nodeChannel)) {
    throw new Error("Node 安装通道无效（仅支持 lts / latest）");
  }

  const npmRegistry = options?.npmRegistry ? safeUrl(options.npmRegistry) : null;

  return {
    autoInstall,
    openclawPackage,
    nodeChannel,
    npmRegistry
  };
}

function findBrewPath() {
  const candidates = ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"];
  for (const candidate of candidates) {
    try {
      if (fssync.existsSync(candidate)) return candidate;
    } catch {
      // ignore
    }
  }
  return null;
}

async function runProcess({
  command,
  args = [],
  env,
  cwd,
  signal,
  onLog,
  collectOutput = false,
  shell = false
}) {
  assertNotAborted(signal);

  return await new Promise((resolve, reject) => {
    let settled = false;
    const finishResolve = (value) => {
      if (settled) return;
      settled = true;
      resolve(value);
    };
    const finishReject = (error) => {
      if (settled) return;
      settled = true;
      reject(error);
    };

    const child = spawn(command, args, {
      env,
      cwd,
      shell,
      windowsHide: true,
      detached: process.platform !== "win32",
      stdio: ["ignore", "pipe", "pipe"]
    });

    let stdout = "";
    let stderr = "";

    const write = (prefix, chunk) => {
      const text = chunk.toString("utf8");
      if (collectOutput) {
        if (prefix === "stdout") stdout += text;
        if (prefix === "stderr") stderr += text;
      }

      const lines = splitLines(text);
      for (const line of lines) onLog?.(line);
    };

    child.stdout.on("data", (chunk) => write("stdout", chunk));
    child.stderr.on("data", (chunk) => write("stderr", chunk));

    child.on("error", (error) => finishReject(error));

    child.on("close", (code, signalName) => {
      if (code === 0) {
        finishResolve({ code: 0, stdout, stderr });
        return;
      }

      const details = signalName ? `signal=${signalName}` : `code=${code}`;
      const error = new Error(`命令执行失败：${command} ${args.join(" ")} (${details})`);
      error.code = code;
      error.stdout = stdout;
      error.stderr = stderr;
      finishReject(error);
    });

    const abortHandler = () => {
      try {
        if (process.platform === "win32") {
          try {
            spawn("taskkill", ["/pid", String(child.pid), "/T", "/F"], {
              windowsHide: true,
              stdio: ["ignore", "ignore", "ignore"]
            });
          } catch {
            // ignore
          }
          try {
            child.kill("SIGTERM");
          } catch {
            // ignore
          }
        } else {
          try {
            process.kill(-child.pid, "SIGTERM");
          } catch {
            try {
              child.kill("SIGTERM");
            } catch {
              // ignore
            }
          }
        }
      } finally {
        finishReject(new Error("任务已取消"));
      }
    };

    if (signal) signal.addEventListener("abort", abortHandler, { once: true });
  });
}

async function runWindowsShim({ baseCommand, args, env, signal, onLog, collectOutput = false }) {
  if (process.platform !== "win32") {
    return await runProcess({ command: baseCommand, args, env, signal, onLog, collectOutput });
  }

  const candidates = [`${baseCommand}.cmd`, `${baseCommand}.exe`, baseCommand];
  let lastNotFound = null;

  for (const candidate of candidates) {
    try {
      return await runProcess({ command: candidate, args, env, signal, onLog, collectOutput });
    } catch (error) {
      if (error?.code === "ENOENT" || error?.code === "EINVAL") {
        lastNotFound = error;
        continue;
      }
      throw error;
    }
  }

  try {
    return await runProcess({ command: baseCommand, args, env, signal, onLog, shell: true, collectOutput });
  } catch (error) {
    throw lastNotFound ?? error;
  }
}

async function commandExists(command, { env, onLog, signal } = {}) {
  try {
    if (process.platform === "win32") {
      await runProcess({
        command: "where",
        args: [command],
        env,
        signal,
        onLog: onLog ? (line) => onLog(`[where] ${line}`) : null,
        collectOutput: false
      });
      return true;
    }

    await runProcess({
      command: "/usr/bin/which",
      args: [command],
      env,
      signal,
      onLog: onLog ? (line) => onLog(`[which] ${line}`) : null,
      collectOutput: false
    });
    return true;
  } catch {
    return false;
  }
}

function expandWindowsEnvVars(value, env) {
  return String(value).replace(/%([^%]+)%/g, (_m, name) => env?.[name] ?? `%${name}%`);
}

function parseRegQueryValue(output) {
  const lines = splitLines(output);
  for (const line of lines) {
    const parts = line.trim().split(/\s{2,}/g);
    if (parts.length >= 3) {
      return parts.slice(2).join("  ");
    }
  }
  return null;
}

async function getWindowsRegistryPathValue(hive, { env, signal }) {
  try {
    const result = await runProcess({
      command: "reg",
      args: ["query", hive, "/v", "Path"],
      env,
      signal,
      collectOutput: true
    });
    return parseRegQueryValue(result.stdout) ?? parseRegQueryValue(result.stderr) ?? null;
  } catch {
    return null;
  }
}

async function refreshWindowsPath({ env, signal, onLog }) {
  if (process.platform !== "win32") return env;

  const baseEnv = env ?? { ...process.env };
  const userPath = await getWindowsRegistryPathValue("HKCU\\Environment", { env: baseEnv, signal });
  const machinePath = await getWindowsRegistryPathValue(
    "HKLM\\SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment",
    { env: baseEnv, signal }
  );

  const separator = ";";
  const merged = mergePathEntries({
    existingPath: baseEnv.PATH,
    extraEntries: [
      userPath ? expandWindowsEnvVars(userPath, baseEnv) : null,
      machinePath ? expandWindowsEnvVars(machinePath, baseEnv) : null
    ],
    separator
  });

  onLog?.("已刷新 Windows PATH（从注册表读取）");

  return { ...baseEnv, PATH: merged };
}

function parseSemverMajor(text) {
  const match = String(text ?? "").trim().match(/(\d+)\.(\d+)\.(\d+)/);
  if (!match) return null;
  return Number(match[1]);
}

function parseSemverTuple(text) {
  const match = String(text ?? "").trim().match(/(\d+)\.(\d+)\.(\d+)/);
  if (!match) return null;
  return [Number(match[1]), Number(match[2]), Number(match[3])];
}

function compareSemverTuple(a, b) {
  for (let i = 0; i < 3; i++) {
    const delta = (a?.[i] ?? 0) - (b?.[i] ?? 0);
    if (delta !== 0) return delta;
  }
  return 0;
}

function fileExistsSync(filePath) {
  try {
    return fssync.existsSync(filePath);
  } catch {
    return false;
  }
}

async function resolveNpmGlobalPrefix({ env, signal, onLog }) {
  const hasNpm = await commandExists("npm", { env, signal });
  if (!hasNpm) return null;

  const result =
    process.platform === "win32"
      ? await runWindowsShim({
          baseCommand: "npm",
          args: ["prefix", "-g"],
          env,
          signal,
          onLog: onLog ? (line) => onLog(`[npm] ${line}`) : null,
          collectOutput: true
        })
      : await runProcess({
          command: "npm",
          args: ["prefix", "-g"],
          env,
          signal,
          onLog: onLog ? (line) => onLog(`[npm] ${line}`) : null,
          collectOutput: true
        });

  const prefix = splitLines(`${result.stdout ?? ""}\n${result.stderr ?? ""}`)[0]?.trim();
  if (!prefix) return null;
  return prefix;
}

async function resolveOpenclawExecution({ env, signal, onLog } = {}) {
  const baseEnv = env ?? createBaseEnv();
  const resolvedEnv = process.platform === "win32" ? await refreshWindowsPath({ env: baseEnv, signal, onLog }) : baseEnv;

  // 1) Fast path: already in PATH.
  const hasOpenclaw = await commandExists("openclaw", { env: resolvedEnv, signal, onLog });
  if (hasOpenclaw) {
    try {
      const pathResult =
        process.platform === "win32"
          ? await runProcess({ command: "where", args: ["openclaw"], env: resolvedEnv, signal, collectOutput: true })
          : await runProcess({ command: "/usr/bin/which", args: ["openclaw"], env: resolvedEnv, signal, collectOutput: true });
      const first = splitLines(`${pathResult.stdout ?? ""}\n${pathResult.stderr ?? ""}`)[0]?.trim();
      if (first) return { command: first, env: resolvedEnv, source: "path" };
    } catch {
      // ignore and fall back
    }
    return { command: "openclaw", env: resolvedEnv, source: "path" };
  }

  // 2) Common macOS Homebrew prefixes.
  if (process.platform === "darwin") {
    const brewCandidates = ["/opt/homebrew/bin/openclaw", "/usr/local/bin/openclaw"];
    for (const candidate of brewCandidates) {
      if (fileExistsSync(candidate)) return { command: candidate, env: resolvedEnv, source: "brew_bin" };
    }
  }

  // 3) npm prefix -g => <prefix>/bin/openclaw
  try {
    const prefix = await resolveNpmGlobalPrefix({ env: resolvedEnv, signal, onLog });
    if (prefix) {
      const binDir = path.join(prefix, "bin");
      const candidate = process.platform === "win32" ? path.join(prefix, "openclaw.cmd") : path.join(binDir, "openclaw");

      const separator = process.platform === "win32" ? ";" : ":";
      const mergedPath = mergePathEntries({
        existingPath: resolvedEnv.PATH,
        extraEntries: process.platform === "win32" ? [prefix] : [binDir],
        separator
      });
      const envWithBin = { ...resolvedEnv, PATH: mergedPath };

      if (fileExistsSync(candidate)) return { command: candidate, env: envWithBin, source: "npm_prefix" };

      // If file isn't present but PATH includes it now, still attempt direct execution.
      const hasAfterPath = await commandExists("openclaw", { env: envWithBin, signal, onLog });
      if (hasAfterPath) return { command: "openclaw", env: envWithBin, source: "npm_prefix_path" };
    }
  } catch {
    // ignore
  }

  // 4) Backward-compat: older installer versions used nvm on macOS. Try locating ~/.nvm/.../bin/openclaw.
  if (process.platform === "darwin") {
    const nvmVersionsDir = path.join(os.homedir(), ".nvm", "versions", "node");
    try {
      const entries = await fs.readdir(nvmVersionsDir, { withFileTypes: true });
      const candidates = [];
      for (const entry of entries) {
        if (!entry.isDirectory()) continue;
        const name = entry.name;
        if (!name.startsWith("v")) continue;
        const version = parseSemverTuple(name.slice(1));
        if (!version) continue;
        const candidate = path.join(nvmVersionsDir, name, "bin", "openclaw");
        if (!fileExistsSync(candidate)) continue;
        candidates.push({ version, candidate });
      }

      candidates.sort((a, b) => compareSemverTuple(b.version, a.version));
      const best = candidates[0];
      if (best?.candidate) return { command: best.candidate, env: resolvedEnv, source: "nvm_scan" };
    } catch {
      // ignore
    }
  }

  return null;
}

async function getMacNvmSourcePath({ brewPath, env }) {
  const homeNvm = path.join(os.homedir(), ".nvm", "nvm.sh");
  if (fssync.existsSync(homeNvm)) return homeNvm;

  const brewOptCandidates = ["/opt/homebrew/opt/nvm/nvm.sh", "/usr/local/opt/nvm/nvm.sh"];
  for (const candidate of brewOptCandidates) {
    if (fssync.existsSync(candidate)) return candidate;
  }

  if (!brewPath) return null;

  try {
    const result = await runProcess({
      command: brewPath,
      args: ["--prefix", "nvm"],
      env,
      collectOutput: true
    });
    const prefix = splitLines(result.stdout)[0]?.trim();
    if (!prefix) return null;
    const candidate = path.join(prefix, "nvm.sh");
    if (fssync.existsSync(candidate)) return candidate;
  } catch {
    // ignore
  }

  return null;
}

async function ensureWinget({ env, signal }) {
  const exists = await commandExists("winget", { env, signal });
  if (!exists) {
    throw new Error(
      "未检测到 winget。请先安装 Microsoft App Installer（Windows 软件包管理器），再重试。"
    );
  }
}

async function ensureBrew({ brewPath, env }) {
  if (brewPath) return brewPath;
  const exists = await commandExists("brew", { env });
  if (exists) return "brew";
  throw new Error("未检测到 Homebrew。请先安装 Homebrew（brew.sh），再重试。");
}

async function ensureNvm({ env, signal, onLog, options, context }) {
  if (process.platform === "win32") {
    const hasNvm = await commandExists("nvm", { env, signal });
    if (hasNvm) {
      onLog?.("已检测到 nvm（Windows）");
      return;
    }

    if (!options.autoInstall) {
      throw new Error("未检测到 nvm。请先手动安装 nvm-windows，然后重试。");
    }

    await ensureWinget({ env, signal });
    onLog?.("正在通过 winget 安装 nvm-windows…");
    await runProcess({
      command: "winget",
      args: [
        "install",
        "-e",
        "--id",
        "CoreyButler.NVMforWindows",
        "--accept-package-agreements",
        "--accept-source-agreements"
      ],
      env,
      signal,
      onLog
    });

    context.env = await refreshWindowsPath({ env: context.env, signal, onLog });

    const nowHasNvm = await commandExists("nvm", { env: context.env, signal });
    if (!nowHasNvm) {
      throw new Error("nvm 安装完成但当前进程未检测到 nvm。请重启安装器后重试。");
    }

    onLog?.("nvm 安装完成");
    return;
  }

  const brewPath = context.brewPath ?? findBrewPath();
  context.brewPath = brewPath;

  let nvmSourcePath = await getMacNvmSourcePath({ brewPath, env: context.env });
  if (nvmSourcePath) {
    context.nvmSourcePath = nvmSourcePath;
    onLog?.(`已检测到 nvm：${nvmSourcePath}`);
    return;
  }

  if (!options.autoInstall) {
    throw new Error("未检测到 nvm。请先手动安装 nvm（nvm-sh/nvm），然后重试。");
  }

  const resolvedBrew = await ensureBrew({ brewPath, env: context.env });
  onLog?.("正在通过 brew 安装 nvm…");
  await runProcess({
    command: resolvedBrew,
    args: ["install", "nvm"],
    env: context.env,
    signal,
    onLog
  });

  await fs.mkdir(path.join(os.homedir(), ".nvm"), { recursive: true });

  nvmSourcePath = await getMacNvmSourcePath({ brewPath: resolvedBrew === "brew" ? null : brewPath, env: context.env });
  if (!nvmSourcePath) {
    nvmSourcePath = await getMacNvmSourcePath({ brewPath, env: context.env });
  }

  if (!nvmSourcePath) {
    throw new Error("nvm 安装完成，但未找到 nvm.sh。请检查你的 brew 安装是否正常。");
  }

  context.nvmSourcePath = nvmSourcePath;
  onLog?.(`nvm 安装完成：${nvmSourcePath}`);
}

async function ensureNode({ env, signal, onLog, options, context }) {
  if (process.platform === "win32") {
    const hasNode = await commandExists("node", { env, signal });
    const hasNpm = await commandExists("npm", { env, signal });

    if (hasNode && hasNpm) {
      onLog?.("已检测到 Node.js / npm（Windows）");
      return;
    }

    if (!options.autoInstall) {
      throw new Error("未检测到 Node.js。请先手动安装 Node.js（建议 LTS），然后重试。");
    }

    await ensureWinget({ env, signal });
    onLog?.("正在通过 winget 安装 Node.js（LTS）…");
    await runProcess({
      command: "winget",
      args: [
        "install",
        "-e",
        "--id",
        "OpenJS.NodeJS.LTS",
        "--accept-package-agreements",
        "--accept-source-agreements"
      ],
      env,
      signal,
      onLog
    });

    context.env = await refreshWindowsPath({ env: context.env, signal, onLog });

    const nowHasNode = await commandExists("node", { env: context.env, signal });
    const nowHasNpm = await commandExists("npm", { env: context.env, signal });
    if (!nowHasNode || !nowHasNpm) {
      throw new Error("Node.js 安装完成但当前进程未检测到 node/npm。请重启安装器后重试。");
    }

    onLog?.("Node.js 安装完成");
    return;
  }

  const hasNode = await commandExists("node", { env, signal });
  const hasNpm = await commandExists("npm", { env, signal });

  let currentMajor = null;
  if (hasNode) {
    try {
      const result = await runProcess({ command: "node", args: ["-v"], env, signal, collectOutput: true });
      currentMajor = parseSemverMajor(String(result.stdout ?? "").replace(/^v/, "")) ?? parseSemverMajor(result.stdout ?? "");
    } catch {
      currentMajor = null;
    }
  }

  if (hasNode && hasNpm && currentMajor != null && currentMajor >= MIN_NODE_MAJOR) {
    onLog?.(`已检测到 Node.js / npm（macOS, major=${currentMajor}）`);
    return;
  }

  if (!options.autoInstall) {
    const got = currentMajor ? `当前检测到 Node major=${currentMajor}` : "当前未检测到 Node.js";
    throw new Error(
      `${got}，但 openclaw 需要 Node.js >= ${MIN_NODE_MAJOR}。\n` +
        "请先安装/升级 Node.js（建议官方安装器或版本管理器：Volta/fnm/nvm），再重试。"
    );
  }

  // Prefer Homebrew Node when brew is available.
  const brewPath = context.brewPath ?? findBrewPath();
  context.brewPath = brewPath;
  const brewCmd = brewPath || ((await commandExists("brew", { env, signal })) ? "brew" : null);

  if (!brewCmd) {
    throw new Error(
      `未检测到 Homebrew，且当前 Node.js 不满足要求（需要 >= ${MIN_NODE_MAJOR}）。\n` +
        "请先安装 Node.js（推荐官方安装器，或安装 Volta/fnm/nvm 后安装 Node.js），然后重新运行安装器。"
    );
  }

  onLog?.("检测到 Homebrew，正在通过 brew 安装/升级 Node.js…");
  let hasBrewNode = false;
  try {
    await runProcess({ command: brewCmd, args: ["list", "node"], env: context.env, signal, onLog });
    hasBrewNode = true;
  } catch {
    hasBrewNode = false;
  }

  if (hasBrewNode) {
    await runProcess({ command: brewCmd, args: ["upgrade", "node"], env: context.env, signal, onLog }).catch(() => null);
  } else {
    await runProcess({ command: brewCmd, args: ["install", "node"], env: context.env, signal, onLog });
  }

  const nowHasNode = await commandExists("node", { env: context.env, signal });
  const nowHasNpm = await commandExists("npm", { env: context.env, signal });
  if (!nowHasNode || !nowHasNpm) {
    throw new Error("Node.js 安装完成但未检测到 node/npm（可能需要重启终端/安装器）。");
  }

  const after = await runProcess({ command: "node", args: ["-v"], env: context.env, signal, collectOutput: true });
  const major = parseSemverMajor(String(after.stdout ?? "").replace(/^v/, "")) ?? null;
  if (!major || major < MIN_NODE_MAJOR) {
    throw new Error(
      `Node.js 安装完成，但版本不满足要求（检测到 major=${major ?? "unknown"}，需要 >= ${MIN_NODE_MAJOR}）。`
    );
  }

  onLog?.(`Node.js 安装完成（macOS, major=${major}）`);
}

async function ensureGit({ env, signal, onLog, options, context }) {
  const hasGit = await commandExists("git", { env, signal });
  if (hasGit) {
    onLog?.("已检测到 Git");
    return;
  }

  if (!options.autoInstall) {
    throw new Error("未检测到 Git。请先手动安装 Git，然后重试。");
  }

  if (process.platform === "win32") {
    await ensureWinget({ env, signal });
    onLog?.("正在通过 winget 安装 Git…");
    await runProcess({
      command: "winget",
      args: [
        "install",
        "-e",
        "--id",
        "Git.Git",
        "--accept-package-agreements",
        "--accept-source-agreements"
      ],
      env,
      signal,
      onLog
    });

    context.env = await refreshWindowsPath({ env: context.env, signal, onLog });

    const nowHasGit = await commandExists("git", { env: context.env, signal });
    if (!nowHasGit) {
      throw new Error("Git 安装完成但当前进程未检测到 git。请重启安装器后重试。");
    }

    onLog?.("Git 安装完成");
    return;
  }

  const brewPath = context.brewPath ?? findBrewPath();
  context.brewPath = brewPath;
  const brewCmd = brewPath || ((await commandExists("brew", { env, signal })) ? "brew" : null);

  if (brewCmd) {
    onLog?.("正在通过 brew 安装 Git…");
    await runProcess({ command: brewCmd, args: ["install", "git"], env: context.env, signal, onLog });
    const nowHasGit = await commandExists("git", { env: context.env, signal });
    if (!nowHasGit) {
      throw new Error("Git 安装完成但未检测到 git（可能需要重启终端/安装器）。");
    }
    onLog?.("Git 安装完成");
    return;
  }

  // No Homebrew: fall back to Xcode Command Line Tools (git is provided by Apple).
  if (!options.autoInstall) {
    throw new Error("未检测到 Git。请先安装 Xcode Command Line Tools（或手动安装 Git），然后重试。");
  }

  onLog?.("未检测到 Homebrew，尝试触发 Xcode Command Line Tools 安装（将弹出系统提示）…");
  try {
    await runProcess({
      command: "/usr/bin/xcode-select",
      args: ["--install"],
      env: context.env,
      signal,
      onLog,
      collectOutput: false
    });
  } catch {
    // xcode-select --install returns non-zero when already installed or prompt is shown; ignore.
  }

  const nowHasGit = await commandExists("git", { env: context.env, signal });
  if (!nowHasGit) {
    throw new Error("已请求安装 Xcode Command Line Tools。请在弹窗完成安装后，重新运行安装器。");
  }

  onLog?.("Git 已可用（来自 Xcode Command Line Tools）");
}

async function verifyEnvironment({ env, signal, onLog, options: _options, context: _context }) {
  onLog?.("开始环境校验…");

  if (process.platform === "win32") {
    await runProcess({ command: "nvm", args: ["version"], env, signal, onLog }).catch(async () => {
      await runProcess({ command: "nvm", args: ["--version"], env, signal, onLog });
    });
    await runProcess({ command: "node", args: ["-v"], env, signal, onLog });
    await runWindowsShim({ baseCommand: "npm", args: ["-v"], env, signal, onLog });
    await runProcess({ command: "git", args: ["--version"], env, signal, onLog });
    onLog?.("环境校验通过（Windows）");
    return;
  }

  const nodeResult = await runProcess({ command: "node", args: ["-v"], env, signal, onLog, collectOutput: true });
  const versionText = String(nodeResult.stdout ?? "").trim().replace(/^v/, "");
  const major = parseSemverMajor(versionText);
  if (!major || major < MIN_NODE_MAJOR) {
    throw new Error(`Node.js 版本不满足要求（检测到：${versionText || "unknown"}，需要 >= ${MIN_NODE_MAJOR}）。`);
  }

  await runProcess({ command: "npm", args: ["-v"], env, signal, onLog });
  await runProcess({ command: "git", args: ["--version"], env, signal, onLog });

  onLog?.(`环境校验通过（macOS, Node major=${major}）`);
}

async function installOpenclaw({ env, signal, onLog, options, context: _context }) {
  onLog?.(`开始安装：npm install -g ${options.openclawPackage}`);

  if (process.platform === "win32") {
    await runWindowsShim({ baseCommand: "npm", args: ["install", "-g", options.openclawPackage], env, signal, onLog });

    onLog?.("全局安装完成，尝试验证 openclaw 命令…");
    try {
      await runWindowsShim({ baseCommand: "openclaw", args: ["--version"], env, signal, onLog });
    } catch {
      onLog?.("未能直接运行 openclaw（可能是 PATH 未刷新或包未提供 CLI）。可忽略或重启后再试。");
    }

    return;
  }

  await runProcess({ command: "npm", args: ["install", "-g", options.openclawPackage], env, signal, onLog });

  const verify = async () => {
    const resolved = await resolveOpenclawExecution({ env, signal, onLog });
    if (!resolved) return false;
    try {
      await runProcess({
        command: resolved.command,
        args: ["--version"],
        env: resolved.env,
        signal,
        onLog
      });
      return true;
    } catch {
      return false;
    }
  };

  onLog?.("全局安装完成，正在验证 openclaw 命令…");
  const ok = await verify();
  if (ok) return;

  onLog?.("未能直接运行 openclaw，尝试修复（npm --force 重新生成全局命令链接）…");
  await runProcess({ command: "npm", args: ["install", "-g", options.openclawPackage, "--force"], env, signal, onLog });

  const ok2 = await verify();
  if (ok2) return;

  const prefix = await resolveNpmGlobalPrefix({ env, signal, onLog }).catch(() => null);
  const binDir = prefix ? path.join(prefix, "bin") : null;
  throw new Error(
    "openclaw 安装完成，但当前进程仍无法定位 openclaw 命令。\n" +
      (prefix ? `npm prefix -g: ${prefix}\n` : "") +
      (binDir ? `请确认该目录存在 openclaw：${binDir}\n` : "") +
      "建议：重启终端/安装器；或将 npm 全局 bin 目录加入 PATH 后重试。"
  );
}

async function uninstallOpenclaw({ signal, onLog, openclawPackage: _openclawPackage = DEFAULT_OPENCLAW_PACKAGE } = {}) {
  const openclawPackage = validateNpmPackageName(_openclawPackage);

  const baseEnv = createBaseEnv();
  const env = process.platform === "win32" ? await refreshWindowsPath({ env: baseEnv, signal, onLog }) : baseEnv;

  const resolved = await resolveOpenclawExecution({ env, signal, onLog });
  const resolvedPath =
    resolved?.command && path.isAbsolute(resolved.command) ? resolved.command : null;

  const tryRealpath = async (filePath) => {
    if (!filePath) return null;
    try {
      return await fs.realpath(filePath);
    } catch {
      return null;
    }
  };

  const openclawRealPath = await tryRealpath(resolvedPath);

  const logResolved = () => {
    if (resolvedPath) onLog?.(`[which] ${resolvedPath}`);
    if (openclawRealPath && openclawRealPath !== resolvedPath) {
      onLog?.(`[realpath] ${openclawRealPath}`);
    }
  };

  const isMacNvmNodeInstall = (filePath) => {
    if (!filePath) return false;
    if (process.platform !== "darwin") return false;
    const marker = `${path.sep}.nvm${path.sep}versions${path.sep}node${path.sep}`;
    return String(filePath).includes(marker);
  };

  const cleanupMacNvmOpenclaw = async () => {
    if (!isMacNvmNodeInstall(resolvedPath)) return { ok: false, skipped: true };
    if (!resolvedPath) return { ok: false, skipped: true };

    const binDir = path.dirname(resolvedPath);
    if (path.basename(binDir) !== "bin") return { ok: false, skipped: true };
    const nodeRoot = path.dirname(binDir);

    const safeWithinNodeRoot = (candidatePath) => {
      const abs = path.resolve(candidatePath);
      const root = path.resolve(nodeRoot) + path.sep;
      return abs === path.resolve(nodeRoot) || abs.startsWith(root);
    };

    const removed = [];
    const removePath = async (candidatePath, label) => {
      if (!candidatePath) return false;
      if (!safeWithinNodeRoot(candidatePath)) return false;
      try {
        await fs.rm(candidatePath, { recursive: true, force: true });
        removed.push(label ? `${label}: ${candidatePath}` : candidatePath);
        return true;
      } catch {
        return false;
      }
    };

    const moduleDir = path.join(nodeRoot, "lib", "node_modules", openclawPackage);
    const moduleDirFallback = path.join(nodeRoot, "lib", "node_modules", "openclaw");
    const binOpenclaw = path.join(nodeRoot, "bin", "openclaw");

    await removePath(binOpenclaw, "bin");
    await removePath(moduleDir, "module");
    if (moduleDirFallback !== moduleDir) await removePath(moduleDirFallback, "module");

    if (removed.length > 0) {
      onLog?.(`[cleanup] nvm: removed ${removed.length} item(s)`);
      for (const item of removed) onLog?.(`[cleanup] nvm: ${item}`);
      return { ok: true };
    }

    return { ok: false };
  };

  const runSimple = async ({ command, args, prefix }) => {
    if (!command) return { ok: false, skipped: true };
    try {
      if (process.platform === "win32") {
        await runWindowsShim({
          baseCommand: command,
          args,
          env,
          signal,
          onLog: prefix ? (line) => onLog?.(`${prefix} ${line}`) : onLog,
          collectOutput: false
        });
      } else {
        await runProcess({
          command,
          args,
          env,
          signal,
          onLog: prefix ? (line) => onLog?.(`${prefix} ${line}`) : onLog,
          collectOutput: false
        });
      }
      return { ok: true };
    } catch (error) {
      return { ok: false, error };
    }
  };

  const getBrewCommand = async () => {
    const brewPath = findBrewPath();
    if (brewPath) return brewPath;
    const exists = await commandExists("brew", { env, signal, onLog });
    return exists ? "brew" : null;
  };

  const safeUnlinkDanglingSymlink = async (filePath) => {
    if (!filePath) return false;
    try {
      const stat = await fs.lstat(filePath);
      if (!stat.isSymbolicLink()) return false;
      try {
        const target = await fs.readlink(filePath);
        const abs = path.isAbsolute(target) ? target : path.resolve(path.dirname(filePath), target);
        const targetExists = fileExistsSync(abs);
        if (targetExists) return false;
      } catch {
        // If we can't readlink, avoid deleting.
        return false;
      }
      await fs.unlink(filePath);
      onLog?.(`Removed dangling symlink: ${filePath}`);
      return true;
    } catch {
      return false;
    }
  };

  const removeCliIfInstalled = async () => {
    onLog?.("正在尝试移除 openclaw CLI（自动判断安装来源：brew / npm / pnpm）…");
    logResolved();

    const attempts = [];

    const summarizeAttempt = (kind, result) => {
      if (!result) return;
      if (result.skipped) {
        onLog?.(`[cleanup] ${kind}: skipped`);
        return;
      }
      if (result.ok) {
        onLog?.(`[cleanup] ${kind}: ok`);
        return;
      }
      const message = result?.error?.message || "unknown error";
      onLog?.(`[cleanup] ${kind}: failed (${message})`);
    };

    // 1) Homebrew uninstall (best-effort; do not fail uninstall if brew errors).
    if (process.platform === "darwin") {
      const brewCmd = await getBrewCommand();
      if (brewCmd) {
        onLog?.(`${brewCmd} uninstall openclaw`);
        const result = await runSimple({ command: brewCmd, args: ["uninstall", "openclaw"], prefix: "[brew]" });
        attempts.push({ kind: "brew", result });
        summarizeAttempt("brew", result);
        // Even if brew uninstall fails, keep going: the binary in /opt/homebrew/bin can be from npm.
      }
    }

    // 2) npm uninstall -g (try all detected npm candidates; do not fail uninstall if npm errors).
    const tryNpmUninstall = async (npmCommand) => {
      if (!npmCommand) return;
      onLog?.(`${npmCommand} uninstall -g ${openclawPackage}`);
      const result = await runSimple({
        command: npmCommand,
        args: ["uninstall", "-g", openclawPackage],
        prefix: "[npm]"
      });
      attempts.push({ kind: "npm", result, npmCommand });
      summarizeAttempt(`npm(${npmCommand})`, result);
    };

    const nvmNpm =
      resolvedPath && resolvedPath.includes(`${path.sep}.nvm${path.sep}versions${path.sep}node${path.sep}`)
        ? path.join(path.dirname(resolvedPath), process.platform === "win32" ? "npm.cmd" : "npm")
        : null;

    const npmCandidates = [];
    if (nvmNpm && fileExistsSync(nvmNpm)) npmCandidates.push(nvmNpm);
    const hasNpm = await commandExists("npm", { env, signal, onLog });
    if (hasNpm) npmCandidates.push("npm");

    const seenNpm = new Set();
    for (const candidate of npmCandidates) {
      const key = String(candidate);
      if (seenNpm.has(key)) continue;
      seenNpm.add(key);
      await tryNpmUninstall(candidate);
    }

    // 3) pnpm remove -g (best-effort; do not fail uninstall if pnpm errors).
    const hasPnpm = await commandExists("pnpm", { env, signal, onLog });
    if (hasPnpm) {
      onLog?.(`pnpm remove -g ${openclawPackage}`);
      const result = await runSimple({
        command: "pnpm",
        args: ["remove", "-g", openclawPackage],
        prefix: "[pnpm]"
      });
      attempts.push({ kind: "pnpm", result });
      summarizeAttempt("pnpm", result);
    }

    // Clean up a common leftover: dangling symlink in a global bin dir.
    if (resolvedPath) await safeUnlinkDanglingSymlink(resolvedPath);

    // Extra cleanup: if OpenClaw was found under ~/.nvm/... even when not in PATH, remove that copy too.
    // This avoids the UI still showing "已安装" due to backward-compat nvm scan.
    const nvmCleanup = await cleanupMacNvmOpenclaw().catch(() => ({ ok: false }));
    if (nvmCleanup?.ok) {
      // If the resolvedPath was a symlink into the module, re-check and remove any dangling leftovers.
      if (resolvedPath) await safeUnlinkDanglingSymlink(resolvedPath);
    }

    // Final verification: is openclaw still resolvable by the same logic we use for "已安装"?
    const stillResolved = await resolveOpenclawExecution({ env, signal, onLog }).catch(() => null);
    if (!stillResolved) {
      onLog?.("CLI 已移除：当前环境中已找不到 openclaw 命令。");
      return;
    }

    if (stillResolved?.command) onLog?.(`CLI 仍存在：${stillResolved.command}`);
    onLog?.("已尝试移除 CLI，但检测到系统中仍有 openclaw（可能来自其它 Node/路径）。如需彻底清理，请运行诊断并根据输出卸载对应来源。");
  };

  // 1) Always run OpenClaw's own uninstaller (service/state/workspace). Non-interactive.
  //    `openclaw uninstall --yes` can still prompt for which components to remove; we force defaults.
  onLog?.("openclaw uninstall --service --state --workspace --yes --non-interactive");
  try {
    await runOpenclawCommand({
      signal,
      args: ["uninstall", "--service", "--state", "--workspace", "--yes", "--non-interactive"],
      onLog
    });
  } catch (error) {
    const message = error?.message || String(error);
    throw new Error(`openclaw uninstall 失败：${message}`);
  }

  // 2) Then remove the CLI itself (auto-detect / best-effort, but fail if all attempts fail).
  await removeCliIfInstalled();

  return { ok: true };
}

async function runInstall({ signal, options, onProgress, onLog }) {
  const validated = validateOptions(options);

  const context = {
    env: createBaseEnv(),
    brewPath: null,
    nvmSourcePath: null
  };

  if (validated.npmRegistry) {
    context.env = { ...context.env, npm_config_registry: validated.npmRegistry };
  }

  if (process.platform === "win32") {
    context.env = await refreshWindowsPath({ env: context.env, signal, onLog });
  }

  const steps =
    process.platform === "win32"
      ? [
          { id: "nvm", title: "安装 nvm", run: () => ensureNvm({ env: context.env, signal, onLog, options: validated, context }) },
          {
            id: "node",
            title: "安装 Node.js",
            run: () => ensureNode({ env: context.env, signal, onLog, options: validated, context })
          },
          { id: "git", title: "安装 Git", run: () => ensureGit({ env: context.env, signal, onLog, options: validated, context }) },
          {
            id: "verify",
            title: "环境校验",
            run: () => verifyEnvironment({ env: context.env, signal, onLog, options: validated, context })
          },
          {
            id: "openclaw",
            title: "安装 openclaw",
            run: () => installOpenclaw({ env: context.env, signal, onLog, options: validated, context })
          }
        ]
      : [
          {
            id: "node",
            title: "安装 Node.js",
            run: () => ensureNode({ env: context.env, signal, onLog, options: validated, context })
          },
          { id: "git", title: "安装 Git", run: () => ensureGit({ env: context.env, signal, onLog, options: validated, context }) },
          {
            id: "verify",
            title: "环境校验",
            run: () => verifyEnvironment({ env: context.env, signal, onLog, options: validated, context })
          },
          {
            id: "openclaw",
            title: "安装 openclaw",
            run: () => installOpenclaw({ env: context.env, signal, onLog, options: validated, context })
          }
        ];

  onLog?.("OpenClaw 安装器开始执行…");
  onLog?.(`平台：${process.platform} / ${process.arch}`);
  onLog?.(`自动安装：${validated.autoInstall ? "开启" : "关闭"}`);
  onLog?.(`Node 通道：${validated.nodeChannel}`);

  const total = steps.length;

  for (let index = 0; index < steps.length; index++) {
    assertNotAborted(signal);
    const step = steps[index];
    const percent = index / total;
    onProgress?.({ stage: step.id, title: step.title, index: index + 1, total, percent });
    onLog?.(`\n==> [${index + 1}/${total}] ${step.title}`);
    await step.run();
  }

  onProgress?.({ stage: "done", title: "完成", index: total, total, percent: 1 });
  onLog?.("\n全部步骤完成");

  return { ok: true };
}

function parseVersionFromOutput(text) {
  const lines = splitLines(text);
  for (const line of lines) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    if (/^v?\d+\.\d+\.\d+/.test(trimmed)) return trimmed;
    return trimmed;
  }
  return null;
}

async function getOpenclawInfo({ withHelp = false } = {}) {
  const env = createBaseEnv();
  const resolved = await resolveOpenclawExecution({ env });
  const base = resolved?.env ?? (process.platform === "win32" ? await refreshWindowsPath({ env }) : env);
  const command = resolved?.command ?? "openclaw";

  try {
    const versionResult =
      process.platform === "win32"
        ? await runWindowsShim({ baseCommand: command, args: ["--version"], env: base, collectOutput: true })
        : await runProcess({ command, args: ["--version"], env: base, collectOutput: true });

    const version = parseVersionFromOutput(`${versionResult.stdout}\n${versionResult.stderr}`) ?? "unknown";

    let trimmedHelp = null;
    if (withHelp) {
      const helpResult =
        process.platform === "win32"
          ? await runWindowsShim({ baseCommand: command, args: ["--help"], env: base, collectOutput: true })
          : await runProcess({ command, args: ["--help"], env: base, collectOutput: true });

      const helpText = `${helpResult.stdout}\n${helpResult.stderr}`.trim();
      trimmedHelp = helpText.length > 200_000 ? `${helpText.slice(0, 200_000)}\n…` : helpText;
    }

    return {
      installed: true,
      version,
      help: trimmedHelp,
      executable: resolved?.command ?? null
    };
  } catch (error) {
    return {
      installed: false,
      version: null,
      help: null,
      error: error?.message || String(error)
    };
  }
}

async function runOpenclawCommand({ signal, args, onLog }) {
  if (!Array.isArray(args) || args.length === 0) {
    throw new Error("缺少 openclaw 参数");
  }

  const env = createBaseEnv();
  const resolved = await resolveOpenclawExecution({ env, signal, onLog });
  if (!resolved) throw new Error("未检测到 openclaw，请先完成安装。");
  const baseEnv = resolved.env;

  const finalArgs = ["--no-color", ...args];
  onLog?.(`openclaw ${finalArgs.join(" ")} (${resolved.source})`);

  if (process.platform === "win32") {
    await runWindowsShim({ baseCommand: resolved.command, args: finalArgs, env: baseEnv, signal, onLog });
    return { ok: true };
  }

  await runProcess({ command: resolved.command, args: finalArgs, env: baseEnv, signal, onLog });
  return { ok: true };
}

async function runOpenclawCommandCollect({ signal, args, onLog }) {
  if (!Array.isArray(args) || args.length === 0) {
    throw new Error("缺少 openclaw 参数");
  }

  const env = createBaseEnv();
  const resolved = await resolveOpenclawExecution({ env, signal, onLog });
  if (!resolved) throw new Error("未检测到 openclaw，请先完成安装。");
  const baseEnv = resolved.env;

  const finalArgs = ["--no-color", ...args];
  onLog?.(`openclaw ${finalArgs.join(" ")} (${resolved.source})`);

  if (process.platform === "win32") {
    return await runWindowsShim({
      baseCommand: resolved.command,
      args: finalArgs,
      env: baseEnv,
      signal,
      onLog,
      collectOutput: true
    });
  }

  return await runProcess({
    command: resolved.command,
    args: finalArgs,
    env: baseEnv,
    signal,
    onLog,
    collectOutput: true
  });
}

function parseGatewayStatusFromText(text) {
  const raw = String(text ?? "").trim();
  const lower = raw.toLowerCase();

  const dashboardMatch = raw.match(/Dashboard:\s*(https?:\/\/\S+)/i);
  const dashboardUrl = dashboardMatch ? dashboardMatch[1].trim() : null;

  const hasRunning =
    /\bactive:\s*active\b/i.test(raw) ||
    /\(running\)/i.test(raw) ||
    (/\brunning\b/i.test(lower) && !/\bnot\s+running\b/i.test(lower));

  const hasStopped =
    /\bnot\s+running\b/i.test(lower) ||
    /\binactive\b/i.test(lower) ||
    /\bdead\b/i.test(lower) ||
    /\bstopped\b/i.test(lower) ||
    /\bexited\b/i.test(lower) ||
    /\bfailed\b/i.test(lower);

  const hasLoaded =
    /\bloaded:\s*loaded\b/i.test(raw) ||
    (/\(loaded\)/i.test(raw) && !/\(not loaded\)/i.test(raw));

  const hasNotInstalled =
    /service not installed/i.test(raw) ||
    /service unit not found/i.test(raw) ||
    /\bunit\b.*\bnot found\b/i.test(raw) ||
    /\bloaded:\s*not-found\b/i.test(raw) ||
    /\bnot-found\b/i.test(lower);

  // Prefer "running" over everything else. For "not installed" vs "stopped",
  // distinguish them when we have a clear "not installed" signal and the unit
  // isn't loaded. Some platforms print combined messages like "not installed or
  // not running"; in that case we still surface "not installed" if the unit
  // truly isn't present so the UI can guide users correctly.
  if (hasRunning) return { state: "running", dashboardUrl, raw };
  if (hasNotInstalled && !hasLoaded) return { state: "not_installed", dashboardUrl, raw };
  if (hasStopped) return { state: "stopped", dashboardUrl, raw };
  if (hasNotInstalled) return { state: "stopped", dashboardUrl, raw };

  return { state: "unknown", dashboardUrl, raw };
}

async function getGatewayStatus({ signal, onLog } = {}) {
  try {
    const result = await runOpenclawCommandCollect({ signal, args: ["gateway", "status"], onLog });
    const combined = `${result.stdout ?? ""}\n${result.stderr ?? ""}`;
    return parseGatewayStatusFromText(combined);
  } catch (error) {
    const combined = `${error?.stdout ?? ""}\n${error?.stderr ?? ""}`.trim();
    if (combined) return parseGatewayStatusFromText(combined);
    return { state: "unknown", dashboardUrl: null, raw: error?.message || String(error) };
  }
}

module.exports = {
  runInstall,
  getOpenclawInfo,
  runOpenclawCommand,
  runOpenclawCommandCollect,
  getGatewayStatus,
  uninstallOpenclaw,
  resolveOpenclawExecution
};
