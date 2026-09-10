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
  topbarSub: "Codex / Claude profile manager",
  langToggleTitle: "Switch language",
  navProfiles: "Profiles",
  navSessions: "Sessions",
  navAbout: "About",
  // About page
  aboutDesc:
    "Launch Codex / Claude in isolated per-profile runtimes: independent routing, resumable sessions, and API keys that never touch disk.",
  aboutCliTitle: "CLI environment",
  aboutRecheck: "Re-check",
  aboutInstalled: "installed",
  aboutMissing: "not found on this machine",
  aboutInstallHint: "Install it, then click Re-check:",
  aboutCodexName: "Codex CLI (OpenAI protocol)",
  aboutClaudeName: "Claude CLI (Anthropic protocol)",
  aboutDataTitle: "Data location",
  aboutDataDir: "Profiles & session data live in",
  // List view
  profilesTitle: "Profiles",
  addBtn: "+ Add",
  listError: "Failed to load profiles: {err}",
  emptyState: "No profiles yet.",
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
  confirmDelete: 'Delete profile "{id}"?\nThis removes its TOML file.',
  toastDeleted: "deleted {id}",
  toastDeleteFailed: "delete failed: {err}",
  toastSaved: "saved",
  statusLoading: "loading status…",
  profilesCount: "{n} profiles",
  // Provider categories
  typeRelay: "relay",
  typeVllm: "vLLM local",
  typeRelayOpt: "Relay (relay)",
  typeVllmOpt: "vLLM local (vllm)",
  // Editor
  editorLoad: "Loading profile…",
  editorLoadError: "Failed to load profile: {err}",
  back: "Back",
  editTitle: "Edit profile: {id}",
  newTitle: "New profile",
  fId: "Profile ID",
  fIdReadOnly: "(read-only)",
  fIdPh: "my-provider-a",
  idAutoHint: "Auto-generated from the name — still editable.",
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
  modelHintCodex: "Stored as model in the generated Codex config.toml.",
  modelHintClaude: "Injected as ANTHROPIC_MODEL at launch.",
  fEffort: "Reasoning effort (optional)",
  effortPh: "e.g. xhigh, medium, low",
  effortHint:
    "Claude only — injected as CLAUDE_CODE_EFFORT_LEVEL. Set it when the server rejects the CLI's default (some vLLM builds accept only xhigh / medium / low).",
  fAuthMode: "Key mode",
  authBearer: "Bearer — ANTHROPIC_AUTH_TOKEN",
  authApiKey: "x-api-key — ANTHROPIC_API_KEY",
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
  launchNoProfiles: "No profiles available. Add one from the profile list first.",
  fProfile: "Profile",
  fWorkspace: "Workspace",
  wsPh: "leave empty for current directory",
  pickDir: "Browse…",
  clearWs: "Clear",
  launchBtnCodex: "Launch Codex",
  launchBtnClaude: "Launch Claude",
  launching: "Launching…",
  launchFailed: "Launch failed: {err}",
  launchPrepared: "Launch prepared",
  launchStartedCodex: "Codex started in a new terminal window.",
  launchStartedClaude: "Claude started in a new terminal window.",
  backToList: "Back to profile list",
  // Sessions pool page
  sessionsTitle: "Session pool",
  sessionsEmpty: "No sessions yet.",
  sessionsEmptyHint: "Launch a profile once — its conversations will show up here.",
  sessionRefresh: "Refresh",
  sessionResume: "Resume",
  resuming: "Resuming…",
  sessionResumed: "Session resumed in a new terminal window.",
  sessionResumeFail: "Resume failed: {err}",
  sessionNoProfile: "Profile deleted — cannot resume",
  sessionCwd: "workspace",
  relJustNow: "just now",
  relMinAgo: "{n} min ago",
  relHourAgo: "{n} h ago",
  relDayAgo: "{n} d ago",
} as const;

export type TKey = keyof typeof EN;

