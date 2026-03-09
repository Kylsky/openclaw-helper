# OpenClaw Installer (Electron)

一个用 **Electron** 写的 OpenClaw 安装器（兼容 Windows / macOS），用于一键准备开发/运行环境并安装 `openclaw`：

## 功能

- Windows：安装 `nvm-windows`（通过 `winget`）
- macOS：优先复用系统已有 `node/npm`；如检测不到或版本过低且已安装 Homebrew，则通过 `brew install/upgrade node` 自动修复
- 安装 `Git`（优先复用；macOS 无 Homebrew 时会尝试触发 Xcode Command Line Tools 安装）
- 环境校验（node/npm/git 版本检测，`openclaw` 需要 Node.js >= 22）
- 执行 `npm install -g openclaw`，并在命令链接缺失时自动 `--force` 修复，避免“装了但找不到命令”

> 说明：macOS 若未安装 Homebrew 且本机也没有满足要求的 Node.js，会提示你先用官方安装器或版本管理器（Volta/fnm/nvm）安装 Node.js 后再重试。

## 开发

```bash
npm install
npm run dev
```

## 打包发布（生成安装器）

```bash
npm run dist
```

产物输出到 `release/`：

- Windows: `*.exe`（portable，可直接运行）
- macOS: `*.dmg`

也可以按平台打包：

```bash
npm run dist:mac
npm run dist:win
```

> 提示：`dist:win` 最稳妥是在 Windows 环境执行（跨平台打包可能需要额外依赖）。
