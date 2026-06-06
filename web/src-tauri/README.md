# XenoClaw Desktop

A cross-platform desktop client for the XenoClaw agent runtime, built with
**Tauri 2** and targeting **Windows and Linux**. It wraps and restyles the
existing React/Vite frontend (it does **not** fork the UI) and connects to a
XenoClaw backend either **remotely** (a VPS-hosted instance) or **locally** (the
backend bundled and run as a Tauri sidecar).

## How it fits the monorepo

```
web/                     # the existing Vite + React frontend (shared)
├── src/                 # ← reused as-is; the desktop app is just another host
│   ├── lib/
│   │   ├── tauri.ts        # typed window.__TAURI__ shim (no-ops on the web)
│   │   └── backend.ts      # server profiles + active-server URL resolution
│   ├── hooks/
│   │   ├── useServer.ts     # profiles, live connection status, local sidecar
│   │   └── useTheme.ts      # light/dark/system theme
│   └── components/
│       ├── Titlebar/        # native window chrome (desktop only)
│       ├── Markdown/        # chat markdown + code blocks
│       └── DesktopSettings/ # server profiles, local backend, updates
└── src-tauri/           # ← this crate: the native desktop shell
    ├── src/lib.rs          # window, tray, notifications, updater, commands
    ├── src/sidecar.rs      # local backend (sidecar) lifecycle
    ├── tauri.conf.json     # window, bundle, updater config
    ├── capabilities/       # permission set for the main window
    ├── icons/              # generated app icons (coral "spark" mark)
    ├── binaries/           # staged backend sidecar (git-ignored)
    └── scripts/            # icon + sidecar build helpers
```

The same `web/dist` build serves the browser (the backend serves it on its API
port) **and** the desktop webview. The frontend detects Tauri at runtime
(`isTauri()`); in a plain browser every desktop-only path is inert, so the web
UI and the TUI are unaffected.

### Web vs. desktop request routing

- **Web build:** `getApiBase()` returns `''`, so the existing relative
  `/api/...` URLs resolve same-origin (and the Vite dev proxy still applies).
  Behaviour is identical to before the desktop app existed.
- **Desktop build:** requests resolve against the **active server profile's**
  URL (`https://…` / `wss://…`), with the saved bearer token attached. Profiles,
  the active selection, theme, and per-server tokens persist in the webview's
  localStorage (kept in the app data dir across restarts).

## Prerequisites

- Node 18+ and Rust (stable) with the Tauri 2 prerequisites for your OS:
  <https://v2.tauri.app/start/prerequisites/>
- **Linux:** `libwebkit2gtk-4.1-dev libgtk-3-dev libappindicator3-dev librsvg2-dev libsoup-3.0-dev patchelf`
- **Windows:** WebView2 (preinstalled on Win 10/11) + the MSVC build tools.

## Dev setup

```bash
cd web
npm install
npm run icons:generate     # one-time: generate the app icon set

# Optional — only needed for LOCAL backend mode:
npm run sidecar:build      # builds `xenoclaw` and stages it in binaries/

npm run tauri:dev          # launches the desktop app with HMR
```

`npm run tauri:dev` runs the Vite dev server (`beforeDevCommand`) and opens the
native window pointing at it. Edit the React code and it hot-reloads; edit the
Rust in `src-tauri/` and Tauri rebuilds the shell.

> If you only use **remote** mode, the sidecar binary isn't required at runtime —
> but `externalBin` in `tauri.conf.json` expects it at *bundle* time. Either run
> `npm run sidecar:build` first, or comment out the `externalBin` line for a
> remote-only build.

## Local vs. remote

Both modes are first-class and switchable at runtime (title-bar server switcher,
or Settings → Servers), persisted across restarts.

- **Remote (primary).** Settings → Servers → *Add server*: give it a name, the
  base URL (`https://xenoclaw.example.com`), and optionally an API key/token.
  Select it to make it active. The title bar shows the active server and a live
  connection dot (connected / connecting / offline) with automatic WebSocket
  reconnect.
- **Local.** Settings → Local backend → *Start*. The app picks a free port,
  launches the bundled `xenoclaw serve` bound to `127.0.0.1:<port>` (via the
  `XENOCLAW_API_PORT` env override the backend honours), streams its logs, and
  health-checks `/api/v1/health` before the indicator goes green. Requires a
  configured backend — run `xenoclaw setup` once. Stop it from the same panel;
  it's also killed automatically when the app exits.

