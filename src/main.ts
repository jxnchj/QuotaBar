// QuotaBar frontend: popover UI with provider cards + settings.
import { invoke } from "@tauri-apps/api/core";

type View = "main" | "settings";
let currentView: View = "main";

const app = document.getElementById("app")!;

function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]!),
  );
}

interface AppConfig {
  claude_session_key: string | null;
}

async function loadConfig(): Promise<AppConfig> {
  return await invoke<AppConfig>("get_config");
}

async function saveClaudeSessionKey(key: string): Promise<void> {
  await invoke("set_claude_session_key", { key });
}

async function render() {
  if (currentView === "settings") {
    await renderSettings();
  } else {
    await renderMain();
  }
}

async function renderMain() {
  const config = await loadConfig();
  const hasClaude = !!config.claude_session_key;

  app.innerHTML = `
    ${hasClaude
      ? `<div class="card">
          <div class="card-header">
            <div class="card-title">Claude</div>
            <div class="card-subtitle">设置成功，下一步接 API</div>
          </div>
          <div class="card-subtitle">sessionKey 已保存，本地配置文件 ~/Library/Application Support/QuotaBar/config.json</div>
        </div>`
      : `<div class="empty-state">
          还未配置 Claude sessionKey。<br/>
          <button class="btn-link" id="open-settings">点这里录入</button>
        </div>`
    }
    <div class="footer">
      <span>QuotaBar v0.1</span>
      <button class="btn-link" id="footer-settings">设置</button>
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
    currentView = "main";
    await render();
  });
}

window.addEventListener("DOMContentLoaded", () => {
  render();
});
