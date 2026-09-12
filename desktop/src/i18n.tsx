// Bilingual UI strings (Chinese / English) with a one-click top-right
// toggle. Plain string dictionary — no i18n framework (spec: keep it
// simple). The choice persists in localStorage and defaults to zh.

import {
  createContext,
  useContext,
  useState,
  type ReactNode,
} from "react";

export type Lang = "zh" | "en";

// English is the reference shape; ZH must cover every key (type-checked).
const EN = {
  // App shell
  topbarSub: "Codex / Claude provider manager",
  langToggleTitle: "Switch language",
  navProfiles: "Providers",
  navSessions: "Sessions",
  navAbout: "About",
  // About page
  aboutDesc:
    "Launch Codex / Claude in isolated per-profile runtimes: independent routing, resumable sessions, and keys kept out of generated runtime files.",
  aboutCliTitle: "CLI environment",
  aboutRecheck: "Re-check",
  aboutInstalled: "installed",
  aboutMissing: "not found on this machine",
  aboutInstallHint: "Install it, then click Re-check:",
  aboutCodexName: "Codex CLI (OpenAI protocol)",
  aboutClaudeName: "Claude CLI (Anthropic protocol)",
  aboutDataTitle: "Data location",
  aboutDataDir: "Provider & session data live in",
  // List view
  profilesTitle: "Providers",
  addBtn: "+ Add",
  listError: "Failed to load providers: {err}",
  emptyState: "No providers yet.",
  emptyHint: 'Run agent-switch add from the CLI, or click + Add.',
  cardModel: "model",
  cardBaseUrl: "base_url",
  cardProvider: "provider",
  cardCli: "cli",
  cardApiKey: "api key",
  keyConfigured: "configured",
  keyMissing: "missing",
  launchBtn: "Launch",
  editBtn: "Edit",
  deleteBtn: "Delete",
  confirmDelete: 'Delete provider "{name}"?\nThis removes its TOML file.',
  toastDeleted: "deleted {name}",
  toastDeleteFailed: "delete failed: {err}",
  toastSaved: "saved",
  statusLoading: "loading status…",
  profilesCount: "{n} providers",
  // Provider categories
  typeRelay: "relay",
  typeOpenaiComp: "OpenAI Compatible (self-hosted)",
  typeOfficial: "Official (vendor direct)",
  typeRelayOpt: "Relay (relay)",
  typeOpenaiCompOpt: "OpenAI Compatible (vLLM / SGLang / …)",
  typeOfficialOpt: "Official (OpenAI / Anthropic vendor direct)",
  typeOpenaiCompHint:
    "Works with any OpenAI-protocol endpoint: vLLM, SGLang, llama.cpp, Ollama, or other self-hosted inference.",
  typeOfficialHint:
    "Direct to the vendor API — the official endpoint is built in and never overridden. Key is optional: leave it empty and use the Login action to sign in with a subscription account (Codex OAuth / Claude Pro/Max). The account stays inside this provider's isolated home.",
  // Editor
  editorLoad: "Loading provider…",
  editorLoadError: "Failed to load provider: {err}",
  back: "Back",
  editTitle: "Edit provider: {name}",
  newTitle: "New provider",
  fName: "Display Name",
  fNamePh: "My Provider A",
  fDesc: "Description",
  fDescPh: "Optional notes",
  fCli: "CLI / Protocol",
  fType: "Provider Type",
  fBaseUrl: "Base URL",
  basePhCodex: "https://api.example.com/v1",
  basePhClaude: "https://your-relay.example.com (no /v1)",
  fKey: "API Key",
  keyPh: "sk-…",
  showKey: "Show",
  hideKey: "Hide",
  fModel: "Default Model",
  modelPhCodex: "gpt-5.6",
  modelPhClaude: "claude-sonnet-4-5",
  fetchModelsBtn: "Fetch model list",
  fetchingModels: "Fetching…",
  modelsFound: "Found {n} models",
  modelsNotFound: "no models reported by the server",
  modelsSynced: "Model list synced: {a} added, {b} removed",
  fModelList: "Model List",
  modelListPh: "gpt-5.6-sol, gpt-6 … (comma/space separated, editable)",
  modelListHint:
    "Synced strictly from the server on every fetch: models the upstream no longer offers are removed, new ones are added (the default model is always kept). Written to the model catalog — switch with /model in the TUI.",
  modelCustomOpt: "Custom… (type a model id)",
  modelFromList: "Choose from the fetched list",
  modelHintCodex: "Stored as model in the generated Codex config.toml.",
  modelHintClaude: "Injected as ANTHROPIC_MODEL at launch.",
  fContextWindow: "Context Window (tokens, optional)",
  contextWindowPh: "auto-fills from the server's max_model_len",
  contextWindowHint:
    "Codex only — the model's context window in tokens. Auto-filled when fetching the model list; written into the generated Codex model catalog.",
  contextWindowInvalid: "Context window must be a positive integer.",
  fEffort: "Reasoning effort (optional)",
  effortPh: "e.g. xhigh, medium, low",
  effortHint:
    "Claude only — injected as CLAUDE_CODE_EFFORT_LEVEL. Set it when the server rejects the CLI's default (some vLLM builds accept only xhigh / medium / low).",
  fAuthMode: "Key mode",
  authBearer: "Bearer — ANTHROPIC_AUTH_TOKEN",
  authApiKey: "x-api-key — ANTHROPIC_API_KEY",
  keyPhOfficial: "optional — or sign in with a subscription via Login",
  loginBtn: "Log in (official account)",
  loginStarted: "Login terminal opened — finish the browser flow there.",
  loginFail: "Login failed: {err}",
  loginHint:
    "Opens a terminal in this provider's isolated home (codex login / the Claude login screen). The account credential stays in that home — other providers and your global ~/.codex / ~/.claude are untouched.",
  testBtn: "Test Connection",
  testing: "Testing…",
  testOk: "✓ Success",
  modelsCount: "{n} models",
  testFail: "✗ Failed — {msg}",
  saveError: "Save failed: {err}",
  cancel: "Cancel",
  save: "Save",
  saving: "Saving…",
  // Launch page
  launchTitleCodex: "Launch Codex",
  launchTitleClaude: "Launch Claude",
  fWorkspace: "Workspace",
  wsPh: "leave empty for current directory",
  pickDir: "Browse…",
  clearWs: "Clear",
  launchBtnCodex: "Launch Codex",
  launchBtnClaude: "Launch Claude",
  launching: "Launching…",
  launchFailed: "Launch failed: {err}",
  launchCliMissing:
    "The {cli} CLI is not installed on this machine, so launching is disabled. Install it with the command below, then press Re-check.",
  launchStartedCodex: "Codex started in a new terminal window.",
  launchStartedClaude: "Claude started in a new terminal window.",
  backToList: "Back to providers",
  // Sessions pool page
  sessionsTitle: "Session pool",
  sessionsEmpty: "No sessions yet.",
  sessionsEmptyHint: "Launch a provider once — its conversations will show up here.",
  sessionRefresh: "Refresh",
  sessionResume: "Resume",
  resuming: "Resuming…",
  sessionOpen: "Open",
  sessionAlreadyOpen: "Session already open — close the running terminal first.",
  sessionResumed: "Session resumed in a new terminal window.",
  sessionResumeFail: "Resume failed: {err}",
  sessionNoProfile: "Provider deleted — cannot resume",
  sessionCwd: "workspace",
  sessionSearchPh: "Search by title…",
  sessionPin: "Pin to top",
  sessionUnpin: "Unpin",
  sessionRename: "Set title",
  sessionTitlePh: "Session title",
  sessionDelete: "Delete session",
  confirmDeleteSession:
    'Delete session "{id}"?\nThis removes its transcript file.',
  sessionDeleteFail: "Delete failed: {err}",
  clearUnpinnedBtn: "Clear unpinned",
  clearingUnpinned: "Clearing…",
  confirmClearUnpinned:
    "Delete all {n} unpinned session(s)? Pinned and currently open sessions are kept.",
  clearUnpinnedDone: "Cleared {n} unpinned session(s).",
  clearUnpinnedNone: "Nothing to clear — every session is pinned or open.",
  clearUnpinnedFail: "Clear failed: {err}",
  pinFail: "Pin action failed: {err}",
  titleFail: "Failed to set title: {err}",
  sessionsFiltered: "showing {a} of {b} sessions",
  noSearchResults: "No sessions match \"{q}\".",
  pageOf: "Page {a} / {b}",
  prevPage: "← Prev",
  nextPage: "Next →",
  relJustNow: "just now",
  relMinAgo: "{n} min ago",
  relHourAgo: "{n} h ago",
  relDayAgo: "{n} d ago",
  // Settings page
  navSettings: "Settings",
  settingsTitle: "Settings",
  settingsRunTitle: "Run configuration",
  fDangerous: "Start in dangerous mode (bypass all approvals & sandbox)",
  dangerousHint:
    "When enabled, every launch — including session resume — runs with the bypass flag (codex --dangerously-bypass-approvals-and-sandbox / claude --dangerously-skip-permissions). No approval prompts at all. Use only in trusted environments.",
  fProxyHost: "Proxy host (IP / domain)",
  proxyHostPh: "e.g. 127.0.0.1 — empty means direct connection",
  fProxyPort: "Proxy port",
  proxyPh: "7897",
  proxyHint:
    "When set, every launched Codex / Claude routes all traffic through this HTTP proxy (no direct connections). localhost and 127.0.0.1 always bypass it, so self-hosted vLLM / SGLang still work.",
  proxyPortInvalid: "Proxy port must be a number between 1 and 65535.",
  settingsTerminalTitle: "Terminal",
  terminalAuto: "Auto-detect (recommended)",
  terminalAutoHint: "Detected on this machine: {list}",
  noTerminals: "None detected — pin a custom path below.",
  terminalCustom: "Custom path",
  terminalPh: "Full path to the terminal executable",
  pickTerminal: "Choose terminal…",
  terminalHint:
    "The start script opens in this terminal. If a pinned terminal stops existing, the app falls back to auto-detection and tells you.",
  settingsLogTitle: "Logs",
  logRefresh: "Refresh",
  logEmpty: "No log entries yet.",
  logPathLabel: "file",
  settingsSave: "Save settings",
  settingsSaving: "Saving…",
  settingsSaved: "Settings saved.",
  settingsSaveFail: "Save settings failed: {err}",
  settingsLoadFail: "Load settings failed: {err}",
  settingsLogLoadFail: "Load logs failed: {err}",
  // About page — open-source declaration
  aboutOssTitle: "Open source",
  aboutOssBody:
    "Agent Switch is free & open source: you can use, modify and share it. Source code, issues and releases live on GitHub:",
  aboutOssLink: "github.com/AntyRia/agent-switch",
  // About page — acknowledgements
  aboutThanksTitle: "Acknowledgments",
  aboutThanksLdo:
    "linux.do — an open technical community; thank you for the discussions and feedback.",
  aboutThanksHyperRoute:
    "HyperRoute — model relay (GPT / Claude) that works out of the box; thank you for the support.",
} as const;

