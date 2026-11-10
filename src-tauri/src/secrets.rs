//! Refresh-token storage: Windows Credential Manager / Secret Service via `keyring`, with a
//! user-only file fallback for systems without a secret service (common for AppImages).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

const SERVICE: &str = "io.github.schme16.sync";

fn fallback_file(dir: &Path) -> PathBuf {
    dir.join("secrets.json")
}

fn read_fallback(dir: &Path) -> HashMap<String, String> {
    std::fs::read(fallback_file(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write_fallback(dir: &Path, map: &HashMap<String, String>) -> std::io::Result<()> {
    let f = fallback_file(dir);
    std::fs::write(&f, serde_json::to_vec(map).unwrap_or_default())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub fn store(dir: &Path, account: &str, token: &str) -> anyhow::Result<()> {
    match keyring::Entry::new(SERVICE, account).and_then(|e| e.set_password(token)) {
        Ok(()) => {
            // make sure a stale fallback copy doesn't linger
            let mut m = read_fallback(dir);
            if m.remove(account).is_some() {
                let _ = write_fallback(dir, &m);
            }
            Ok(())
        }
        Err(e) => {
            tracing::warn!("system keyring unavailable ({e}); storing token in a private file");
            let mut m = read_fallback(dir);
            m.insert(account.to_string(), token.to_string());
            write_fallback(dir, &m)?;
            Ok(())
        }
    }
}

pub fn load(dir: &Path, account: &str) -> Option<String> {
    if let Ok(p) = keyring::Entry::new(SERVICE, account).and_then(|e| e.get_password()) {
        return Some(p);
    }
    read_fallback(dir).remove(account)
}

pub fn delete(dir: &Path, account: &str) {
    if let Ok(e) = keyring::Entry::new(SERVICE, account) {
        let _ = e.delete_credential();
    }
    let mut m = read_fallback(dir);
    if m.remove(account).is_some() {
        let _ = write_fallback(dir, &m);
    }
}
