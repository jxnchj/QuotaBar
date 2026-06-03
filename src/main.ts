// QuotaBar frontend: popover with provider progress + settings.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";

type View = "main" | "settings";
let currentView: View = "main";

const app = document.getElementById("app")!;

interface AppConfig {
  claude_session_key: string | null;
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
  | { status: "ok"; five_hour?: RateWindow | null; seven_day?: RateWindow | null; seven_day_opus?: RateWindow | null; seven_day_sonnet?: RateWindow | null; fetched_at: string }
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

function fmtTokens(n: number): string {
  if (n >= 1_000_000_000) return `${(n / 1_000_000_000).toFixed(2)}B`;
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return `${n}`;
}

async function saveClaudeSessionKey(key: string): Promise<void> {
  await invoke("set_claude_session_key", { key });
}

async function refreshClaudeNow(): Promise<void> {
  try {
    await invoke("refresh_claude_now");
  } catch (e) {
    console.error("refresh failed:", e);
  }
}

function renderClaudeCard(snap: ClaudeSnapshot, configured: boolean): string {
  if (!configured) {
    return `
      <div class="empty-state">
        还未配置 Claude sessionKey。<br/>
        <button class="btn-link" id="open-settings">点这里录入</button>
      </div>`;
  }

  switch (snap.status) {
    case "idle":
    case "loading":
      return `<div class="card">
        <div class="card-header">
          <div class="card-title">Claude</div>
          <div class="card-subtitle">加载中…</div>
        </div>
      </div>`;
    case "error":
      return `<div class="card">
        <div class="card-header">
          <div class="card-title">Claude</div>
          <div class="card-subtitle" style="color: var(--danger);">失败</div>
        </div>
        <div class="card-subtitle" style="margin-top: 4px;">${escapeHtml(snap.message)}</div>
      </div>`;
    case "ok": {
      const w5 = snap.five_hour;
      const w7 = snap.seven_day;
      const wo = snap.seven_day_opus;
      const ws = snap.seven_day_sonnet;
      const fetched = new Date(snap.fetched_at);
      const ago = Math.max(0, Math.floor((Date.now() - fetched.getTime()) / 1000));
      const agoText = ago < 60 ? `${ago}s ago` : `${Math.floor(ago / 60)}m ago`;
      return `<div class="card">
        <div class="card-header">
          <div class="card-title">Claude</div>
          <div class="card-subtitle">${agoText}</div>
        </div>
        ${progressBar("5h 滚动窗口", w5)}
        ${progressBar("7d 全部模型", w7)}
        ${progressBar("7d Sonnet", ws)}
        ${progressBar("7d Opus", wo)}
      </div>`;
    }
    case "stale": {
      const d = snap.data;
      return `<div class="card" style="border-color: var(--warn);">
        <div class="card-header">
          <div class="card-title">Claude</div>
          <div class="card-subtitle" style="color: var(--warn);">数据过期</div>
        </div>
        ${progressBar("5h 滚动窗口", d.five_hour)}
        ${progressBar("7d 总额度", d.seven_day)}
        ${progressBar("7d Opus 单独", d.seven_day_opus)}
        <div class="card-subtitle" style="margin-top: 6px; color: var(--warn);">${escapeHtml(snap.error)}</div>
      </div>`;
    }
  }
}

function renderLocalCard(local: ClaudeLocalSummary | null): string {
  if (!local) return "";
  const today = local.today.input + local.today.output + local.today.cache_create + local.today.cache_read;
  if (today === 0 && local.last_7_days_total === 0) return ""; // no local data, hide
  const topModel = local.top_models[0]?.[0] ?? "—";
  // Sparkline-ish: just show 7 days as text bars
  const max = Math.max(1, ...local.daily.map(d => d.input + d.output + d.cache_create + d.cache_read));
  const bars = local.daily.map(d => {
    const t = d.input + d.output + d.cache_create + d.cache_read;
    const blocks = Math.round((t / max) * 8);
    return "▁▂▃▄▅▆▇█"[blocks] ?? "▁";
  }).join("");
  return `<div class="card">
    <div class="card-header">
      <div class="card-title">Claude Code 本地</div>
      <div class="card-subtitle" style="font-family: ui-monospace, monospace;">${bars}</div>
    </div>
    <div class="progress-label" style="margin-top: 4px;">
      <span>今日已消耗</span>
      <span>${escapeHtml(fmtTokens(today))} tokens</span>
    </div>
    <div class="progress-label">
      <span>近 7 日累计</span>
      <span>${escapeHtml(fmtTokens(local.last_7_days_total))} tokens</span>
    </div>
    <div class="progress-label">
      <span>主用模型</span>
      <span style="font-family: ui-monospace, monospace; font-size: 10px;">${escapeHtml(topModel)}</span>
    </div>
  </div>`;
}

async function renderMain() {
  const [config, snap, local] = await Promise.all([
    loadConfig(),
    loadClaudeSnapshot(),
    loadClaudeLocal(),
  ]);
  const hasClaude = !!config.claude_session_key;

  app.innerHTML = `
    ${renderClaudeCard(snap, hasClaude)}
    ${renderLocalCard(local)}
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
      <div style="display: flex; gap: 8px; justify-content: flex-end;">
        <button class="btn-link" id="cancel">取消</button>
        <button class="primary" id="save">保存</button>
      </div>
    </div>
  `;

  document.getElementById("cancel")?.addEventListener("click", () => {
    currentView = "main";
    render();
  });
  document.getElementById("save")?.addEventListener("click", async () => {
    const input = document.getElementById("claude-key") as HTMLInputElement;
    const value = input.value.trim();
    await saveClaudeSessionKey(value);
    if (value) {
      // Trigger immediate fetch so user sees data right away.
      refreshClaudeNow();
    }
    currentView = "main";
    await render();
  });
}

async function autoResize() {
  // Measure actual rendered height and shrink window to fit.
  // requestAnimationFrame waits one frame for layout to settle after innerHTML changes.
  await new Promise(requestAnimationFrame);
  const h = Math.max(100, Math.ceil(document.body.scrollHeight));
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

window.addEventListener("DOMContentLoaded", () => {
  render();
});
