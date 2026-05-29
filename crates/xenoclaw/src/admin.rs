//! Admin credential repair + diagnostic CLI helpers.
//!
//! These exist because the TUI wizard can leave the on-disk credentials
//! in a state where login silently fails (wrong hash, wrong username,
//! empty fields) — and without server access there's no way to tell from
//! the API whether the stored hash matches what the user typed.
//!
//! Each helper operates on the same config file the running service
//! reads, so a restart picks the change up immediately.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use anyhow::{bail, Context, Result};

use common::config::load_config;
use security_layer::auth::{ApiKeyAuthenticator, PasswordAuthenticator};

/// Show what the service thinks the admin identity is. Pure read-only —
/// useful before guessing why login is rejected.
pub fn show_admin(config_path: &Path) -> Result<()> {
    let config = load_config(config_path)
        .with_context(|| format!("failed to load config at {}", config_path.display()))?;

    println!("Config file:           {}", config_path.display());
    println!(
        "admin_username:        {:?}",
        config.security.admin_username
    );

    let pw_hash = &config.security.admin_password_hash;
    if pw_hash.is_empty() {
        println!("admin_password_hash:   <empty — password login disabled>");
    } else {
        println!(
            "admin_password_hash:   {} chars, prefix={:?}",
            pw_hash.len(),
            &pw_hash[..pw_hash.len().min(7)]
        );
        if !(pw_hash.starts_with("$2a$")
            || pw_hash.starts_with("$2b$")
            || pw_hash.starts_with("$2y$"))
        {
            println!("  WARNING: hash does not look like bcrypt ($2a$/$2b$/$2y$).");
        }
    }

    let key_hash = &config.security.admin_key_hash;
    if key_hash.is_empty() {
        println!("admin_key_hash:        <empty — API key login disabled>");
    } else {
        println!(
            "admin_key_hash:        {} chars, prefix={:?}",
            key_hash.len(),
            &key_hash[..key_hash.len().min(12)]
        );
        if key_hash.len() != 64 || !key_hash.chars().all(|c| c.is_ascii_hexdigit()) {
            println!("  WARNING: hash is not a 64-char hex SHA-256 digest.");
        }
    }

    Ok(())
}

/// Verify a username/password combination against the on-disk config
/// using the exact same bcrypt path the API server uses. Returns Ok with
/// a printed result; the outer process exit code is non-zero on mismatch.
pub fn verify_login(config_path: &Path, username: &str, password: &str) -> Result<()> {
    let config = load_config(config_path)
        .with_context(|| format!("failed to load config at {}", config_path.display()))?;

    println!("Config:    {}", config_path.display());
    println!("Username:  {:?}", username);
    println!(
        "Password:  ({} chars, {} bytes)",
        password.chars().count(),
        password.len()
    );

    if username != config.security.admin_username {
        println!(
            "\nResult:    FAIL — username mismatch.\n           Config admin_username is {:?}; you provided {:?}.",
            config.security.admin_username, username
        );
        std::process::exit(1);
    }

    let hash = &config.security.admin_password_hash;
    if hash.is_empty() {
        println!("\nResult:    FAIL — admin_password_hash is empty. Password login is disabled.");
        println!("           Run `xenoclaw passwd` to set one.");
        std::process::exit(1);
    }

    let auth = PasswordAuthenticator::new();
    match auth.verify_password(password, hash) {
        Ok(()) => {
            println!("\nResult:    OK — bcrypt verifies. Login would succeed.");
            Ok(())
        }
        Err(_) => {
            println!(
                "\nResult:    FAIL — bcrypt verify rejected this password against the stored hash.\n           The hash in config does not match the password you typed."
            );
            println!("           Run `xenoclaw passwd` to set a known-good password.");
            std::process::exit(1);
        }
    }
}

