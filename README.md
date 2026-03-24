# OpenClaw 助手（Tauri）

一个基于 **Tauri** 的 OpenClaw 桌面助手，用于在 Windows / macOS 上完成安装、更新、健康检查、网关控制、插件配置和卸载清理。

## 功能

- 安装 / 更新 `openclaw`
- 自动检测并修复常见环境问题
- 控制 OpenClaw Gateway 的启动、停止和重启
- 一键执行 `doctor --fix`
- 配置中心与 Workspace Markdown 编辑
- 微信插件配置辅助
- 卸载 OpenClaw 并清理残留

## 开发

```bash
npm install
npm run dev
```

## 打包

```bash
npm run build
```

默认产物位于 Tauri 输出目录：

- Windows：`src-tauri/target/release/bundle/`
- macOS：`src-tauri/target/release/bundle/`

## GitHub Release

当前 workflow 会在打 tag 时：

- 构建 Windows / macOS 安装包
- 上传构建产物到 GitHub Release
- 附带上传 `.github/workflows/tauri-build.yml`
- 附带上传 macOS 修复脚本 `scripts/macos-app-first-aid.command`

## macOS 首次打开提示“App 已损坏”

如果 macOS 首次打开 `OpenClaw 助手.app` 时提示“App 已损坏”或“无法验证开发者”，一般按下面步骤处理即可。

通常只需要前两步：

### 1）允许“任何来源”的 App 运行

打开“终端”，执行：

```bash
sudo spctl --master-disable
```

然后前往：

- macOS Ventura 及更新版本：`系统设置 -> 隐私与安全性 -> 安全性`
- 较早版本：`系统偏好设置 -> 安全性与隐私 -> 通用`

勾选或确认允许“任何来源”的 App。

### 2）移除应用的安全隔离属性

打开“终端”，执行：

```bash
sudo xattr -dr com.apple.quarantine "/Applications/OpenClaw 助手.app"
```

如果你的 App 不在 `/Applications` 目录，请把路径替换成实际安装位置。

### 3）可直接使用 Release 中附带的修复脚本

Release 会附带 `macos-app-first-aid.command`，它会执行上面两条命令：

```bash
./macos-app-first-aid.command
```

如果脚本没有执行权限，可先运行：

```bash
chmod +x ./macos-app-first-aid.command
./macos-app-first-aid.command
```

也可以给脚本传入自定义 App 路径：

```bash
./macos-app-first-aid.command "/Applications/OpenClaw 助手.app"
```

## 说明

- macOS 若未安装 Homebrew 且本机也没有满足要求的 Node.js，助手会提示你先安装 Node.js 后再继续。
- Windows 更新后，为避免弹出终端，助手不会自动重启网关；如需应用新版本，请手动启动或重启网关。
