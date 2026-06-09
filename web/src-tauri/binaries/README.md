# Sidecar binaries

The desktop app's **local mode** runs the XenoClaw backend as a Tauri sidecar.
Tauri looks for a platform-suffixed binary in this directory:

```
binaries/xenoclaw-<target-triple>      # Linux/macOS
binaries/xenoclaw-<target-triple>.exe   # Windows
```

`<target-triple>` is your Rust host triple, e.g. `x86_64-unknown-linux-gnu` or
`x86_64-pc-windows-msvc` (find it with `rustc -Vv | grep host`).

## Produce it

From the repo root, build the backend and copy it here with the right name:

```bash
npm --prefix web run sidecar:build        # cross-platform helper
# or manually:
cargo build -p xenoclaw --release
triple=$(rustc -Vv | sed -n 's/host: //p')
cp target/release/xenoclaw "web/src-tauri/binaries/xenoclaw-$triple"
```

The actual binaries are git-ignored (see `.gitignore`) — only this README is
tracked. Run the helper before `npm run tauri:build` / `tauri:dev` if you want
local mode. Remote-only builds don't need it, but `externalBin` in
`tauri.conf.json` expects the file to exist at bundle time; comment that line
out for a remote-only build, or run the helper.
