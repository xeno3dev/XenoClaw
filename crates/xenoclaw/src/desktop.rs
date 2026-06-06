//! `xenoclaw install desktop` and the shared desktop build pipeline reused by
//! `xenoclaw try --desktop`.
//!
//! The desktop app (Tauri) lives in `web/src-tauri/` and wraps the same React
//! frontend as the web UI (`web/`). Building it is a Node/Tauri flow, not cargo,
//! so this module shells out to the npm scripts defined in `web/package.json`:
//!
//!   npm install            # JS deps (incl. @tauri-apps/cli)
//!   npm run icons:generate # app icon set
//!   npm run sidecar:build  # build `xenoclaw` + stage it as the Tauri sidecar
//!   npm run tauri:build    # produce installers (deb / AppImage / nsis)
//!
//! `install desktop` additionally installs the produced artifact for the current
//! user (AppImage → ~/.local, or a .deb via dpkg).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

/// What to do after staging the frontend + sidecar.
#[derive(Clone, Copy, PartialEq)]
pub enum DesktopMode {
    /// `tauri build` — produce installers/bundles. Returns the bundle dir.
    Bundle,
    /// `tauri dev` — launch the app live against the Vite dev server.
    Dev,
    /// Build the frontend bundle + stage the sidecar only (no Tauri step).
    FrontendOnly,
}

/// Entry point for `xenoclaw install desktop`.
pub fn run_install(dir: Option<PathBuf>, build_only: bool) -> Result<()> {
    let web_dir = find_web_dir(dir.as_deref())?;
    let bundle = build_desktop(&web_dir, DesktopMode::Bundle)?
        .expect("Bundle mode returns the bundle dir");

    if build_only {
        println!("\n✓ Built (skipping install). Installers under:\n  {}", bundle.display());
        return Ok(());
    }
    install_from_bundle(&bundle, &web_dir)
}

/// Shared build pipeline. Returns the bundle dir for [`DesktopMode::Bundle`].
pub fn build_desktop(web_dir: &Path, mode: DesktopMode) -> Result<Option<PathBuf>> {
    if which("npm").is_none() {
        bail!("`npm` not found on PATH — install Node.js (18+) to build the desktop app");
    }
    warn_if_root();
    println!("→ desktop frontend at {}", web_dir.display());

    // JS deps. Prefer `npm ci` (reproducible) when a lockfile is present, but
    // fall back to `npm install` if the lockfile is out of sync.
    println!("→ installing frontend dependencies…");
    ensure_node_modules_writable(web_dir)?;
    if web_dir.join("package-lock.json").exists() {
        if npm(web_dir, &["ci"]).is_err() {
            println!("  (npm ci failed — retrying with npm install)");
            npm(web_dir, &["install"])?;
        }
    } else {
        npm(web_dir, &["install"])?;
    }

    println!("→ generating app icons…");
    npm(web_dir, &["run", "icons:generate"])?;

    println!("→ building backend sidecar…");
    npm(web_dir, &["run", "sidecar:build"])?;

    match mode {
        DesktopMode::FrontendOnly => {
            println!("→ building frontend bundle…");
            npm(web_dir, &["run", "build"])?;
            Ok(None)
        }
        DesktopMode::Dev => {
            println!("\n▶ launching the desktop app (tauri dev) — Ctrl-C to stop\n");
            npm(web_dir, &["run", "tauri:dev"])?;
            Ok(None)
        }
        DesktopMode::Bundle => {
            println!("→ building desktop installers (tauri build)…\n");
            npm(web_dir, &["run", "tauri:build"])?;
            Ok(Some(web_dir.join("src-tauri/target/release/bundle")))
        }
    }
}

/// Locate the `web/` frontend dir from a hint (repo root or web dir) or by
/// walking up from the current directory.
fn find_web_dir(hint: Option<&Path>) -> Result<PathBuf> {
    if let Some(h) = hint {
        if h.join("src-tauri/tauri.conf.json").exists() {
            return Ok(h.to_path_buf());
        }
        let web = h.join("web");
        if web.join("src-tauri/tauri.conf.json").exists() {
            return Ok(web);
        }
        bail!(
            "--dir {} is neither the web/ dir nor a repo root containing web/src-tauri",
            h.display()
        );
    }

    let mut dir = std::env::current_dir().context("cannot read current directory")?;
    loop {
        if dir.join("src-tauri/tauri.conf.json").exists() {
            return Ok(dir);
        }
        if dir.join("web/src-tauri/tauri.conf.json").exists() {
            return Ok(dir.join("web"));
        }
        if !dir.pop() {
            break;
        }
    }
    bail!(
        "could not find the XenoClaw desktop app — run this from a checkout, or pass \
         --dir <repo-or-web-dir>"
    )
}

