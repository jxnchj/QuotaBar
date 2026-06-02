// QuotaBar frontend: popover with provider progress + settings.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

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
  return `
    <div class="progress">
      <div class="progress-label">
        <span>${escapeHtml(label)}</span>
        <span>${pct.toFixed(0)}% · ${escapeHtml(fmtRelativeFromNow(w.resets_at))}</span>
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

async function renderMain() {
  const [config, snap] = await Promise.all([loadConfig(), loadClaudeSnapshot()]);
  const hasClaude = !!config.claude_session_key;

  app.innerHTML = `
    ${renderClaudeCard(snap, hasClaude)}
    <div class="footer">
      <span>QuotaBar v0.1</span>
      <span>
        ${hasClaude ? `<button class="btn-link" id="refresh">刷新</button> · ` : ""}
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
    // Snapshot update will arrive via event; manual re-render is just for instant feedback.
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

async function render() {
  if (currentView === "settings") {
    await renderSettings();
  } else {
    await renderMain();
  }
}

// Subscribe to backend pushes (Rust emits "claude-snapshot-updated" after each fetch).
listen("claude-snapshot-updated", () => {
  if (currentView === "main") render();
});

window.addEventListener("DOMContentLoaded", () => {
  render();
});
