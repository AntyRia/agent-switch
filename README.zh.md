# Agent Switch

面向 **Codex CLI**（OpenAI 协议）与 **Claude CLI**（Anthropic 协议）的
profile 驱动多供应商启动器。每个供应商对应一份小小的 TOML *profile*；
每次启动使用该 profile 独立、持久的配置与会话目录，无需修改全局 CLI 配置。

<p align="center">
  <a href="README.md">English</a> | <b>简体中文</b>
</p>

> **VS Code 扩展版：**[Agent Switch VS Code 扩展](https://github.com/AntyRia/agent-switch-vscode)
> 在编辑器内使用同一批 profile —— 终端启动 + 编辑器内聊天面板，隔离模型
> 相同。两者读取同一配置根目录，profile 原样通用。

- [简介](#简介)
- [功能特性](#功能特性)
- [运行前提](#运行前提)
- [快速上手](#快速上手)
- [安装](#安装)
- [使用](#使用)
- [界面导览](#界面导览)
- [Profile 格式](#profile-格式)
- [隔离原理](#隔离原理)
- [常见问题](#常见问题)
- [开发](#开发)
- [参与贡献](#参与贡献)
- [捐赠支持](#捐赠支持)
- [致谢](#致谢)
- [开源协议](#开源协议)

## 简介

### 要解决的问题

Codex CLI 与 Claude CLI 都有默认配置目录（`~/.codex`、`~/.claude`）。
通过默认配置管理多个供应商时，需要手动维护各组设置：

- **切换供应商需要管理成组配置。** 接口地址、凭据和模型需要保持一致，
  同时避免影响已有登录状态。
- **并行使用需要独立配置。** 一个终端连接中转、另一个连接本地服务时，
  需要为每次启动单独管理环境变量或 CLI home。

### 解决思路

Agent Switch 把"供应商"从全局配置中拆出来，落到每个供应商独立的一份
小文件里：

```text
Profile (profiles/<id>.toml)
        |
        v
隔离持久 home (runtime/<profile-id>/.codex 或 .claude)
        |
        v
独立的 CODEX_HOME / CLAUDE_CONFIG_DIR
        |
        v
启动 Codex CLI（OpenAI）或 Claude CLI（Anthropic），
绑定该 profile 的供应商 / 模型 / Key
```

每个 profile 都拥有一份**持久的**隔离 home，因此：

- **一个供应商 = 一份 profile 文件。** 新增供应商不会触碰任何全局内容。
- **多供应商并行。** 同一个 CLI 可以同时对接任意多个中转、本地服务或
  官方端点，互不干扰。
- **切换零成本。** 切供应商就是启动另一个 profile；会话内用 `/model`
  在 profile 的模型列表之间切换。
- **Key 安全启动。** API Key 只以子进程环境变量的方式注入，从不复制到生成
  的运行时配置或启动脚本。若使用 `provider.api_key`，该值会按设计保存在
  profile TOML 中；如需密钥不落入 Agent Switch 配置目录，请使用
  `provider.api_key_env`。

## 功能特性

- **一个供应商一份 profile** —— 一个 TOML 文件定义接口地址、API Key 与
  默认模型（中转站、自建 vLLM，或厂商官方端点）。
- **双协议、双 CLI** —— `codex`（OpenAI 协议，GPT 系模型）与 `claude`
  （Anthropic 协议，Claude 系模型）；由 profile 决定启动哪个 CLI。
- **配置与历史独立** —— 每个 profile 拥有持久运行环境
  （`CODEX_HOME` / `CLAUDE_CONFIG_DIR`）。Agent Switch 不修改全局 CLI
  配置，也不限制底层 CLI 的文件或网络访问。
- **官方供应商支持** —— profile 可直接指向 OpenAI / Anthropic：使用
  平台 API Key，或运行一次订阅登录（`agent-switch login`），之后由该
  profile 的隔离环境保存账号。
- **会话池** —— 会话历史按 profile 存储，可在 CLI 或 GUI 中恢复；终端
  仍打开的会话会标记为 **运行中（Open）**，不能重复恢复。已置顶会话受
  保护，GUI 还支持一键清除所有未置顶会话。
- **模型自动同步** —— 成功获取模型列表后替换该供应商目录（删除已下线
  模型、加入新模型，并保留当前默认模型）。获取失败时保留旧目录并报告
  同步失败，旧模型仍可能已被服务端停用。
- **Key 安全** —— API Key 只以子进程环境变量注入，从不复制到生成的运行时
  配置或启动脚本。直接填写的 `provider.api_key` 仍会保存在 profile TOML；
  如需密钥不落盘，请使用 `provider.api_key_env`。列表和摘要中打码；编辑器有主动显示密钥的按钮。
- **CLI + GUI 双形态** —— 单文件命令行工具与双语（中文 / 英文）Tauri
  桌面应用，共用同一套 profile 文件。

## 运行前提

Agent Switch 负责驱动官方 CLI 工具，**不内置**这些工具。请按需全局安装
一次：

| Profile 的 CLI | 安装命令 |
| --- | --- |
| Codex CLI（OpenAI 协议） | `npm install -g @openai/codex` |
| Claude CLI（Anthropic 协议） | `npm install -g @anthropic-ai/claude-code` |

如果缺少对应的 CLI，命令行与 GUI 都会明确提示缺的是哪一个——GUI 还会
在补装前禁用启动按钮。随时可用 `agent-switch doctor` 体检。

## 快速上手

从安装到第一次会话，只需三步。

**第 1 步——安装**（任选其一）：

- 只用命令行 → [安装 CLI](#1-纯命令行不含-gui)
- 使用桌面应用 → [安装 GUI](#2-gui-桌面应用)

**第 2 步——检查环境**：

GUI 用户打开**关于**页检查 CLI 环境；命令行用户运行：

```bash
agent-switch doctor      # 检查 Codex / Claude CLI 与配置目录
```

**第 3 步——创建 profile 并启动**：

GUI 用户添加 profile、填写供应商配置并保存，然后点击**启动**选择工作区。
命令行用户运行：

```bash
agent-switch add         # 交互式创建：CLI / 供应商 / 接口 / Key / 模型
agent-switch test <id>   # 测试接口连通（可选）
agent-switch run <id>    # 启动——进入 CLI 的 TUI
agent-switch sessions    # 查看 / 恢复历史会话
```

> 不带参数运行 `agent-switch` 会打印这份快速上手指南和命令一览；
> `agent-switch <命令> --help` 查看单个命令的详细说明。

## 安装

两种形态共用同一个配置根目录（Windows：`%APPDATA%\agent-switch`；
macOS / Linux：`~/.agent-switch`）。删除程序与该目录即可移除 Agent Switch
及其配置、会话历史；底层 CLI 和系统应用缓存需要分别管理。

### 1. 纯命令行（不含 GUI）

#### 方式 A——一行命令安装（推荐）

macOS/Linux 安装脚本需要 [Node.js](https://nodejs.org) 解析 GitHub 发行信息。
也可以直接下载 CLI ZIP 手动安装。

脚本会自动从 [GitHub Releases](https://github.com/AntyRia/agent-switch/releases)
选择包含当前平台安装包的最新正式版本。Windows 脚本更新用户 PATH 与当前 PowerShell 会话。
macOS/Linux 安装到 `~/.local/bin`；若该目录不在 PATH 中，会向已存在的
`.bashrc`、`.zshrc` 和 `.profile` 添加配置，并打印当前终端需执行的
`export` 命令。管道中的脚本无法修改父 shell 的环境。

**Windows**（PowerShell）：

```powershell
irm https://raw.githubusercontent.com/AntyRia/agent-switch/main/scripts/install.ps1 | iex
```

**macOS / Linux**：

```bash
curl -fsSL https://raw.githubusercontent.com/AntyRia/agent-switch/main/scripts/install.sh | sh
```

> 各版本可能提供不同平台的安装包。脚本会选择包含当前平台安装包的最新正式版本，
> 跳过草稿和预发布版本；若没有匹配的安装包，请从源码构建。

发布资产命名规范：`agent-switch-<版本>-<target>.zip`（标准 Rust 目标名）。
下载前请核对对应 Release 的资产列表。

| 平台 | CLI 安装包 |
| --- | --- |
| Windows x64 | `agent-switch-0.2.5-x86_64-pc-windows-msvc.zip`（[v0.2.5](https://github.com/AntyRia/agent-switch/releases/tag/v0.2.5)） |
| macOS（Apple 芯片） | `agent-switch-0.2.5-aarch64-apple-darwin.zip` |
| macOS（Intel）/ Linux | 从源码构建 |

习惯手动操作？下载对应 zip 解压后，把 `agent-switch(.exe)` 放到任意
`PATH` 目录即可。

**卸载**：

| 安装方式 | 卸载方法 |
| --- | --- |
| 一行命令（Windows） | 删除 `%LOCALAPPDATA%\agent-switch`，并从用户 PATH 中移除其 `bin` |
| 一行命令（macOS / Linux） | 删除 `~/.local/bin/agent-switch` |
| 手动 / cargo | 删除可执行文件，或 `cargo uninstall agent-switch` |

如需同时清除 profile 与会话历史，再删除配置根目录。

#### 方式 B——源码构建

前置条件：[Rust stable](https://rustup.rs)，以及所需的底层 CLI
（见[运行前提](#运行前提)）。

```bash
git clone https://github.com/AntyRia/agent-switch.git && cd agent-switch
cargo build --release                    # → target/release/agent-switch(.exe)
cargo install --path crates/agent-switch-cli   # 可选：安装到 PATH
```

### 2. GUI 桌面应用

GUI 提供 Profile 编辑、启动、会话池、设置、日志、
连通性测试与模型列表，不依赖 `agent-switch` CLI 二进制。底层仍需 npm
安装对应的 Codex / Claude CLI（见[运行前提](#运行前提)）；只有实际检测到可
执行文件后，GUI 才会显示为可用。

#### 方式 A——下载安装包（推荐）

到 [GitHub Releases](https://github.com/AntyRia/agent-switch/releases)
下载对应平台的安装包并运行。

| 平台 | GUI 安装包 |
| --- | --- |
| Windows x64 | 使用 [v0.2.5 Windows 安装包](https://github.com/AntyRia/agent-switch/releases/tag/v0.2.5) |
| macOS（Apple 芯片） | `Agent.Switch_0.2.5_aarch64.dmg`（ad-hoc 签名，未公证） |
| macOS（Intel） | 本版本未构建 |
| Linux | 本版本未构建，请从源码构建 |

**macOS 安装**——打开 DMG，将 **Agent Switch** 拖入**应用程序**。
安装包适用于 Apple 芯片，不包含 Codex 或 Claude，使用前需安装所需的 CLI。
Terminal 与 iTerm 会自动检测。

应用使用 ad-hoc 签名，尚未通过 Apple 公证。若被 macOS 拦截，请先将下载文件
的校验和与同一 Release 的 `SHA256SUMS.txt` 核对；确认信任该下载来源后，
尝试打开应用，再到**系统设置 → 隐私与安全性 → 仍要打开**完成首次启动。

**卸载**——使用系统常规卸载方式（Windows「应用和功能」、macOS 拖出
应用程序、Linux `apt` / `rpm`）。Profile 仍保留在配置根目录中；删除
该目录即可清除 Agent Switch 的配置与会话历史。

#### 方式 B——源码构建

前置条件：[Rust stable](https://rustup.rs)、**Node.js 18+**（Tauri
前端）、底层 CLI；macOS 另需 Command Line Tools（`xcode-select --install`），Linux 另需
[Tauri 系统依赖](https://tauri.app/start/prerequisites/)。

```bash
git clone https://github.com/AntyRia/agent-switch.git && cd agent-switch
cd desktop
npm install
npm run tauri build            # 安装包 → src-tauri/target/release/bundle/
```

### 更新

- **GUI**：应用启动时会检查官方发布（有缓存、不阻塞界面），发现新版本时弹出确认框；确认后显示下载进度条，校验 SHA-256，替换旧版本并自动重启。关于页也有手动检查按钮。
- **CLI**：`agent-switch update` 下载并就地安装最新正式版（带校验）；每次启动发现新版本时都会打印一行提示，设置 `AGENT_SWITCH_NO_UPDATE_CHECK=1` 可关闭。

## 使用

### 命令一览

| 命令 | 说明 |
| --- | --- |
| `agent-switch` | 不带参数：打印快速上手指南与命令一览。 |
| `agent-switch init` | 初始化配置根目录（`profiles/` + `runtime/`）。可重复执行。 |
| `agent-switch list` | 列出全部 profile（ID / 名称 / CLI / 默认模型）。 |
| `agent-switch show <PROFILE>` | 查看单个 profile；API Key 永远打码。 |
| `agent-switch add` | 交互式新建 profile（直接回车 = 使用括号内默认值；支持管道输入）。 |
| `agent-switch edit <PROFILE>` | 用 `$EDITOR` 打开 profile 的 TOML（未设置时回退：Windows 用 `notepad`，其他系统用 `vi`）。 |
| `agent-switch remove <PROFILE> [--yes]` | 删除 profile（会请求确认；脚本中可用 `--yes`）。 |
| `agent-switch test <PROFILE>` | 测试供应商接口（codex：`GET <base_url>/models`；claude：`POST <base_url>/v1/messages`，1 token 请求）。 |
| `agent-switch models <PROFILE>` | 拉取供应商的模型列表，每行一个 id。 |
| `agent-switch sessions` | 列出所有 profile 运行环境中可恢复的会话（会话池）。 |
| `agent-switch run <PROFILE> [WORKSPACE] [-- cli 参数]` | 在隔离环境中启动该 profile 的 CLI（Codex 或 Claude；默认工作区为当前目录）。 |
| `agent-switch login <PROFILE> [WORKSPACE]` | 为 `official` profile 登录官方账号：在 profile 的隔离环境中运行 `codex login` / Claude 登录界面。订阅凭证保存在该环境中，之后无需 API Key。 |
| `agent-switch doctor` | 检查 Codex / Claude CLI 是否可用及配置目录状态。 |
| `agent-switch cleanup` | 清理旧运行环境目录（保留最近 20 个，删除 7 天以前的）。 |
| `agent-switch logs [N]` | 查看应用日志最后 N 行（默认 50）。 |
| `agent-switch settings [--edit]` | 查看或编辑全局启动设置（危险模式、代理、终端）。 |
| `agent-switch update [--check]` | 下载最新正式版、校验 SHA-256 后就地替换当前二进制；`--check` 仅报告最新版本。 |

### 示例

```bash
# profile 管理
agent-switch show my-relay-gpt
agent-switch edit my-relay-gpt
agent-switch remove my-relay-gpt

# 给 CLI 追加参数，放在 -- 之后
agent-switch run my-relay-gpt -- --model gpt-5.6-sol

# 恢复会话（codex: resume，claude: --resume）
agent-switch run my-relay-gpt -- resume <SESSION_ID>
agent-switch run my-relay-claude -- --resume <SESSION_ID>

# 环境体检 / 清理旧运行环境 / 查看日志
agent-switch doctor
agent-switch cleanup
agent-switch logs 80
```

如果某个会话的终端仍然打开着，恢复会提示 `session already open`——
先关闭那个终端窗口，再恢复即可。

### 启动之后（TUI 内）

`agent-switch run` 会直接进入 CLI 的 TUI（Codex 或 Claude），工作区
即你执行命令时所在的目录：

- **`/model`** —— 在 profile 内切换模型：列出该 profile 的全部模型
  （默认模型 + profile 的 *模型列表* + 从服务端同步的模型），选中即
  生效并关闭选择器。
- **会话历史** —— 保存在该 profile 自己的隔离运行环境中，退出 TUI 后
  仍在。之后用 `agent-switch sessions` 找到它，再用
  `agent-switch run <PROFILE> -- resume <SESSION_ID>` 继续。
- **退出** —— 输入 `/exit`（或按 `Ctrl+C`）即可回到你自己的终端。
  profile 与全部会话原样保留，随时可以再次 `run` 或恢复。

如果 profile 对应的 CLI 未安装，`agent-switch run` 会输出：

```text
Claude CLI not found.
Please install Claude CLI first:
  npm install -g @anthropic-ai/claude-code
```

（codex profile 则输出对应的 `Codex CLI not found.` /
`npm install -g @openai/codex`。）

### 同时运行多个供应商

```bash
# 终端 A
agent-switch run relay-gpt

# 终端 B
agent-switch run relay-claude

# 终端 C
agent-switch run local-vllm
```

所有进程并行运行——各自拥有独立的 `runtime/<profile-id>/` 环境与独立的
Key 环境变量，互不干扰。

## 界面导览

桌面应用（Tauri + TypeScript）与 CLI 共用同一套 profile 文件，
**支持中英文双语**——右上角按钮一键切换，选择会被记住。

- **Profile 列表** —— 每个 profile 一张卡片：CLI 徽章（Codex / Claude）、
  供应商类别、默认模型、Key 状态，以及 启动 / 登录（official）/ 编辑 /
  删除 操作。启动会弹出小窗选择工作区目录，然后在本机终端打开 CLI。
- **Profile 编辑器** —— 选择 CLI / 协议（Codex / OpenAI 或 Claude /
  Anthropic）、供应商类别（中转 / 自建 OpenAI 兼容 / 官方）；Claude
  profile 还可选择 Key 方式（Bearer / x-api-key）。official profile 的
  厂商端点固定，API Key 可选。模型字段旁的 *拉取模型列表* 在获取成功后
  刷新 *模型列表*（移除旧模型、加入新模型，并保留默认模型）；获取失败时
  保留原列表。启动时该列表写入模型目录，TUI 中可用 `/model`
  切换。新建 profile 的 ID 为自动生成的 UUID，仅供内部使用——不在界面
  中展示，也不可编辑。
- **会话池** —— 跨所有 profile 的全部可恢复会话，按时间倒序，每行带
  CLI（codex / claude）与通道（profile 名 + 供应商类别）徽章、首条用户
  消息、原始工作目录和相对时间。**终端仍打开的会话显示绿色
  「运行中」标记，其恢复按钮被禁用**——关闭终端后再恢复。支持搜索、
  置顶、重命名、删除、一键恢复，以及 **一键清除非置顶**（删除所有既未
  置顶也未运行的会话）。
- **设置** —— 危险模式（每次启动绕过所有审批与沙箱）、所有被启动 CLI
  共同使用的 HTTP 代理（localhost / 127.0.0.1 始终直连）、使用的终端
  （自动检测或自定义路径），以及用于排查的应用日志尾部。
- **关于** —— CLI 健康检查（缺失时给出安装命令，并附「重新检测」按钮，
  装好后即可解锁启动）与配置目录位置。
- 状态栏实时显示两个 CLI 是否都已安装。
- 当某个 profile 的 CLI 缺失时，启动（及其会话恢复）会被禁用，并直接
  展示对应的 `npm install` 命令——不再出现难懂的报错。

## Profile 格式

Profile 位于配置根目录（Unix 下 `~/.agent-switch`，Windows 下
`%APPDATA%\agent-switch`）的 `profiles/<id>.toml`。

GPT 中转站——Codex CLI，OpenAI 协议：

```toml
id = "relay-gpt"
name = "GPT Relay A"
description = "主力云中转"
cli = "codex"

[provider]
type = "relay"
base_url = "https://example.com/v1"
api_key = "sk-xxxxxxxx"

[model]
default = "gpt-5.6"

[codex]
provider_name = "relay-gpt"
```

Claude 中转站——Claude CLI，Anthropic 协议：

```toml
id = "relay-claude"
name = "Claude Relay A"
description = "主力 Claude 中转"
cli = "claude"

[provider]
type = "relay"
# Anthropic 惯例：服务端根地址，结尾不带 /v1
base_url = "https://relay.example.com"
api_key = "sk-xxxxxxxx"
# Key 的传递方式："auth_token"（Authorization: Bearer，默认）
# 或 "api_key"（x-api-key 头，官方 API 惯例）
auth_mode = "auth_token"

[model]
default = "claude-sonnet-4-5"

[codex]
provider_name = "relay-claude"
```

本地 vLLM 服务——Codex CLI（vLLM 讲 OpenAI 协议）：

```toml
id = "local-vllm"
name = "本地 Qwen vLLM"
description = "本地 vLLM 服务"
cli = "codex"

[provider]
type = "vllm"
base_url = "http://127.0.0.1:8000/v1"
# 不写 api_key：本地服务通常无需鉴权

[model]
default = "Qwen/Qwen3.8-27B"

[codex]
provider_name = "local-vllm"
```

官方厂商——不经过任何中转（订阅登录或厂商 API Key）：

```toml
id = "official-gpt"
name = "OpenAI 官方"
cli = "codex"

[provider]
type = "official"
# 仅作展示；启动时使用内置厂商端点，从不发送 base URL 覆盖。保持厂商默认值即可。
base_url = "https://api.openai.com/v1"
# api_key 可选：填平台 API Key（以 OPENAI_API_KEY / ANTHROPIC_API_KEY
# 注入），或干脆不填——先 `agent-switch login official-gpt` 登录一次，
# 之后使用该 profile 隔离环境中保存的订阅账号。

[model]
default = "gpt-5.6"

[codex]
provider_name = "official-gpt"
```

claude 的官方配置：`type = "official"`、
`base_url = "https://api.anthropic.com"`；Key（若填写）始终按厂商
惯例以 `x-api-key` 头传递。

字段说明：

- `id` —— 仅 `a-z`、`0-9`、`-`、`_`，最长 64 字符；须与文件名一致。
- `cli` —— `"codex"`（OpenAI 协议，Codex CLI）或 `"claude"`（Anthropic
  协议，Claude CLI）。缺省 = `codex`（旧 profile 继续可用）。
- `provider.type` —— `"relay"`（第三方中转）、`"vllm"`（本地部署）或
  `"official"`（厂商直连）。`relay` / `vllm` 仅作信息展示；`official`
  改变启动行为：不覆盖 base URL，Key 可选（无 Key 启动时使用保存在
  该 profile 隔离环境中的订阅登录；codex 还会固定
  `cli_auth_credentials_store = "file"`，确保登录态不会泄漏进系统钥匙
  串）。旧值 `openai-compatible` / `openai` 仍被接受。
- `provider.base_url` —— codex：OpenAI 兼容根地址（通常以 `/v1` 结尾）；
  claude：Anthropic 服务端根地址（通常**不带** `/v1`）。
- `provider.api_key` —— Key，保存在 profile TOML 中，启动时以子进程
  环境变量注入，不复制到生成的运行时配置或启动脚本。如需密钥保存在
  Agent Switch 配置目录之外，请使用 `provider.api_key_env`。
- `provider.api_key_env` —— 可选：持有 Key 的环境变量名。该变量已设置
  且非空时，优先于 `api_key`。
- `provider.auth_mode` —— 仅 claude：`"auth_token"`（Bearer，默认）或
  `"api_key"`（x-api-key）。codex profile 忽略此字段。
- `model.default` —— 模型 id（Codex 配置中的 `model` / Claude 的
  `ANTHROPIC_MODEL`）。
- `model.effort` —— 仅 claude，可选：以 `CLAUDE_CODE_EFFORT_LEVEL`
  注入的思考强度。当服务端不接受 CLI 默认值时设置（部分 vLLM 构建只
  接受 `xhigh` / `medium` / `low`）。
- `model.models` —— 成功获取模型列表后刷新；成功时删除旧条目，服务端不可达
  时保留最近目录，以便仍可尝试离线启动。
- `codex.provider_name` —— 生成的 Codex 配置中 `[model_providers.<name>]`
  小节的名称（仅 codex 引擎使用）。

各种组合（中转 / vLLM / 官方 × codex / claude）的现成模板见
[`examples/`](examples/)。

## 隔离原理

- 每个 profile 都拥有一份**持久的**隔离 home `runtime/<profile-id>/`：
  - **codex profile**：`runtime/<profile-id>/.codex/`，其中的
    `config.toml` 每次启动都从 profile 重新生成（模型、供应商、base
    URL——**不含 API Key**）；profile 始终是路由的唯一事实来源。
  - **claude profile**：`runtime/<profile-id>/.claude/`（Claude 所需
    配置全部来自环境变量；该目录用于存放会话记录）。
- 因为 home 是持久的，**会话历史按 profile 保存、可恢复**：Claude 会话
  记录与 Codex rollout 都存放在其中，会话池（`agent-switch sessions` /
  GUI 的会话页）列出它们并在同一隔离环境中恢复。
- 启动 CLI 时把 `CODEX_HOME`（codex）或 `CLAUDE_CONFIG_DIR`（claude）
  指向该 profile 的目录。Agent Switch 不修改全局 CLI 配置；工作区文件、
  插件及底层 CLI 自身的文件与网络访问仍由该 CLI 的设置控制。
- claude profile 的启动还会设置 `ANTHROPIC_BASE_URL`、`ANTHROPIC_MODEL`
  与 `ANTHROPIC_SMALL_FAST_MODEL`（固定为同一模型，避免中转站没有
  haiku 时后台请求失败）。
- API Key 只作为被启动 CLI 的子进程环境变量传递——codex 用
  `OPENAI_API_KEY`，claude 用 `ANTHROPIC_AUTH_TOKEN` 或
  `ANTHROPIC_API_KEY`（按 `auth_mode` 而定）。不会导出到你的 shell，也不会
  写入生成的运行时配置或启动脚本。直接填写的 `provider.api_key` 仍会保存在
  profile TOML；如需密钥不落盘，请使用 `provider.api_key_env`。
- **被启动的 CLI 拿到的是干净环境。** 来自 shell（或启动 GUI 的那个
  应用）的环境里残留的 `CLAUDE_CODE_*` / `CLAUDECODE` 嵌套会话标记，
  以及无关的 `ANTHROPIC_*` / `OPENAI_*` 供应商变量都会被清除，只应用
  profile 自己的变量。这既保证了 GUI 本身运行在 Claude Code 会话里时
  会话记录照常保存，也让路由与父 shell 完全解耦。
- **claude home 预置首次启动引导。** 全新的
  `runtime/<profile-id>/.claude/` 会预置 `settings.json`（仅在不存在时
  写入 `theme`）和 `.claude.json`（`hasCompletedOnboarding`，以及按
  工作区合并的 `hasTrustDialogAccepted`，不覆盖 Claude 自身状态），
  首次启动即可跳过主题选择与「信任此文件夹」对话框。
- 旧版本遗留的临时 `runtime/<uuid>` 目录会被自动清理：7 天以前的直接
  删除，且最多保留最近 20 个（`agent-switch cleanup` 可随时手动执行
  同样逻辑）。按 profile 命名的 home 永不清理。

## 常见问题

- **`Codex CLI not found.` / `Claude CLI not found.`** —— 该 profile
  对应的底层 CLI 未安装。按提示执行打印出的 `npm install -g …` 命令
  后重试（GUI：在「关于」页或启动弹窗中点击 *重新检测*）。
- **`session already open`** —— 该会话的终端仍开着。关闭那个终端窗口
  后再恢复。
- **连通性测试失败** —— 检查 `provider.base_url`（codex 通常以 `/v1`
  结尾，claude 通常不带）、API Key，以及该模型 id 是否在服务端存在
  （`agent-switch models <id>`）。
- **能列出模型但无法对话** —— `/models` 请求成功不代表推理可用。
  Codex profile 需要 Responses API，Claude profile 需要 Anthropic
  Messages；仅支持 Chat Completions 的服务不足以使用。请检查所选模型
  的可用状态与供应商返回的错误信息。
- **其他问题** —— `agent-switch logs` 可查看应用日志的最后若干行（GUI
  的设置页有同样的日志尾部），`agent-switch doctor` 可随时复检环境。

## 开发

- **独立开发数据** —— 设置 `AGENT_SWITCH_HOME` 可让 CLI 与 GUI 指向不同的
  配置根目录；这隔离的是 Agent Switch 数据，不是底层 CLI 的文件或网络访问权限：

  ```bash
  export AGENT_SWITCH_HOME=/tmp/agent-switch-test   # Unix
  # set AGENT_SWITCH_HOME=C:\temp\agent-switch-test # Windows
  ```

- **热重载运行 GUI**：

  ```bash
  cd desktop
  npm install
  npm run tauri dev
  ```

## 参与贡献

项目结构、构建 / 测试命令，以及每个改动都必须保持的安全不变量，见
[CONTRIBUTING.md](CONTRIBUTING.md)。

## 捐赠支持

如果 Agent Switch 为你节省了时间，欢迎请维护者喝杯咖啡。捐赠将用于项目
的持续维护，以及开发测试过程中产生的模型 API 费用。

| 金额 | 链接 |
| --- | --- |
| 港币 20 | [Stripe 捐赠](https://buy.stripe.com/4gM7sLepN8bmdCq1eQ08g01) |
| 港币 50 | [Stripe 捐赠](https://buy.stripe.com/bJe5kD81pajucym9Lm08g02) |
| 港币 100 | [Stripe 捐赠](https://buy.stripe.com/9B67sL6XlfDObui9Lm08g03) |

感谢支持！

## 致谢

- **HyperRoute · 超路由** —— [hyperroute.cc](https://hyperroute.cc) ——
  开箱即用的模型中转接入（GPT / Claude），感谢支持。
- **linux.do** —— [linux.do](https://linux.do/) —— 开放的技术社区，本
  项目的许多讨论与反馈都发生在这里，谢谢大家。

## 开源协议

[MIT](LICENSE) —— 完整文本见 [LICENSE](LICENSE) 文件。

本软件按「现状」提供，不提供任何形式的担保。你需对通过本软件配置的
供应商、Key 与内容自行负责。