## Building installers

```bash
cd web
npm run sidecar:build      # stage the backend sidecar for the current host
npm run tauri:build        # → src-tauri/target/release/bundle/
```

Targets (configured in `tauri.conf.json`):

| Platform | Artifacts |
|----------|-----------|
| Windows  | NSIS installer (`.exe`) — per-machine, Start Menu entry |
| Linux    | `.AppImage` and `.deb` |

Limit targets with `npm run tauri:build -- --bundles nsis` (or `deb,appimage`).

### CLI shortcuts (`xenoclaw`)

The `xenoclaw` binary wraps the whole npm/Tauri flow so you don't have to run
the steps by hand:

```bash
# Build the installers and install for the current user
# (AppImage → ~/.local with a .desktop launcher, or a .deb via dpkg)
xenoclaw install desktop
xenoclaw install desktop --build-only        # build only; print the bundle path
xenoclaw install desktop --dir /path/to/repo # point at a specific checkout

# No source on disk (installed only the prebuilt binary)? It clones via git
# into ~/.xenoclaw/desktop-src and builds from there:
xenoclaw install desktop --ref main          # branch/tag/PR to build
xenoclaw install desktop --clone             # force a fresh clone even in a checkout
xenoclaw install desktop --repo <url>        # use a fork

# Build the desktop app from a branch/PR worktree (your checkout is untouched)
xenoclaw try my-branch --desktop             # build installers
xenoclaw try my-branch --desktop --exec      # launch it live (tauri dev)
xenoclaw try my-branch --desktop --no-run    # frontend + sidecar only
```

Both run `npm install → icons:generate → sidecar:build → tauri build` for you,
so the Tauri prerequisites (Node, Rust, and the system WebKit/GTK libs) still
need to be installed. **Run them as your normal user, not `sudo`** — only the
final `.deb` install elevates (and the command does that itself). A prior `sudo`
run leaves root-owned `node_modules`/`target`; the command now detects that and
prints the `chown`/`--clone` fix instead of a cryptic EACCES.

### Arch Linux (AUR)

A starting `PKGBUILD` is provided at `packaging/aur/PKGBUILD`. It builds from
source; adjust `pkgver`/`source`/`sha256sums` for a release and submit to the
AUR as `xenoclaw-desktop`.

## Auto-updater

`tauri.conf.json` configures the updater to read a signed `latest.json` from the
project's GitHub releases. To enable real updates:

1. Generate a signing key once:
   ```bash
   npm run tauri signer generate -- -w ~/.tauri/xenoclaw.key
   ```
2. Put the **public** key in `tauri.conf.json` → `plugins.updater.pubkey`
   (replace the placeholder generated by `icons:generate`).
3. Build with the private key in the environment so artifacts are signed:
   ```bash
   export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/xenoclaw.key)"
   export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="…"
   npm run tauri:build
   ```

The `desktop-build` GitHub workflow does this on tag pushes (keys come from the
`TAURI_SIGNING_PRIVATE_KEY*` secrets) and publishes a draft release containing
the installers and `latest.json`. The in-app **Settings → About & updates →
Check for updates** button queries the configured endpoint.

> Until you replace the placeholder pubkey + add the secrets, the build still
> works but update *verification* will fail by design.

## Code signing (optional / stubbed)

- **Windows:** sign the NSIS `.exe` with `signtool` using an Authenticode
  certificate. Set `bundle.windows.certificateThumbprint` (or sign in CI).
- **Linux:** AppImage/.deb signing is optional; the updater signature above is
  what gates auto-updates.

Both are intentionally left unconfigured here — add your certs in CI secrets.

## Native touches

- **Custom titlebar** (`decorations: false`) with min/max/close, drag region,
  server switcher, connection status, and theme toggle.
- **System tray** — left-click shows the window; the menu offers Show / Hide /
  Quit. Closing the window hides to the tray; Quit exits fully.
- **Native notifications** when a response completes and the window is unfocused.
- **Window size/position** are remembered across restarts
  (`tauri-plugin-window-state`).

## Regenerating icons

`npm run icons:generate` renders the full PNG set + `icon.ico` from a
dependency-free Node script (`scripts/generate-icons.mjs`) — no ImageMagick
required. To use your own artwork instead, drop a 1024×1024 PNG and run
`npm run tauri icon ./your-icon.png`.