/// Set the admin password hash in the config. Reads the password from
/// `password` if `Some`, otherwise prompts twice on the TTY. The username
/// is updated when `username` is `Some`. The file is rewritten in place
/// (line-substitution to preserve formatting + comments).
pub fn set_password(
    config_path: &Path,
    username: Option<&str>,
    password: Option<&str>,
) -> Result<()> {
    if !config_path.exists() {
        bail!(
            "config file does not exist: {}. Run `xenoclaw -s` first.",
            config_path.display()
        );
    }

    let pw = match password {
        Some(p) => p.to_string(),
        None => prompt_password()?,
    };
    if pw.is_empty() {
        bail!("password must not be empty");
    }

    let hash = bcrypt::hash(&pw, bcrypt::DEFAULT_COST).context("bcrypt::hash failed")?;

    // Sanity check: the hash we just generated MUST verify against the
    // password we just typed, before we touch the config file. If this
    // ever fails it's a bug we want to know about immediately.
    if !bcrypt::verify(&pw, &hash).unwrap_or(false) {
        bail!("internal error: freshly-generated bcrypt hash did not self-verify");
    }

    rewrite_security_field(config_path, "admin_password_hash", &hash)?;
    if let Some(u) = username {
        rewrite_security_field(config_path, "admin_username", u)?;
    }

    println!("Updated {}.", config_path.display());
    if let Some(u) = username {
        println!("  admin_username      = {:?}", u);
    }
    println!("  admin_password_hash = (bcrypt, {} chars)", hash.len());
    println!("\nRestart the service to pick up the change:");
    println!("  sudo systemctl restart xenoclaw-agent");
    println!("\nThen verify with:");
    let user_for_verify = username
        .map(String::from)
        .unwrap_or_else(|| "admin".to_string());
    println!("  xenoclaw verify-login --user {user_for_verify} --password '...'");
    Ok(())
}

/// Generate a fresh admin API key, hash it, write the hash to the config,
/// and print the raw key (one-time display).
pub fn set_api_key(config_path: &Path) -> Result<()> {
    if !config_path.exists() {
        bail!(
            "config file does not exist: {}. Run `xenoclaw -s` first.",
            config_path.display()
        );
    }
    let auth = ApiKeyAuthenticator::new();
    let raw = format!("xc_{}", auth.generate_key(40));
    let hash = auth.hash_key(&raw);

    rewrite_security_field(config_path, "admin_key_hash", &hash)?;

    println!("New admin API key (save this — it will NOT be shown again):\n");
    println!("  {raw}\n");
    println!("Hash written to {}.", config_path.display());
    println!("\nRestart the service:");
    println!("  sudo systemctl restart xenoclaw-agent");
    Ok(())
}

// ─── Internals ───────────────────────────────────────────────────────────────

fn prompt_password() -> Result<String> {
    print!("New password: ");
    io::stdout().flush().ok();
    let p1 = rpassword::read_password().context("failed to read password")?;
    print!("Confirm:      ");
    io::stdout().flush().ok();
    let p2 = rpassword::read_password().context("failed to read password")?;
    if p1 != p2 {
        bail!("passwords did not match");
    }
    Ok(p1)
}

