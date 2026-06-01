//! `xenoclaw try <ref>` — quickly build and run a branch or PR for testing.
//!
//! Flow:
//!   1. Resolve `<ref>` — an all-numeric arg is treated as a PR number
//!      (fetched via `pull/N/head`), anything else as a branch name.
//!   2. Check the resolved commit out into a dedicated git worktree under
//!      `~/.xenoclaw/try-worktrees/<label>/` so the current checkout (and any
//!      in-progress work) is never touched. Worktrees are reused across runs
//!      so cargo's incremental cache survives.
//!   3. `cargo build` the binary inside that worktree.
//!   4. Run it. Two modes:
//!      - **service (default):** install the fresh binary over the path the
//!        systemd unit runs (`/opt/xenoclaw/bin/xenoclaw`) and restart
//!        `xenoclaw-agent`. The new version persists and logs to journald.
//!        Privileged steps run via `sudo` when not already root.
//!      - **`--exec`:** stop the service if it's running, free the API port,
//!        and exec the fresh binary in the foreground (logs in your terminal,
//!        Ctrl-C stops it, the installed service binary is left untouched).
//!
//! Linux-only — the port → PID lookup reads /proc directly, matching the rest
//! of the binary (see the metrics sampler in main.rs).

use std::fs;
use std::net::TcpStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use common::config::load_config;

/// systemd unit installed by install.sh / deploy/xenoclaw-agent.service.
const SERVICE_NAME: &str = "xenoclaw-agent";
/// Path the unit's ExecStart runs — where install.sh drops the release binary.
const INSTALLED_BIN: &str = "/opt/xenoclaw/bin/xenoclaw";
/// Where service mode stashes the binary it's about to overwrite, so
/// `xenoclaw try --restore` can swap the real deployment back in.
const BACKUP_BIN: &str = "/opt/xenoclaw/bin/xenoclaw.bak";

/// Entry point for `xenoclaw try`.
pub fn run_try(
    config_path: &Path,
    reference: Option<&str>,
    release: bool,
    no_run: bool,
    exec: bool,
    restore: bool,
) -> Result<()> {
    // `--restore` is a standalone action — no ref, no build.
    if restore {
        if reference.is_some() {
            println!("(ignoring ref argument — --restore just swaps the backup binary back)");
        }
        return restore_service(config_path);
    }

    let reference = reference.map(str::trim).unwrap_or("");
    if reference.is_empty() {
        bail!("no branch name or PR number given (usage: xenoclaw try <branch|PR#>)");
    }

    let repo_root = repo_root().context(
        "not inside a XenoClaw git checkout — run this from the repository (or a worktree of it)",
    )?;

    // Resolve the ref into a concrete commit + a filesystem-safe label.
    let resolved = resolve_ref(&repo_root, reference)?;
    println!(
        "→ {} resolves to commit {}",
        resolved.label,
        &resolved.commit[..resolved.commit.len().min(12)]
    );

    let worktree = ensure_worktree(&repo_root, &resolved)?;
    println!("→ worktree ready at {}", worktree.display());

    build(&worktree, release)?;

    let bin = worktree
        .join("target")
        .join(if release { "release" } else { "debug" })
        .join("xenoclaw");
    if !bin.exists() {
        bail!("build reported success but binary is missing at {}", bin.display());
    }

    if no_run {
        println!("\n✓ Built (skipping run). Binary:\n  {}", bin.display());
        println!(
            "Install + restart the service with:\n  xenoclaw try {reference}\n\
             Or run it in the foreground with:\n  {} serve --config {}",
            bin.display(),
            config_path.display()
        );
        return Ok(());
    }

    if exec {
        run_foreground(config_path, &bin, &resolved.label)
    } else {
        run_as_service(config_path, &bin, &resolved.label)
    }
}

