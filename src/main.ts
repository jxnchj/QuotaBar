// QuotaBar frontend: popover with provider progress + settings.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";

type View = "main" | "settings";
let currentView: View = "main";

const app = document.getElementById("app")!;

interface AppConfig {
  claude_session_key: string | null;
  kimi_auth_token: string | null;
  menubar_providers: string[] | null;
}

interface RateWindow {
  percent: number;
  resets_at: string | null;
}

interface ClaudeUsage {
  five_hour: RateWindow | null;
  seven_day: RateWindow | null;
  seven_day_opus: RateWindow | null;
  seven_day_sonnet: RateWindow | null;
  plan: string | null;
  fetched_at: string;
}

interface DailyBucket {
  date: string;
  input: number;
  output: number;
  cache_create: number;
  cache_read: number;
}

interface ClaudeLocalSummary {
  today: DailyBucket;
  last_7_days_total: number;
  daily: DailyBucket[];
  top_models: [string, number][];
  fetched_at: string;
}

type ClaudeSnapshot =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "ok"; five_hour?: RateWindow | null; seven_day?: RateWindow | null; seven_day_opus?: RateWindow | null; seven_day_sonnet?: RateWindow | null; plan?: string | null; fetched_at: string }
  | { status: "error"; message: string }
  | { status: "stale"; data: ClaudeUsage; error: string };

function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]!),
  );
}