/// Substitute (or append) a single key inside the `[security]` table.
///
/// We avoid a full TOML round-trip because it would discard the wizard's
/// comments and reorder keys; a line-level substitution is enough for the
/// scalar string fields we touch (`admin_username`, `admin_password_hash`,
/// `admin_key_hash`).
fn rewrite_security_field(config_path: &Path, key: &str, value: &str) -> Result<()> {
    let original = fs::read_to_string(config_path)
        .with_context(|| format!("read {}", config_path.display()))?;

    let mut in_security = false;
    let mut wrote = false;
    let mut out = String::with_capacity(original.len() + value.len() + 32);

    for line in original.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            // Entering a new table; if we were in [security] and never
            // saw the key, append it before leaving.
            if in_security && !wrote {
                out.push_str(&format!("{key} = {}\n", toml_escape(value)));
                wrote = true;
            }
            in_security = trimmed == "[security]";
            out.push_str(line);
            out.push('\n');
            continue;
        }

        if in_security && !wrote {
            // Match `key = ...` ignoring leading whitespace.
            if let Some(rest) = trimmed.strip_prefix(key) {
                let after = rest.trim_start();
                if after.starts_with('=') {
                    out.push_str(&format!("{key} = {}\n", toml_escape(value)));
                    wrote = true;
                    continue;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }

    // EOF reached while still in [security] and key not yet written.
    if in_security && !wrote {
        out.push_str(&format!("{key} = {}\n", toml_escape(value)));
        wrote = true;
    }

    if !wrote {
        bail!(
            "could not find or create [security] section in {}",
            config_path.display()
        );
    }

    // Atomic write: temp file in the same dir, then rename.
    let tmp = config_path.with_extension("toml.tmp");
    let perms = fs::metadata(config_path)
        .with_context(|| format!("stat {}", config_path.display()))?
        .permissions();
    fs::write(&tmp, &out).with_context(|| format!("write {}", tmp.display()))?;
    fs::set_permissions(&tmp, perms).with_context(|| format!("chmod {}", tmp.display()))?;
    fs::rename(&tmp, config_path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), config_path.display()))?;
    Ok(())
}

/// Format a string value as a TOML basic string. Conservative quoting —
/// only handles the characters bcrypt + hex hashes actually produce
/// (`$`, `/`, `.`, alphanumerics) plus a few defensive escapes.
fn toml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use tempfile::NamedTempFile;

    fn write_tmp(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    #[test]
    fn rewrites_existing_field_in_security_table() {
        let f = write_tmp(
            "[general]\nfoo = 1\n\n[security]\nadmin_username = \"old\"\nadmin_password_hash = \"\"\n\n[web]\nport = 8080\n",
        );
        rewrite_security_field(f.path(), "admin_password_hash", "$2b$12$abc").unwrap();
        let after = std::fs::read_to_string(f.path()).unwrap();
        assert!(after.contains("admin_password_hash = \"$2b$12$abc\""));
        assert!(after.contains("admin_username = \"old\""));
        assert!(after.contains("[web]"));
    }

    #[test]
    fn appends_field_when_missing() {
        let f = write_tmp("[security]\nadmin_username = \"u\"\n\n[web]\nport = 1\n");
        rewrite_security_field(f.path(), "admin_password_hash", "$2b$12$xyz").unwrap();
        let after = std::fs::read_to_string(f.path()).unwrap();
        assert!(after.contains("admin_password_hash = \"$2b$12$xyz\""));
    }

    #[test]
    fn appends_field_when_security_at_eof() {
        let f = write_tmp("[security]\nadmin_username = \"u\"\n");
        rewrite_security_field(f.path(), "admin_password_hash", "$2b$12$qqq").unwrap();
        let after = std::fs::read_to_string(f.path()).unwrap();
        assert!(after.contains("admin_password_hash = \"$2b$12$qqq\""));
    }

    #[test]
    fn escapes_special_chars() {
        assert_eq!(toml_escape(r#"a"b\c"#), r#""a\"b\\c""#);
    }

    #[test]
    fn bcrypt_hash_round_trips_through_rewrite() {
        // The whole reason this module exists: prove a hash we wrote can
        // still be loaded and verified by the same machinery the API uses.
        let pw = "Xeno3raJ1#$";
        let hash = bcrypt::hash(pw, bcrypt::DEFAULT_COST).unwrap();

        let f = write_tmp(
            "[security]\nadmin_username = \"admin\"\nadmin_password_hash = \"\"\nadmin_key_hash = \"\"\n",
        );
        rewrite_security_field(f.path(), "admin_password_hash", &hash).unwrap();

        let raw = std::fs::read_to_string(f.path()).unwrap();
        // Parse the TOML and pull the hash back out.
        let parsed: toml::Value = toml::from_str(&raw).unwrap();
        let stored = parsed["security"]["admin_password_hash"].as_str().unwrap();
        assert_eq!(stored, hash);
        assert!(bcrypt::verify(pw, stored).unwrap());
    }
}
