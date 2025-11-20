use std::collections::HashMap;
#[cfg(test)]
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

const CACHE_FILE_NAME: &str = "lightcode-resume-cache.json";
pub(crate) const TTY_OVERRIDE_ENV: &str = "LIGHTCODE_FORCE_TTY_ID";

#[derive(Debug, Default, Serialize, Deserialize)]
struct ResumeCacheFile {
    #[serde(default)]
    entries: HashMap<String, ResumeCacheEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResumeCacheEntry {
    session_id: String,
    updated_at: u64,
}

pub fn record_session_for_current_tty(session_id: &str) {
    if session_id.is_empty() {
        return;
    }

    if let Err(err) = try_record_session_for_current_tty(session_id) {
        tracing::debug!(?err, "failed to update resume cache");
    }
}

fn try_record_session_for_current_tty(session_id: &str) -> Result<()> {
    let Some(tty_id) = current_tty_identifier() else {
        return Ok(());
    };

    let mut cache = match load_cache() {
        Ok(cache) => cache,
        Err(err) => {
            tracing::debug!(?err, "failed to read existing resume cache; recreating");
            ResumeCacheFile::default()
        }
    };
    cache.entries.insert(
        tty_id,
        ResumeCacheEntry {
            session_id: session_id.to_string(),
            updated_at: current_timestamp(),
        },
    );
    persist_cache(&cache)
}

pub fn lookup_cached_session_for_current_tty() -> Result<Option<String>> {
    let Some(tty_id) = current_tty_identifier() else {
        return Ok(None);
    };

    let cache = load_cache()?;
    Ok(cache.entries.get(&tty_id).map(|entry| entry.session_id.clone()))
}

fn load_cache() -> Result<ResumeCacheFile> {
    let path = cache_file_path()?;
    match fs::read(&path) {
        Ok(bytes) => {
            if bytes.is_empty() {
                Ok(ResumeCacheFile::default())
            } else {
                serde_json::from_slice::<ResumeCacheFile>(&bytes)
                    .with_context(|| format!("failed to parse resume cache at {}", path.display()))
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(ResumeCacheFile::default()),
        Err(err) => Err(err)
            .with_context(|| format!("failed to read resume cache at {}", path.display())),
    }
}

fn persist_cache(cache: &ResumeCacheFile) -> Result<()> {
    let path = cache_file_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let tmp_dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut temp = NamedTempFile::new_in(tmp_dir)
        .with_context(|| format!("failed to create temp file next to {}", path.display()))?;
    serde_json::to_writer_pretty(&mut temp, cache)?;
    temp.persist(&path)
        .map_err(|err| err.error)
        .with_context(|| format!("failed to persist resume cache to {}", path.display()))?;
    Ok(())
}

fn cache_file_path() -> Result<PathBuf> {
    let mut path = code_core::config::find_code_home()
        .context("failed to locate code home for resume cache")?;
    path.push(CACHE_FILE_NAME);
    Ok(path)
}

fn current_tty_identifier() -> Option<String> {
    if let Ok(value) = std::env::var(TTY_OVERRIDE_ENV) {
        if !value.is_empty() {
            return Some(value);
        }
    }

    current_tty_identifier_os()
}

#[cfg(unix)]
fn current_tty_identifier_os() -> Option<String> {
    use std::ffi::CStr;

    const FD_CANDIDATES: [i32; 3] = [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO];

    for fd in FD_CANDIDATES {
        if unsafe { libc::isatty(fd) } != 1 {
            continue;
        }

        let mut buf_len = 128;
        while buf_len <= 4096 {
            let mut buf = vec![0u8; buf_len];
            let result = unsafe { libc::ttyname_r(fd, buf.as_mut_ptr() as *mut libc::c_char, buf_len) };
            if result == 0 {
                let cstr = unsafe { CStr::from_ptr(buf.as_ptr() as *const libc::c_char) };
                if let Ok(raw) = cstr.to_str() {
                    let normalized = normalize_tty_path(raw);
                    if !normalized.is_empty() {
                        return Some(normalized);
                    }
                }
                break;
            } else if result == libc::ERANGE {
                buf_len *= 2;
                continue;
            } else {
                break;
            }
        }
    }

    None
}

#[cfg(not(unix))]
fn current_tty_identifier_os() -> Option<String> {
    None
}

#[cfg(unix)]
fn normalize_tty_path(raw: &str) -> String {
    let path = Path::new(raw);
    std::fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| raw.to_string())
}

#[cfg(not(unix))]
fn normalize_tty_path(raw: &str) -> String {
    raw.to_string()
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn record_and_lookup_round_trip() {
        let dir = TempDir::new().unwrap();
        let _code_home = EnvGuard::set_path("CODE_HOME", dir.path());
        let _codex_home = EnvGuard::unset("CODEX_HOME");
        let _tty_guard = EnvGuard::set_str(TTY_OVERRIDE_ENV, "tty://test");

        record_session_for_current_tty("session-123");
        let cached = lookup_cached_session_for_current_tty().unwrap();
        assert_eq!(cached.as_deref(), Some("session-123"));
    }

    struct EnvGuard {
        key: &'static str,
        original: Option<OsString>,
    }

    impl EnvGuard {
        fn set_str(key: &'static str, value: &str) -> Self {
            let guard = Self::new(key);
            std::env::set_var(key, value);
            guard
        }

        fn set_path(key: &'static str, value: &Path) -> Self {
            let guard = Self::new(key);
            std::env::set_var(key, value);
            guard
        }

        fn unset(key: &'static str) -> Self {
            let guard = Self::new(key);
            std::env::remove_var(key);
            guard
        }

        fn new(key: &'static str) -> Self {
            Self {
                key,
                original: std::env::var_os(key),
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(value) = self.original.take() {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}