function fmtRelativeFromNow(iso: string | null): string {
  if (!iso) return "—";
  const target = new Date(iso).getTime();
  const diff = target - Date.now();
  if (diff <= 0) return "重置中";
  const s = Math.floor(diff / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (h > 24) {
    const d = Math.floor(h / 24);
    const hh = h % 24;
    return `${d}d ${hh}h`;
  }
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m`;
}

function severityClass(pct: number): string {
  if (pct >= 90) return "danger";
  if (pct >= 70) return "warn";
  return "";
}

function progressBar(label: string, w: RateWindow | null | undefined): string {
  if (!w) return "";
  const pct = Math.min(100, Math.max(0, w.percent));
  const cls = severityClass(pct);
  const reset = fmtRelativeFromNow(w.resets_at);
  const resetText = reset === "—" ? "" : ` · ${reset} 后重置`;
  return `
    <div class="progress">
      <div class="progress-label">
        <span>${escapeHtml(label)}</span>
        <span>已用 ${pct.toFixed(0)}%${escapeHtml(resetText)}</span>
      </div>
      <div class="progress-track">
        <div class="progress-fill ${cls}" style="width: ${pct}%"></div>
      </div>
    </div>
  `;
}

async function loadConfig(): Promise<AppConfig> {
  return await invoke<AppConfig>("get_config");
}

async function loadClaudeSnapshot(): Promise<ClaudeSnapshot> {
  return await invoke<ClaudeSnapshot>("get_claude_snapshot");
}

async function loadClaudeLocal(): Promise<ClaudeLocalSummary | null> {
  return await invoke<ClaudeLocalSummary | null>("get_claude_local_summary");
}

interface CodexWindow {
  percent: number;
  window_label: string;
  resets_at: string | null;
}

interface CodexUsage {
  primary: CodexWindow | null;
  secondary: CodexWindow | null;
  plan_type: string | null;
  fetched_at: string;
}

type CodexSnapshot =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "ok" } & CodexUsage
  | { status: "error"; message: string };

async function loadCodexSnapshot(): Promise<CodexSnapshot> {
  return await invoke<CodexSnapshot>("get_codex_snapshot");
}

interface CodexLocalBucket {
  date: string;
  tokens: number;
  sessions: number;
}

interface CodexLocalSummary {
  today_tokens: number;
  today_sessions: number;
  last_7_days_total: number;
  daily: CodexLocalBucket[];
  top_models: [string, number][];
  fetched_at: string;
}

async function loadCodexLocal(): Promise<CodexLocalSummary | null> {
  return await invoke<CodexLocalSummary | null>("get_codex_local_summary");
}

interface KimiWindow {
  percent: number;
  window_label: string;
  used: number | null;
  limit: number | null;
  resets_at: string | null;
}

interface KimiUsage {
  weekly: KimiWindow | null;
  rate_limit: KimiWindow | null;
  fetched_at: string;
}

type KimiSnapshot =
  | { status: "idle" }
  | { status: "loading" }
  | ({ status: "ok" } & KimiUsage)
  | { status: "error"; message: string };

async function loadKimiSnapshot(): Promise<KimiSnapshot> {
  return await invoke<KimiSnapshot>("get_kimi_snapshot");
}

function renderCodexLocalSection(local: CodexLocalSummary | null): string {
  if (!local || (local.today_tokens === 0 && local.last_7_days_total === 0)) return "";
  const max = Math.max(1, ...local.daily.map(d => d.tokens));
  const bars = local.daily.map(d => "▁▂▃▄▅▆▇█"[Math.round((d.tokens / max) * 8)] ?? "▁").join("");
  const topModel = local.top_models[0]?.[0] ?? "—";
  return `
    <div style="border-top: 0.5px solid var(--border); margin-top: 6px; padding-top: 6px;">
      <div class="progress-label" style="margin-bottom:2px;">
        <span style="color:var(--fg-secondary);font-size:11px;">本地用量</span>
        <span style="font-family:ui-monospace,monospace;font-size:10px;color:var(--fg-secondary);">${bars}</span>
      </div>
      <div class="progress-label">
        <span>今日已消耗</span>
        <span>${escapeHtml(fmtTokens(local.today_tokens))} tokens · ${local.today_sessions} 个会话</span>
      </div>
      <div class="progress-label">
        <span>近 7 日累计</span>
        <span>${escapeHtml(fmtTokens(local.last_7_days_total))} tokens</span>
      </div>
      <div class="progress-label">
        <span>主用模型</span>
        <span style="font-family:ui-monospace,monospace;font-size:10px;">${escapeHtml(topModel)}</span>
      </div>
    </div>`;
}

function renderCodexCard(snap: CodexSnapshot, local: CodexLocalSummary | null = null): string {
  const title = `<div class="card-title">Codex</div>`;
  if (snap.status === "idle" || snap.status === "loading") {
    return `<div class="card"><div class="card-header">${title}<div class="card-subtitle">加载中…</div></div></div>`;
  }
  if (snap.status === "error") {
    // Don't show card if just missing auth file (not installed / not logged in)
    if (snap.message.includes("不存在")) return "";
    return `<div class="card"><div class="card-header">${title}
      <div class="card-subtitle" style="color:var(--danger);">失败</div></div>
      <div class="card-subtitle" style="margin-top:4px;">${escapeHtml(snap.message)}</div>
    </div>`;
  }
  // ok
  const ago = Math.max(0, Math.floor((Date.now() - new Date(snap.fetched_at).getTime()) / 1000));
  const agoText = ago < 60 ? `${ago}s ago` : `${Math.floor(ago / 60)}m ago`;

  const planText = snap.plan_type ? snap.plan_type.charAt(0).toUpperCase() + snap.plan_type.slice(1) : null;
  const planLabel = planText ? `<span class="card-subtitle">${escapeHtml(planText)} · ${agoText}</span>` : `<span class="card-subtitle">${agoText}</span>`;

  const barHtml = (w: CodexWindow | null, label: string) => {
    if (!w) return "";
    const pct = Math.min(100, Math.max(0, w.percent));
    const cls = pct >= 90 ? "danger" : pct >= 70 ? "warn" : "";
    const reset = fmtRelativeFromNow(w.resets_at);
    const resetText = reset === "—" ? "" : ` · ${reset} 后重置`;
    return `<div class="progress">
      <div class="progress-label">
        <span>${escapeHtml(label)} (${escapeHtml(w.window_label)})</span>
        <span>已用 ${pct.toFixed(0)}%${escapeHtml(resetText)}</span>
      </div>
      <div class="progress-track"><div class="progress-fill ${cls}" style="width:${pct}%"></div></div>
    </div>`;
  };

  return `<div class="card">
    <div class="card-header">${title}${planLabel}</div>
    ${barHtml(snap.primary, "5h 滚动窗口")}
    ${barHtml(snap.secondary, "7d 周额度")}
    ${renderCodexLocalSection(local)}
  </div>`;
}

function renderKimiCard(snap: KimiSnapshot): string {
  const title = `<div class="card-title">Kimi</div>`;
  // Idle = no token configured yet → keep the card hidden (opt-in provider).
  if (snap.status === "idle") return "";
  if (snap.status === "loading") {
    return `<div class="card"><div class="card-header">${title}<div class="card-subtitle">加载中…</div></div></div>`;
  }
  if (snap.status === "error") {
    return `<div class="card"><div class="card-header">${title}
      <div class="card-subtitle" style="color:var(--danger);">失败</div></div>
      <div class="card-subtitle" style="margin-top:4px;">${escapeHtml(snap.message)}</div>
    </div>`;
  }
  // ok
  const ago = Math.max(0, Math.floor((Date.now() - new Date(snap.fetched_at).getTime()) / 1000));
  const agoText = ago < 60 ? `${ago}s ago` : `${Math.floor(ago / 60)}m ago`;

  const kbar = (w: KimiWindow | null, label: string) => {
    if (!w) return "";
    const pct = Math.min(100, Math.max(0, w.percent));
    const cls = pct >= 90 ? "danger" : pct >= 70 ? "warn" : "";
    const reset = fmtRelativeFromNow(w.resets_at);
    const resetText = reset === "—" ? "" : ` · ${reset} 后重置`;
    const count = w.used != null && w.limit != null ? ` (${w.used}/${w.limit})` : "";
    return `<div class="progress">
      <div class="progress-label">
        <span>${escapeHtml(label)}</span>
        <span>已用 ${pct.toFixed(0)}%${escapeHtml(count)}${escapeHtml(resetText)}</span>
      </div>
      <div class="progress-track"><div class="progress-fill ${cls}" style="width:${pct}%"></div></div>
    </div>`;
  };

  return `<div class="card">
    <div class="card-header">${title}<div class="card-subtitle">${agoText}</div></div>
    ${kbar(snap.rate_limit, "5h 速率")}
    ${kbar(snap.weekly, "周额度（请求）")}
  </div>`;
}

function fmtTokens(n: number): string {
  if (n >= 1_000_000_000) return `${(n / 1_000_000_000).toFixed(2)}B`;
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return `${n}`;
}

async function saveClaudeSessionKey(key: string): Promise<void> {
  await invoke("set_claude_session_key", { key });
}

async function setMenubarProviders(providers: string[]): Promise<void> {
  await invoke("set_menubar_providers", { providers });
}

async function saveKimiToken(token: string): Promise<void> {
  await invoke("set_kimi_token", { token });
}

// Providers selectable for the menubar title. At most 2 may be shown at once.
// With no saved selection (auto), the first two are displayed by default.
const MENUBAR_PROVIDERS: { id: string; label: string }[] = [
  { id: "claude", label: "Claude" },
  { id: "codex", label: "Codex" },
  { id: "kimi", label: "Kimi" },
];
const MENUBAR_MAX = 2;

async function refreshClaudeNow(): Promise<void> {
  try {
    await invoke("refresh_claude_now");
  } catch (e) {
    console.error("refresh failed:", e);
  }
}

// Inline "Claude Code local usage" section, appended inside the Claude card
// (mirrors renderCodexLocalSection so both providers read the same way).
function renderClaudeLocalSection(local: ClaudeLocalSummary | null): string {
  if (!local) return "";
  const today = local.today.input + local.today.output + local.today.cache_create + local.today.cache_read;
  if (today === 0 && local.last_7_days_total === 0) return "";
  const max = Math.max(1, ...local.daily.map(d => d.input + d.output + d.cache_create + d.cache_read));
  const bars = local.daily.map(d => {
    const t = d.input + d.output + d.cache_create + d.cache_read;
    return "▁▂▃▄▅▆▇█"[Math.round((t / max) * 8)] ?? "▁";
  }).join("");
  const topModel = local.top_models[0]?.[0] ?? "—";
  return `
    <div style="border-top: 0.5px solid var(--border); margin-top: 6px; padding-top: 6px;">
      <div class="progress-label" style="margin-bottom:2px;">
        <span style="color:var(--fg-secondary);font-size:11px;">本地用量 · Claude Code</span>
        <span style="font-family:ui-monospace,monospace;font-size:10px;color:var(--fg-secondary);">${bars}</span>
      </div>
      <div class="progress-label">
        <span>今日已消耗</span>
        <span>${escapeHtml(fmtTokens(today))} tokens</span>
      </div>
      <div class="progress-label">
        <span>近 7 日累计</span>
        <span>${escapeHtml(fmtTokens(local.last_7_days_total))} tokens</span>
      </div>
      <div class="progress-label">
        <span>主用模型</span>
        <span style="font-family:ui-monospace,monospace;font-size:10px;">${escapeHtml(topModel)}</span>
      </div>
    </div>`;
}

function renderClaudeCard(
  snap: ClaudeSnapshot,
  configured: boolean,
  local: ClaudeLocalSummary | null = null,
): string {
  if (!configured) {
    return `
      <div class="empty-state">
        还未配置 Claude sessionKey。<br/>
        <button class="btn-link" id="open-settings">点这里录入</button>
      </div>`;
  }

  const agoOf = (iso: string) => {
    const ago = Math.max(0, Math.floor((Date.now() - new Date(iso).getTime()) / 1000));
    return ago < 60 ? `${ago}s ago` : `${Math.floor(ago / 60)}m ago`;
  };

  switch (snap.status) {
    case "idle":
    case "loading":
      return `<div class="card">
        <div class="card-header">
          <div class="card-title">Claude</div>
          <div class="card-subtitle">加载中…</div>
        </div>
        ${renderClaudeLocalSection(local)}
      </div>`;
    case "error":
      return `<div class="card">
        <div class="card-header">
          <div class="card-title">Claude</div>
          <div class="card-subtitle" style="color: var(--danger);">失败</div>
        </div>
        <div class="card-subtitle" style="margin-top: 4px;">${escapeHtml(snap.message)}</div>
        ${renderClaudeLocalSection(local)}
      </div>`;
    case "ok": {
      const sub = snap.plan ? `${escapeHtml(snap.plan)} · ${agoOf(snap.fetched_at)}` : agoOf(snap.fetched_at);
      return `<div class="card">
        <div class="card-header">
          <div class="card-title">Claude</div>
          <div class="card-subtitle">${sub}</div>
        </div>
        ${progressBar("5h 滚动窗口", snap.five_hour)}
        ${progressBar("7d 全部模型", snap.seven_day)}
        ${progressBar("7d Sonnet", snap.seven_day_sonnet)}
        ${progressBar("7d Opus", snap.seven_day_opus)}
        ${renderClaudeLocalSection(local)}
      </div>`;
    }
    case "stale": {
      const d = snap.data;
      const sub = d.plan ? `${escapeHtml(d.plan)} · 数据过期` : "数据过期";
      return `<div class="card" style="border-color: var(--warn);">
        <div class="card-header">
          <div class="card-title">Claude</div>
          <div class="card-subtitle" style="color: var(--warn);">${sub}</div>
        </div>
        ${progressBar("5h 滚动窗口", d.five_hour)}
        ${progressBar("7d 全部模型", d.seven_day)}
        ${progressBar("7d Sonnet", d.seven_day_sonnet)}
        ${progressBar("7d Opus", d.seven_day_opus)}
        <div class="card-subtitle" style="margin-top: 6px; color: var(--warn);">${escapeHtml(snap.error)}</div>
        ${renderClaudeLocalSection(local)}
      </div>`;
    }
  }
}

async function renderMain() {
  const [config, snap, local, codex, codexLocal, kimi] = await Promise.all([
    loadConfig(),
    loadClaudeSnapshot(),
    loadClaudeLocal(),
    loadCodexSnapshot(),
    loadCodexLocal(),
    loadKimiSnapshot(),
  ]);
  const hasClaude = !!config.claude_session_key;

  app.innerHTML = `
    ${renderClaudeCard(snap, hasClaude, local)}
    ${renderCodexCard(codex, codexLocal)}
    ${renderKimiCard(kimi)}
    <div class="footer">
      <span>QuotaBar v0.1</span>
      <span>
        <button class="btn-link" id="refresh">刷新</button> ·
        <button class="btn-link" id="footer-settings">设置</button>
      </span>
    </div>
  `;

  document.getElementById("open-settings")?.addEventListener("click", () => {
    currentView = "settings";
    render();
  });
  document.getElementById("footer-settings")?.addEventListener("click", () => {
    currentView = "settings";
    render();
  });
  document.getElementById("refresh")?.addEventListener("click", async () => {
    await refreshClaudeNow();
    await render();
  });
}

async function renderSettings() {
  const config = await loadConfig();
  const existing = config.claude_session_key ?? "";
  const existingKimi = config.kimi_auth_token ?? "";
  const sel = config.menubar_providers; // null = auto
  // Auto default = the first MENUBAR_MAX providers (matches the backend's cap-to-2).
  const menubarChecked = (id: string) =>
    sel === null
      ? MENUBAR_PROVIDERS.findIndex((p) => p.id === id) < MENUBAR_MAX
      : sel.includes(id);

  app.innerHTML = `
    <div class="settings">
      <div>
        <div class="card-title" style="margin-bottom: 4px;">设置</div>
        <div class="card-subtitle">仅保存在本机，不上传任何服务器</div>
      </div>
      <div>
        <label for="claude-key">Claude sessionKey</label>
        <input type="password" id="claude-key" placeholder="sk-ant-sid01-..." value="${escapeHtml(existing)}" />
        <div class="card-subtitle" style="margin-top: 4px;">
          Chrome 登录 claude.ai → DevTools → Application → Cookies → sessionKey 的 Value
        </div>
      </div>
      <div>
        <label for="kimi-token">Kimi 令牌 (kimi-auth)</label>
        <input type="password" id="kimi-token" placeholder="留空 = 自动从 Chrome 读取" value="${escapeHtml(existingKimi)}" />
        <div class="card-subtitle" style="margin-top: 4px;">
          留空则自动从 Chrome 读取登录态(首次弹钥匙串授权,点「始终允许」)。读不到再手动粘贴:kimi.com → DevTools → Application → Cookies → <code>kimi-auth</code> 的 Value。
        </div>
      </div>
      <div>
        <label>状态栏显示</label>
        <div class="menubar-opts">
          ${MENUBAR_PROVIDERS.map(p => `
            <label class="checkbox">
              <input type="checkbox" class="menubar-provider" value="${p.id}" ${menubarChecked(p.id) ? "checked" : ""} />
              <span>${escapeHtml(p.label)}</span>
            </label>`).join("")}
        </div>
        <div class="card-subtitle" style="margin-top: 4px;">
          最多选 2 个并排显示(如 <code>C 87% · Cx 44%</code>);只勾一个则只显示那一个。
        </div>
      </div>
      <div style="display: flex; gap: 8px; justify-content: flex-end;">
        <button class="btn-link" id="cancel">取消</button>
        <button class="primary" id="save">保存</button>
      </div>
    </div>
  `;

  // Menubar provider toggles save immediately (no need to hit 保存).
  // Enforce the 2-max: once two are checked, grey out the rest.
  const menubarBoxes = Array.from(
    document.querySelectorAll<HTMLInputElement>(".menubar-provider"),
  );
  const enforceMenubarCap = () => {
    const checked = menubarBoxes.filter((b) => b.checked).length;
    menubarBoxes.forEach((b) => {
      b.disabled = !b.checked && checked >= MENUBAR_MAX;
    });
  };
  menubarBoxes.forEach((cb) => {
    cb.addEventListener("change", async () => {
      enforceMenubarCap();
      const chosen = menubarBoxes.filter((x) => x.checked).map((x) => x.value);
      await setMenubarProviders(chosen);
    });
  });
  enforceMenubarCap();

  document.getElementById("cancel")?.addEventListener("click", () => {
    currentView = "main";
    render();
  });
  document.getElementById("save")?.addEventListener("click", async () => {
    const claudeKey = (document.getElementById("claude-key") as HTMLInputElement).value.trim();
    const kimiTok = (document.getElementById("kimi-token") as HTMLInputElement).value.trim();
    await saveClaudeSessionKey(claudeKey);
    await saveKimiToken(kimiTok);
    // refresh_claude_now refreshes all providers (Claude/Codex/Kimi + locals).
    refreshClaudeNow();
    currentView = "main";
    await render();
  });
}

async function autoResize() {
  // Resize the window to exactly fit the content so nothing ever needs scrolling,
  // however many provider cards are showing (Claude / Codex / Kimi).
  // Two frames so layout — including the merged local sections — is fully settled.
  await new Promise(requestAnimationFrame);
  await new Promise(requestAnimationFrame);
  const pop = document.getElementById("popover");
  // #popover carries a 4px margin all around; the window must fit popover + margins.
  const measured = pop
    ? pop.getBoundingClientRect().height + 8
    : document.body.scrollHeight;
  const h = Math.max(120, Math.ceil(measured) + 1);
  try {
    await getCurrentWindow().setSize(new LogicalSize(320, h));
  } catch (e) {
    console.warn("setSize failed:", e);
  }
}

async function render() {
  if (currentView === "settings") {
    await renderSettings();
  } else {
    await renderMain();
  }
  await autoResize();
}

// Subscribe to backend pushes (Rust emits these after each fetch).
listen("claude-snapshot-updated", () => {
  if (currentView === "main") render();
});
listen("claude-local-updated", () => {
  if (currentView === "main") render();
});
listen("codex-snapshot-updated", () => {
  if (currentView === "main") render();
});
listen("codex-local-updated", () => {
  if (currentView === "main") render();
});
listen("kimi-snapshot-updated", () => {
  if (currentView === "main") render();
});

window.addEventListener("DOMContentLoaded", () => {
  render();
});
