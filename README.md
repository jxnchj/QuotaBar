# QuotaBar

A lightweight macOS **menu-bar** app that shows your remaining LLM subscription
quota at a glance — **Claude**, **Codex (ChatGPT)**, and **Kimi** — without
opening any settings page.

<!-- Add a screenshot here, e.g. docs/screenshot.png -->

## Features

- **Three providers, one panel**
  - **Claude** (claude.ai) — 5h / 7d usage windows, subscription tier (Max / Pro …),
    plus local **Claude Code** token history.
  - **Codex** (ChatGPT) — 5h / 7d rate-limit windows and plan, plus local
    **Codex CLI** token history.
  - **Kimi** (kimi.com) — weekly request quota and 5-hour rate window.
- **Compact menu-bar summary** — per-provider percentage (e.g. `C 87% · Cx 44%`),
  capped to two so it never crowds the menu bar. Choose which providers show.
- **Click-through popover** — a frosted-glass panel that **auto-sizes to its
  content** (never needs scrolling), with progress bars and a 7-day local-usage
  mini bar chart.
- Refreshes every 5 minutes in the background.

## How it reads your usage — and privacy

QuotaBar reads everything **locally**. The only network calls are to each
provider's own usage API; **nothing is sent to any third party**.

| Provider | Source |
| --- | --- |
| Claude | Your `sessionKey` cookie, pasted once in Settings (stored in `~/Library/Application Support/QuotaBar/config.json`). |
| Codex | The JWT in `~/.codex/auth.json` (written by Codex CLI at login). Codex does **not** need to be running. |
| Kimi | The `kimi-auth` cookie, auto-read from Chrome (decrypted via the macOS keychain — a one-time authorization prompt), or pasted manually. |
| Local token history | Read-only parsing of Claude Code logs (`~/.claude/projects/**/*.jsonl`) and Codex rollouts (`~/.codex/sessions/**/*.jsonl`). |

Credentials never leave your machine.

## Tech stack

- [**Tauri 2**](https://tauri.app) — Rust backend + web frontend.
- Frontend: **vanilla TypeScript + Vite** (no UI framework).
- HTTP: [`wreq`](https://crates.io/crates/wreq) with browser TLS impersonation
  (to pass Cloudflare on claude.ai).
- Browser cookie reading: [`rookie`](https://crates.io/crates/rookie).

## Build & run

**Prerequisites:** macOS 12+, [Rust](https://rustup.rs), Node 18+, Xcode Command
Line Tools, and **CMake** (`brew install cmake` — required to build BoringSSL for
`wreq`).

```bash
npm install
npm run tauri dev      # run in development
npm run tauri build    # build a release .app / .dmg
```

## Usage

QuotaBar lives in the menu bar (no Dock icon):

1. The menu-bar text shows a compact summary, e.g. `C 87% · Cx 44%`.
2. **Click it** to open the popover with full per-provider details.
3. Open **Settings** (bottom of the popover) to:
   - paste your Claude `sessionKey` (claude.ai → DevTools → Application → Cookies → `sessionKey`);
   - optionally paste a Kimi `kimi-auth` token — otherwise it is read from Chrome
     automatically (approve the one-time keychain prompt);
   - choose which providers appear in the menu-bar summary (up to two).
4. Codex needs no setup beyond having logged in with the Codex CLI.

Usage refreshes automatically every few minutes.

## Acknowledgements

The Codex and Kimi usage endpoints were worked out with reference to the
open-source [CodexBar](https://github.com/steipete/CodexBar) by @steipete.
QuotaBar's implementation is independent and written in Rust.

## License

[MIT](LICENSE)