export type TKey = keyof typeof EN;

const ZH: Record<TKey, string> = {
  // App shell
  topbarSub: "Codex / Claude 供应商配置管理",
  langToggleTitle: "切换语言",
  navProfiles: "配置供应商",
  navSessions: "会话池",
  navAbout: "关于",
  // About page
  aboutDesc:
    "以独立的隔离环境启动 Codex / Claude：路由互相独立、会话可恢复，密钥不会写入生成的运行时文件。",
  aboutCliTitle: "CLI 环境",
  aboutRecheck: "重新检测",
  aboutInstalled: "已安装",
  aboutMissing: "未检测到",
  aboutInstallHint: "请先安装，然后点「重新检测」：",
  aboutCodexName: "Codex CLI（OpenAI 协议）",
  aboutClaudeName: "Claude CLI（Anthropic 协议）",
  aboutDataTitle: "数据存储",
  aboutDataDir: "供应商与会话数据位于",
  // List view
  profilesTitle: "配置供应商",
  addBtn: "+ 新建",
  listError: "加载供应商失败：{err}",
  emptyState: "还没有供应商。",
  emptyHint: "在终端运行 agent-switch add，或点击 + 新建。",
  cardModel: "模型",
  cardBaseUrl: "接口地址",
  cardProvider: "类型",
  cardCli: "CLI",
  cardApiKey: "API Key",
  keyConfigured: "已配置",
  keyMissing: "缺失",
  launchBtn: "启动",
  editBtn: "编辑",
  deleteBtn: "删除",
  confirmDelete: "删除供应商 “{name}”？\n将移除其 TOML 文件。",
  toastDeleted: "已删除 {name}",
  toastDeleteFailed: "删除失败：{err}",
  toastSaved: "已保存",
  statusLoading: "正在加载状态…",
  profilesCount: "{n} 个供应商",
  // Provider categories
  typeRelay: "中转站",
  typeOpenaiComp: "OpenAI 兼容（自部署）",
  typeOfficial: "官方直连",
  typeRelayOpt: "中转站 (relay)",
  typeOpenaiCompOpt: "OpenAI 兼容（vLLM / SGLang / …）",
  typeOfficialOpt: "官方直连（OpenAI / Anthropic）",
  typeOpenaiCompHint:
    "兼容任意 OpenAI 协议端点：vLLM、SGLang、llama.cpp、Ollama 等本地或自部署推理服务。",
  typeOfficialHint:
    "直连官方 API——内置官方端点，从不覆盖接口地址。Key 可选：留空后用「登录」以订阅账号登录（Codex OAuth / Claude Pro/Max）。账号只保存在该供应商的隔离目录内。",
  // Editor
  editorLoad: "正在加载供应商…",
  editorLoadError: "加载供应商失败：{err}",
  back: "返回",
  editTitle: "编辑供应商：{name}",
  newTitle: "新建供应商",
  fName: "显示名称",
  fNamePh: "我的服务商 A",
  fDesc: "描述",
  fDescPh: "可选备注",
  fCli: "CLI / 协议",
  fType: "Provider 类型",
  fBaseUrl: "Base URL",
  basePhCodex: "https://api.example.com/v1",
  basePhClaude: "https://你的中转站.example.com（不带 /v1）",
  fKey: "API Key",
  keyPh: "sk-…",
  showKey: "显示",
  hideKey: "隐藏",
  fModel: "默认模型",
  modelPhCodex: "gpt-5.6",
  modelPhClaude: "claude-sonnet-4-5",
  fetchModelsBtn: "获取模型列表",
  fetchingModels: "获取中…",
  modelsFound: "找到 {n} 个模型",
  modelsNotFound: "服务器未返回模型",
  modelsSynced: "模型列表已同步：新增 {a} 个，移除 {b} 个",
  fModelList: "模型列表",
  modelListPh: "gpt-5.6-sol、gpt-6 …（逗号/空格分隔，可手动补充）",
  modelListHint:
    "每次获取时与服务器严格同步：上游已下线的模型会被删除，新模型会新增（默认模型始终保留），并写入模型目录——TUI 内 /model 可切换。",
  modelCustomOpt: "自定义…（输入模型 ID）",
  modelFromList: "从获取的列表中选择",
  modelHintCodex: "将写入生成的 Codex config.toml（model 字段）。",
  modelHintClaude: "启动时作为 ANTHROPIC_MODEL 环境变量注入。",
  fContextWindow: "上下文窗口（token，可选）",
  contextWindowPh: "获取模型列表时自动填入",
  contextWindowHint:
    "仅 Codex——模型上下文窗口（token）。获取模型列表时从 max_model_len 自动填入，用于生成的 Codex 模型目录。",
  contextWindowInvalid: "上下文窗口必须是正整数。",
  fEffort: "推理强度（可选）",
  effortPh: "如 xhigh / medium / low",
  effortHint:
    "仅 Claude——以 CLAUDE_CODE_EFFORT_LEVEL 注入。当服务器不接受 CLI 默认值时设置（部分 vLLM 只接受 xhigh / medium / low）。",
  fAuthMode: "Key 传递方式",
  authBearer: "Bearer — ANTHROPIC_AUTH_TOKEN",
  authApiKey: "x-api-key — ANTHROPIC_API_KEY",
  keyPhOfficial: "可选——也可用下方「登录」以订阅账号登录",
  loginBtn: "登录（官方账号）",
  loginStarted: "已打开登录终端——请在其中完成浏览器登录流程。",
  loginFail: "登录启动失败：{err}",
  loginHint:
    "在该供应商的隔离目录中打开终端（codex login / Claude 登录界面）。账号凭据只存放在该隔离目录——不影响其他供应商，也不碰全局 ~/.codex / ~/.claude。",
  testBtn: "测试连接",
  testing: "测试中…",
  testOk: "✓ 成功",
  modelsCount: "{n} 个模型",
  testFail: "✗ 失败 — {msg}",
  saveError: "保存失败：{err}",
  cancel: "取消",
  save: "保存",
  saving: "保存中…",
  // Launch page
  launchTitleCodex: "启动 Codex",
  launchTitleClaude: "启动 Claude",
  fWorkspace: "工作目录",
  wsPh: "留空则使用当前目录",
  pickDir: "选择目录…",
  clearWs: "清空",
  launchBtnCodex: "启动 Codex",
  launchBtnClaude: "启动 Claude",
  launching: "启动中…",
  launchFailed: "启动失败：{err}",
  launchCliMissing:
    "本机未安装 {cli} CLI，已禁止启动。请先安装，然后点击「重新检测」：",
  launchStartedCodex: "已在新终端窗口中启动 Codex。",
  launchStartedClaude: "已在新终端窗口中启动 Claude。",
  backToList: "返回供应商列表",
  // Sessions pool page
  sessionsTitle: "会话池",
  sessionsEmpty: "还没有历史会话。",
  sessionsEmptyHint: "先启动一次供应商，其对话记录会出现在这里。",
  sessionRefresh: "刷新",
  sessionResume: "恢复",
  resuming: "恢复中…",
  sessionOpen: "已打开",
  sessionAlreadyOpen: "该会话已打开，请先关闭正在运行的终端再恢复。",
  sessionResumed: "已在新终端窗口中恢复会话。",
  sessionResumeFail: "恢复失败：{err}",
  sessionNoProfile: "供应商已删除，无法恢复",
  sessionCwd: "工作目录",
  sessionSearchPh: "按标题搜索…",
  sessionPin: "置顶",
  sessionUnpin: "取消置顶",
  sessionRename: "设置标题",
  sessionTitlePh: "会话标题",
  sessionDelete: "删除会话",
  confirmDeleteSession: "删除会话 “{id}”？\n将移除其转录文件。",
  sessionDeleteFail: "删除失败：{err}",
  clearUnpinnedBtn: "一键清除非置顶",
  clearingUnpinned: "清除中…",
  confirmClearUnpinned:
    "将删除全部 {n} 个未置顶会话？已置顶与正在运行的会话会被保留。",
  clearUnpinnedDone: "已清除 {n} 个未置顶会话。",
  clearUnpinnedNone: "没有可清除的会话——所有会话均已置顶或正在运行。",
  clearUnpinnedFail: "清除失败：{err}",
  pinFail: "置顶操作失败：{err}",
  titleFail: "设置标题失败：{err}",
  sessionsFiltered: "显示 {a} / {b} 个会话",
  noSearchResults: "没有匹配 “{q}” 的会话。",
  pageOf: "第 {a} / {b} 页",
  prevPage: "← 上一页",
  nextPage: "下一页 →",
  relJustNow: "刚刚",
  relMinAgo: "{n} 分钟前",
  relHourAgo: "{n} 小时前",
  relDayAgo: "{n} 天前",
  // 设置页
  navSettings: "设置",
  settingsTitle: "设置",
  settingsRunTitle: "运行配置",
  fDangerous: "默认以无视风险模式运行（绕过所有审批与沙箱）",
  dangerousHint:
    "勾选后，所有启动（包括恢复会话）都会附加绕过参数（codex --dangerously-bypass-approvals-and-sandbox / claude --dangerously-skip-permissions），不再弹出任何审批提示。请仅在可信环境中使用。",
  fProxyHost: "代理地址（IP / 域名）",
  proxyHostPh: "如 127.0.0.1 —— 留空表示直连",
  fProxyPort: "代理端口",
  proxyPh: "7897",
  proxyHint:
    "设置后，启动的 Codex / Claude 全部流量走该 HTTP 代理（无法直连）。localhost 与 127.0.0.1 始终豁免，自部署的 vLLM / SGLang 不受影响。",
  proxyPortInvalid: "代理端口必须是 1–65535 之间的数字。",
  settingsTerminalTitle: "终端",
  terminalAuto: "自动检测（推荐）",
  terminalAutoHint: "本机已检测到：{list}",
  noTerminals: "未检测到——请在下方手动指定路径。",
  terminalCustom: "自定义路径",
  terminalPh: "终端可执行文件的完整路径",
  pickTerminal: "选择终端…",
  terminalHint:
    "启动脚本将在此终端中打开。若固定的终端失效，会自动回退到自动检测并提示你。",
  settingsLogTitle: "日志",
  logRefresh: "刷新",
  logEmpty: "暂无日志。",
  logPathLabel: "文件",
  settingsSave: "保存设置",
  settingsSaving: "保存中…",
  settingsSaved: "设置已保存。",
  settingsSaveFail: "保存设置失败：{err}",
  settingsLoadFail: "加载设置失败：{err}",
  settingsLogLoadFail: "加载日志失败：{err}",
  // 关于页——开源声明
  aboutOssTitle: "开源声明",
  aboutOssBody:
    "Agent Switch 免费开源：可自由使用、修改与分享。源代码、问题反馈与版本发布均在 GitHub：",
  aboutOssLink: "github.com/AntyRia/agent-switch",
  // 关于页——特别鸣谢
  aboutThanksTitle: "特别鸣谢",
  aboutThanksLdo:
    "linux.do —— 开放的开源技术社区，感谢大家一直以来的讨论与反馈。",
  aboutThanksHyperRoute:
    "HyperRoute · 超路由 —— 开箱即用的模型中转（GPT / Claude），感谢支持。",
};