const ZH: Record<TKey, string> = {
  // App shell
  topbarSub: "Codex / Claude 多服务商配置管理",
  langToggleTitle: "切换语言",
  navProfiles: "配置档案",
  navSessions: "会话池",
  navAbout: "关于",
  // About page
  aboutDesc:
    "以独立的隔离环境启动 Codex / Claude：路由互相独立、会话可恢复、API 密钥绝不落盘。",
  aboutCliTitle: "CLI 环境",
  aboutRecheck: "重新检测",
  aboutInstalled: "已安装",
  aboutMissing: "未检测到",
  aboutInstallHint: "请先安装，然后点「重新检测」：",
  aboutCodexName: "Codex CLI（OpenAI 协议）",
  aboutClaudeName: "Claude CLI（Anthropic 协议）",
  aboutDataTitle: "数据存储",
  aboutDataDir: "配置档案与会话数据位于",
  // List view
  profilesTitle: "配置档案",
  addBtn: "+ 新建",
  listError: "加载配置档案失败：{err}",
  emptyState: "还没有配置档案。",
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
  confirmDelete: "删除配置档案 “{id}”？\n将移除其 TOML 文件。",
  toastDeleted: "已删除 {id}",
  toastDeleteFailed: "删除失败：{err}",
  toastSaved: "已保存",
  statusLoading: "正在加载状态…",
  profilesCount: "{n} 个配置档案",
  // Provider categories
  typeRelay: "中转站",
  typeVllm: "vLLM 本地",
  typeRelayOpt: "中转站 (relay)",
  typeVllmOpt: "vLLM 本地 (vllm)",
  // Editor
  editorLoad: "正在加载配置档案…",
  editorLoadError: "加载配置档案失败：{err}",
  back: "返回",
  editTitle: "编辑配置档案：{id}",
  newTitle: "新建配置档案",
  fId: "档案 ID",
  fIdReadOnly: "（只读）",
  fIdPh: "my-provider-a",
  idAutoHint: "根据名称自动生成，仍可修改。",
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
  modelHintCodex: "将写入生成的 Codex config.toml（model 字段）。",
  modelHintClaude: "启动时作为 ANTHROPIC_MODEL 环境变量注入。",
  fEffort: "推理强度（可选）",
  effortPh: "如 xhigh / medium / low",
  effortHint:
    "仅 Claude——以 CLAUDE_CODE_EFFORT_LEVEL 注入。当服务器不接受 CLI 默认值时设置（部分 vLLM 只接受 xhigh / medium / low）。",
  fAuthMode: "Key 传递方式",
  authBearer: "Bearer — ANTHROPIC_AUTH_TOKEN",
  authApiKey: "x-api-key — ANTHROPIC_API_KEY",
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
  launchNoProfiles: "没有可用配置档案，请先在列表页新建。",
  fProfile: "配置档案",
  fWorkspace: "工作目录",
  wsPh: "留空则使用当前目录",
  pickDir: "选择目录…",
  clearWs: "清空",
  launchBtnCodex: "启动 Codex",
  launchBtnClaude: "启动 Claude",
  launching: "启动中…",
  launchFailed: "启动失败：{err}",
  launchPrepared: "启动准备就绪",
  launchStartedCodex: "已在新终端窗口中启动 Codex。",
  launchStartedClaude: "已在新终端窗口中启动 Claude。",
  backToList: "返回配置档案列表",
  // Sessions pool page
  sessionsTitle: "会话池",
  sessionsEmpty: "还没有历史会话。",
  sessionsEmptyHint: "先启动一次配置档案，其对话记录会出现在这里。",
  sessionRefresh: "刷新",
  sessionResume: "恢复",
  resuming: "恢复中…",
  sessionResumed: "已在新终端窗口中恢复会话。",
  sessionResumeFail: "恢复失败：{err}",
  sessionNoProfile: "配置档案已删除，无法恢复",
  sessionCwd: "工作目录",
  relJustNow: "刚刚",
  relMinAgo: "{n} 分钟前",
  relHourAgo: "{n} 小时前",
  relDayAgo: "{n} 天前",
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