/// Default mode: install the fresh binary over the service's binary path and
/// restart the unit. Privileged steps go through `sudo` when not root.
fn run_as_service(config_path: &Path, bin: &Path, label: &str) -> Result<()> {
    println!("\n▶ Deploying {label} to the {SERVICE_NAME} service");

    // Stop first so the running process isn't mapped to the inode we're about
    // to replace, and so Restart=on-failure can't race the binary swap.
    if service_is_active() {
        println!("→ stopping {SERVICE_NAME}");
        run_privileged(&["systemctl", "stop", SERVICE_NAME])
            .context("failed to stop the service")?;
    }

    // Back up the current binary the first time we overwrite it, so a later
    // `--restore` returns to the real deployment. Only create the backup when
    // one doesn't already exist — otherwise a second `try` would clobber the
    // genuine binary with the previous try build.
    if Path::new(INSTALLED_BIN).exists() && !Path::new(BACKUP_BIN).exists() {
        println!("→ backing up current binary to {BACKUP_BIN}");
        run_privileged(&["cp", "-p", INSTALLED_BIN, BACKUP_BIN])
            .context("failed to back up the current binary")?;
    }

    println!("→ installing binary to {INSTALLED_BIN}");
    run_privileged(&[
        "install",
        "-m",
        "755",
        &bin.to_string_lossy(),
        INSTALLED_BIN,
    ])
    .with_context(|| format!("failed to install binary to {INSTALLED_BIN}"))?;

    println!("→ starting {SERVICE_NAME}");
    run_privileged(&["systemctl", "restart", SERVICE_NAME])
        .context("failed to restart the service")?;

    // Best-effort readiness probe on the configured port.
    let port = load_config(config_path).map(|c| c.api.port).unwrap_or(0);
    if port != 0 && wait_for_port(port, Duration::from_secs(10)) {
        println!("\n✓ {SERVICE_NAME} is up on port {port} running {label}");
    } else if port != 0 {
        println!(
            "\n⚠ {SERVICE_NAME} restarted but port {port} isn't accepting connections yet."
        );
    } else {
        println!("\n✓ {SERVICE_NAME} restarted running {label}");
    }
    println!("Follow logs with:    journalctl -u {SERVICE_NAME} -f");
    if Path::new(BACKUP_BIN).exists() {
        println!("Restore previous:    xenoclaw try --restore");
    }
    Ok(())
}

/// `--restore` mode: swap the backed-up binary back in and restart the service.
fn restore_service(config_path: &Path) -> Result<()> {
    if !Path::new(BACKUP_BIN).exists() {
        bail!(
            "no backup found at {BACKUP_BIN} — nothing to restore \
             (service mode creates it the first time it overwrites the binary)"
        );
    }
    println!("\n▶ Restoring the previous {SERVICE_NAME} binary");

    if service_is_active() {
        println!("→ stopping {SERVICE_NAME}");
        run_privileged(&["systemctl", "stop", SERVICE_NAME])
            .context("failed to stop the service")?;
    }

    println!("→ restoring {INSTALLED_BIN} from {BACKUP_BIN}");
    run_privileged(&["install", "-m", "755", BACKUP_BIN, INSTALLED_BIN])
        .context("failed to restore the binary")?;
    // Consume the backup so the next `try` captures a fresh baseline.
    run_privileged(&["rm", "-f", BACKUP_BIN]).context("failed to remove the backup")?;

    println!("→ starting {SERVICE_NAME}");
    run_privileged(&["systemctl", "restart", SERVICE_NAME])
        .context("failed to restart the service")?;

    let port = load_config(config_path).map(|c| c.api.port).unwrap_or(0);
    if port != 0 && wait_for_port(port, Duration::from_secs(10)) {
        println!("\n✓ {SERVICE_NAME} restored and up on port {port}");
    } else {
        println!("\n✓ {SERVICE_NAME} restored and restarted");
    }
    println!("Follow logs with:  journalctl -u {SERVICE_NAME} -f");
    Ok(())
}

/// `--exec` mode: run the fresh binary in the foreground on the configured port.
fn run_foreground(config_path: &Path, bin: &Path, label: &str) -> Result<()> {
    let config = load_config(config_path)
        .with_context(|| format!("failed to load config at {}", config_path.display()))?;
    let port = config.api.port;

    // If the systemd service owns the port, stop it first — otherwise
    // Restart=on-failure would race us for the port after we kill the process.
    if service_is_active() {
        println!("→ stopping {SERVICE_NAME} (so it doesn't auto-restart and reclaim the port)");
        run_privileged(&["systemctl", "stop", SERVICE_NAME])
            .context("failed to stop the service")?;
    }

    // Free the port from any remaining (non-systemd) instance.
    stop_listener_on_port(port)?;

    println!(
        "\n▶ Starting {label} on port {port} (config: {})\n",
        config_path.display()
    );

    // exec replaces this process with the server, so Ctrl-C reaches it directly.
    let err = Command::new(bin)
        .arg("serve")
        .arg("--config")
        .arg(config_path)
        .exec();
    Err(anyhow::Error::new(err).context(format!("failed to exec {}", bin.display())))
}