interface I18nValue {
  lang: Lang;
  /** Translate a key, substituting {placeholders} from vars. */
  t: (key: TKey, vars?: Record<string, string | number>) => string;
  /** Switch language (persisted to localStorage). */
  toggle: () => void;
}

const I18nContext = createContext<I18nValue | null>(null);

const STORAGE_KEY = "agent-switch-lang";

function loadLang(): Lang {
  try {
    const saved = window.localStorage.getItem(STORAGE_KEY);
    return saved === "en" || saved === "zh" ? saved : "zh";
  } catch {
    return "zh";
  }
}

export function I18nProvider({ children }: { children: ReactNode }) {
  const [lang, setLang] = useState<Lang>(loadLang);

  function toggle() {
    setLang((l) => {
      const next: Lang = l === "zh" ? "en" : "zh";
      try {
        window.localStorage.setItem(STORAGE_KEY, next);
      } catch {
        // localStorage unavailable: the toggle still works in-memory.
      }
      return next;
    });
  }

  function t(key: TKey, vars?: Record<string, string | number>): string {
    let s = (lang === "zh" ? ZH : EN)[key];
    if (vars) {
      for (const [k, v] of Object.entries(vars)) {
        s = s.split(`{${k}}`).join(String(v));
      }
    }
    return s;
  }

  return (
    <I18nContext.Provider value={{ lang, t, toggle }}>
      {children}
    </I18nContext.Provider>
  );
}

/** Current language + translator; must be used inside I18nProvider. */
export function useI18n(): I18nValue {
  const ctx = useContext(I18nContext);
  if (!ctx) throw new Error("useI18n must be used inside I18nProvider");
  return ctx;
}