/// Install the best artifact under `bundle` for the current user.
fn install_from_bundle(bundle: &Path, web_dir: &Path) -> Result<()> {
    #[cfg(target_os = "windows")]
    if let Some(exe) = find_with_ext(bundle, "exe") {
        println!("→ launching installer {}", exe.display());
        Command::new(&exe).status().context("failed to launch installer")?;
        return Ok(());
    }

    if let Some(appimage) = find_with_ext(bundle, "AppImage") {
        return install_appimage_local(&appimage, web_dir);
    }

    if let Some(deb) = find_with_ext(bundle, "deb") {
        if which("dpkg").is_some() {
            println!("→ installing {} via dpkg (needs sudo)", deb.display());
            crate::try_branch::run_privileged(&["dpkg", "-i", &deb.to_string_lossy()])?;
            return Ok(());
        }
    }

    bail!(
        "no installable artifact found under {}. Build it manually with \
         `cd {} && npm run tauri:build`.",
        bundle.display(),
        web_dir.display()
    )
}

/// Install an AppImage into the user's `~/.local` with a desktop launcher +
/// icon — no root needed, works on any distro (e.g. Arch).
fn install_appimage_local(appimage: &Path, web_dir: &Path) -> Result<()> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")?;
    let bin_dir = home.join(".local/bin");
    let apps_dir = home.join(".local/share/applications");
    let icon_dir = home.join(".local/share/icons/hicolor/256x256/apps");
    for d in [&bin_dir, &apps_dir, &icon_dir] {
        fs::create_dir_all(d).with_context(|| format!("failed to create {}", d.display()))?;
    }

    let dest = bin_dir.join("XenoClaw.AppImage");
    fs::copy(appimage, &dest)
        .with_context(|| format!("failed to copy AppImage to {}", dest.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dest, fs::Permissions::from_mode(0o755)).ok();
    }

    // Icon (256px) — best effort.
    let icon_src = web_dir.join("src-tauri/icons/128x128@2x.png");
    let icon_dest = icon_dir.join("xenoclaw-desktop.png");
    let _ = fs::copy(&icon_src, &icon_dest);

    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=XenoClaw\n\
         Comment=XenoClaw desktop client\n\
         Exec={}\n\
         Icon=xenoclaw-desktop\n\
         Terminal=false\n\
         Categories=Utility;Development;Network;\n",
        dest.display()
    );
    let entry_path = apps_dir.join("xenoclaw-desktop.desktop");
    fs::write(&entry_path, entry)
        .with_context(|| format!("failed to write {}", entry_path.display()))?;

    println!("\n✓ Installed XenoClaw desktop app:");
    println!("    binary:  {}", dest.display());
    println!("    launcher:{}", entry_path.display());
    println!("Make sure {} is on your PATH, or launch it from your app menu.", bin_dir.display());
    Ok(())
}

/// First file under `dir` (recursively) whose extension matches `ext`.
fn find_with_ext(dir: &Path, ext: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_with_ext(&path, ext) {
                return Some(found);
            }
        } else if path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.eq_ignore_ascii_case(ext))
            .unwrap_or(false)
        {
            return Some(path);
        }
    }
    None
}

/// Run an npm script in `web_dir`, inheriting stdio.
fn npm(web_dir: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new("npm")
        .current_dir(web_dir)
        .args(args)
        .status()
        .context("failed to run npm")?;
    if !status.success() {
        bail!("`npm {}` failed", args.join(" "));
    }
    Ok(())
}

/// Locate an executable on PATH (also tries `.cmd`/`.exe` on Windows).
fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: &[&str] = if cfg!(windows) { &["", ".cmd", ".exe"] } else { &[""] };
    for dir in std::env::split_paths(&path) {
        for ext in exts {
            let candidate = dir.join(format!("{bin}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Warn when running as root — npm/cargo would create root-owned files in the
/// user's checkout. Only the final `.deb` install needs elevation, and the
/// command self-elevates for just that step.
fn warn_if_root() {
    #[cfg(unix)]
    if unsafe { libc::geteuid() } == 0 {
        println!(
            "⚠ running as root — npm/cargo will create root-owned files in your checkout. \
             If this isn't a root-only machine, run this command as your normal user instead \
             (it elevates by itself only for the final .deb install)."
        );
    }
}

/// Bail early with actionable guidance if `node_modules` exists but is owned by
/// another user (typically a leftover from a previous `sudo` run) — otherwise
/// npm fails deep in its output with a cryptic EACCES.
fn ensure_node_modules_writable(web_dir: &Path) -> Result<()> {
    let node_modules = web_dir.join("node_modules");
    if !node_modules.exists() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let euid = unsafe { libc::geteuid() };
        if euid != 0 {
            if let Ok(md) = fs::metadata(&node_modules) {
                if md.uid() != euid {
                    bail!(
                        "{} is owned by uid {} but you are uid {} — npm can't modify it \
                         (usually a leftover from a previous `sudo` run).\n\
                         Fix it, then re-run this command WITHOUT sudo:\n  \
                         sudo rm -rf {:?}",
                        node_modules.display(),
                        md.uid(),
                        euid,
                        node_modules
                    );
                }
            }
        }
    }
    Ok(())
}