/// True if the systemd unit is currently active.
fn service_is_active() -> bool {
    Command::new("systemctl")
        .args(["is-active", "--quiet", SERVICE_NAME])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run a command, prefixing `sudo` when the current user isn't root. Stdio is
/// inherited so a sudo password prompt works.
fn run_privileged(args: &[&str]) -> Result<()> {
    let is_root = unsafe { libc::geteuid() } == 0;
    let (program, rest): (&str, &[&str]) = if is_root {
        (args[0], &args[1..])
    } else {
        ("sudo", args)
    };
    let status = Command::new(program)
        .args(rest)
        .status()
        .with_context(|| format!("failed to run {program}"))?;
    if !status.success() {
        bail!("`{} {}` failed", program, rest.join(" "));
    }
    Ok(())
}

/// Poll until something accepts a TCP connection on `port` (loopback) or the
/// deadline passes. Returns whether the port became reachable.
fn wait_for_port(port: u16, timeout: Duration) -> bool {
    let addr = format!("127.0.0.1:{port}");
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(
            &addr.parse().expect("valid loopback addr"),
            Duration::from_millis(500),
        )
        .is_ok()
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    false
}

/// A ref resolved to a concrete commit plus a label safe to use in a path.
struct Resolved {
    commit: String,
    label: String,
}

/// Return the top level of the git repo containing the current directory.
fn repo_root() -> Result<PathBuf> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("failed to run git")?;
    if !out.status.success() {
        bail!("git rev-parse failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(PathBuf::from(path))
}

/// Fetch the requested ref and pin it to a commit SHA.
///
/// An all-digit arg is a PR number → `pull/N/head`. Otherwise it's a branch:
/// we fetch from origin and prefer the just-fetched `FETCH_HEAD`, falling back
/// to a local branch of the same name when origin doesn't have it.
fn resolve_ref(repo_root: &Path, reference: &str) -> Result<Resolved> {
    let is_pr = !reference.is_empty() && reference.chars().all(|c| c.is_ascii_digit());

    if is_pr {
        let refspec = format!("pull/{reference}/head");
        let status = git(repo_root, &["fetch", "origin", &refspec])
            .with_context(|| format!("failed to fetch PR #{reference} ({refspec})"))?;
        if !status {
            bail!(
                "could not fetch PR #{reference}. Check the number, or that origin is the GitHub remote."
            );
        }
        let commit = rev_parse(repo_root, "FETCH_HEAD")?;
        Ok(Resolved {
            commit,
            label: format!("pr-{reference}"),
        })
    } else {
        // Try origin first; ignore failure so local-only branches still work.
        let fetched = git(repo_root, &["fetch", "origin", reference]).unwrap_or(false);
        let commit = if fetched {
            rev_parse(repo_root, "FETCH_HEAD")?
        } else {
            rev_parse(repo_root, reference).with_context(|| {
                format!("branch '{reference}' not found on origin or locally")
            })?
        };
        Ok(Resolved {
            commit,
            label: sanitize_label(reference),
        })
    }
}

/// Create (or update) the dedicated worktree for this ref and return its path.
fn ensure_worktree(repo_root: &Path, resolved: &Resolved) -> Result<PathBuf> {
    let base = crate::xenoclaw_home().join("try-worktrees");
    fs::create_dir_all(&base)
        .with_context(|| format!("failed to create {}", base.display()))?;
    let path = base.join(&resolved.label);

    if path.join(".git").exists() {
        // Reuse the existing worktree — just move it to the new commit.
        if !git(&path, &["checkout", "--detach", &resolved.commit])? {
            bail!("failed to checkout {} in existing worktree", resolved.commit);
        }
    } else {
        // Stale leftover dir without a git link — clear it so `worktree add` succeeds.
        if path.exists() {
            fs::remove_dir_all(&path)
                .with_context(|| format!("failed to clear stale dir {}", path.display()))?;
        }
        let path_str = path.to_string_lossy().to_string();
        if !git(
            repo_root,
            &["worktree", "add", "--detach", &path_str, &resolved.commit],
        )? {
            bail!("git worktree add failed for {}", path.display());
        }
    }
    Ok(path)
}

/// `cargo build [-p xenoclaw] [--release]` inside the worktree, inheriting stdio
/// so the user watches the build live.
fn build(worktree: &Path, release: bool) -> Result<()> {
    println!("→ building (this can take a while on first run)…\n");
    let mut cmd = Command::new("cargo");
    cmd.current_dir(worktree).args(["build", "-p", "xenoclaw"]);
    if release {
        cmd.arg("--release");
    }
    let status = cmd.status().context("failed to run cargo")?;
    if !status.success() {
        bail!("cargo build failed");
    }
    Ok(())
}

/// Find and stop the process listening on `port`. No-op if nothing is bound.
fn stop_listener_on_port(port: u16) -> Result<()> {
    let Some(pid) = listener_pid(port) else {
        return Ok(());
    };
    println!("→ stopping running instance on port {port} (pid {pid})");

    // SIGTERM, then poll until the port frees, escalating to SIGKILL.
    unsafe { libc::kill(pid, libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if listener_pid(port).is_none() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    println!("→ instance didn't exit in time — sending SIGKILL");
    unsafe { libc::kill(pid, libc::SIGKILL) };
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if listener_pid(port).is_none() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    bail!("port {port} still in use after SIGKILL — refusing to start a second instance");
}

/// Return the PID of the process LISTENing on `port`, scanning /proc.
fn listener_pid(port: u16) -> Option<i32> {
    let inode = listening_inode("/proc/net/tcp", port)
        .or_else(|| listening_inode("/proc/net/tcp6", port))?;
    pid_for_socket_inode(inode)
}

/// Parse a /proc/net/tcp{,6} table for the socket inode in LISTEN (state 0A)
/// on the given local port.
fn listening_inode(path: &str, port: u16) -> Option<u64> {
    let contents = fs::read_to_string(path).ok()?;
    for line in contents.lines().skip(1) {
        let mut f = line.split_whitespace();
        let local = f.next()?; // sl
        let _ = local;
        let local_addr = f.next()?; // "IP:PORT" in hex
        let _rem = f.next()?;
        let state = f.next()?; // "0A" == LISTEN
        if state != "0A" {
            continue;
        }
        let port_hex = local_addr.rsplit(':').next()?;
        let local_port = u16::from_str_radix(port_hex, 16).ok()?;
        if local_port != port {
            continue;
        }
        // Skip tx:rx, tr:when, retrnsmt, uid, timeout → inode is the 10th field.
        let inode = f.nth(5)?;
        return inode.parse().ok();
    }
    None
}

/// Scan /proc/<pid>/fd for a symlink to `socket:[inode]` and return that PID.
fn pid_for_socket_inode(inode: u64) -> Option<i32> {
    let target = format!("socket:[{inode}]");
    for entry in fs::read_dir("/proc").ok()? {
        let entry = entry.ok()?;
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|s| s.parse::<i32>().ok()) else {
            continue;
        };
        let fd_dir = entry.path().join("fd");
        let Ok(fds) = fs::read_dir(&fd_dir) else {
            continue; // process gone or not ours
        };
        for fd in fds.flatten() {
            if let Ok(link) = fs::read_link(fd.path()) {
                if link.to_string_lossy() == target {
                    return Some(pid);
                }
            }
        }
    }
    None
}

/// `git -C <dir> <args>` returning whether it succeeded; stdio inherited.
fn git(dir: &Path, args: &[&str]) -> Result<bool> {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .context("failed to run git")?;
    Ok(status.success())
}

/// `git -C <dir> rev-parse --verify <rev>^{commit}` → trimmed SHA.
fn rev_parse(dir: &Path, rev: &str) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--verify", &format!("{rev}^{{commit}}")])
        .output()
        .context("failed to run git rev-parse")?;
    if !out.status.success() {
        bail!(
            "git rev-parse {rev} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Turn a branch name into a path-safe label (`feat/x` → `feat-x`).
fn sanitize_label(reference: &str) -> String {
    reference
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
            c
        } else {
            '-'
        })
        .collect()
}
