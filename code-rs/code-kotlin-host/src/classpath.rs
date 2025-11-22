use anyhow::{anyhow, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

const ENGINE_JAR_NAME: &str = "code-kotlin-engine.jar";

#[derive(Clone, Default)]
pub(crate) struct ResolverOverrides {
    pub env_override: Option<String>,
    pub current_exe_override: Option<PathBuf>,
    pub workspace_jar_override: Option<PathBuf>,
    pub cargo_target_jar_override: Option<PathBuf>,
}

pub(crate) fn resolve_classpath() -> Result<String> {
    let path = resolve_with_overrides(ResolverOverrides::default())?;
    Ok(path_to_string(&path))
}

fn resolve_with_overrides(overrides: ResolverOverrides) -> Result<PathBuf> {
    if let Some(path) = env_override_path(&overrides) {
        return Ok(path);
    }

    let exe_dir = overrides
        .current_exe_override
        .clone()
        .or_else(|| std::env::current_exe().ok())
        .and_then(|path| path.parent().map(|dir| dir.to_path_buf()));

    let mut probes = Vec::new();
    let mut seen = HashSet::new();

    if let Some(dir) = exe_dir {
        push_candidate(
            &mut probes,
            &mut seen,
            dir.join(ENGINE_JAR_NAME),
            "bundled next to the CLI binary",
        );
        if let Some(parent) = dir.parent() {
            push_candidate(
                &mut probes,
                &mut seen,
                parent.join("kotlin").join(ENGINE_JAR_NAME),
                "kotlin/ directory beside the active profile (dev-fast / preview)",
            );
        }
    }

    if let Some(path) = cargo_target_candidate(&overrides) {
        push_candidate(
            &mut probes,
            &mut seen,
            path,
            "CARGO_TARGET_DIR build output (kotlin/code-kotlin-engine.jar)",
        );
    }

    push_candidate(
        &mut probes,
        &mut seen,
        workspace_candidate(&overrides),
        "workspace target/kotlin/code-kotlin-engine.jar",
    );

    let mut attempted = Vec::new();
    for candidate in probes {
        debug!(path = %candidate.path.display(), reason = candidate.reason, "probing Kotlin engine jar");
        if jar_exists(&candidate.path) {
            info!(path = %candidate.path.display(), reason = candidate.reason, "resolved Kotlin engine jar");
            return Ok(candidate.path);
        }
        attempted.push(candidate);
    }

    let mut message = String::from(
        "Kotlin engine jar not found. Set CODE_KOTLIN_CLASSPATH or place code-kotlin-engine.jar in one of:\n",
    );
    for entry in attempted {
        let _ = std::fmt::Write::write_fmt(
            &mut message,
            format_args!("  - {} ({})\n", entry.path.display(), entry.reason),
        );
    }
    Err(anyhow!(message.trim_end().to_string()))
}

fn env_override_path(overrides: &ResolverOverrides) -> Option<PathBuf> {
    let env_value = overrides
        .env_override
        .clone()
        .or_else(|| std::env::var("CODE_KOTLIN_CLASSPATH").ok());

    let raw = env_value?;
    let candidate = PathBuf::from(raw.clone());
    if jar_exists(&candidate) {
        info!(path = %candidate.display(), "using CODE_KOTLIN_CLASSPATH override for Kotlin engine");
        Some(candidate)
    } else {
        warn!(path = %candidate.display(), "CODE_KOTLIN_CLASSPATH points at a missing jar; continuing with defaults");
        None
    }
}

fn cargo_target_candidate(overrides: &ResolverOverrides) -> Option<PathBuf> {
    if let Some(path) = &overrides.cargo_target_jar_override {
        return Some(path.clone());
    }
    option_env!("CARGO_TARGET_DIR").map(|dir| PathBuf::from(dir).join("kotlin").join(ENGINE_JAR_NAME))
}

fn workspace_candidate(overrides: &ResolverOverrides) -> PathBuf {
    if let Some(path) = &overrides.workspace_jar_override {
        return path.clone();
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("target")
        .join("kotlin")
        .join(ENGINE_JAR_NAME)
}

fn push_candidate(probes: &mut Vec<Candidate>, seen: &mut HashSet<PathBuf>, path: PathBuf, reason: &'static str) {
    if seen.insert(path.clone()) {
        probes.push(Candidate { path, reason });
    }
}

fn jar_exists(path: &Path) -> bool {
    path.is_file()
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[derive(Clone)]
struct Candidate {
    path: PathBuf,
    reason: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn uses_env_override_when_valid() {
        let dir = tempdir().unwrap();
        let jar = dir.path().join(ENGINE_JAR_NAME);
        std::fs::write(&jar, b"jar").unwrap();

        let overrides = ResolverOverrides {
            env_override: Some(jar.to_string_lossy().into_owned()),
            ..Default::default()
        };

        let resolved = resolve_with_overrides(overrides).unwrap();
        assert_eq!(resolved, jar);
    }

    #[test]
    fn falls_back_to_bundled_neighbor() {
        let bin_dir = tempdir().unwrap();
        let jar = bin_dir.path().join(ENGINE_JAR_NAME);
        std::fs::write(&jar, b"jar").unwrap();
        let fake_exe = bin_dir.path().join("code");
        std::fs::write(&fake_exe, b"binary").unwrap();

        let overrides = ResolverOverrides {
            current_exe_override: Some(fake_exe),
            ..Default::default()
        };

        let resolved = resolve_with_overrides(overrides).unwrap();
        assert_eq!(resolved, jar);
    }

    #[test]
    fn falls_back_to_parent_kotlin_directory() {
        let root = tempdir().unwrap();
        let profile_dir = root.path().join("target/dev-fast");
        std::fs::create_dir_all(&profile_dir).unwrap();
        let fake_exe = profile_dir.join("code");
        std::fs::write(&fake_exe, b"binary").unwrap();
        let kotlin_dir = root.path().join("target/kotlin");
        std::fs::create_dir_all(&kotlin_dir).unwrap();
        let jar = kotlin_dir.join(ENGINE_JAR_NAME);
        std::fs::write(&jar, b"jar").unwrap();

        let overrides = ResolverOverrides {
            current_exe_override: Some(fake_exe),
            workspace_jar_override: Some(jar.clone()),
            ..Default::default()
        };

        let resolved = resolve_with_overrides(overrides).unwrap();
        assert_eq!(resolved, jar);
    }

    #[test]
    fn surfaces_error_with_attempted_paths() {
        let root = tempdir().unwrap();
        let fake_bin = root.path().join("bin/code");
        std::fs::create_dir_all(fake_bin.parent().unwrap()).unwrap();
        std::fs::write(&fake_bin, b"binary").unwrap();
        let workspace_fallback = root.path().join("fallback/code-kotlin-engine.jar");
        let target_fallback = root.path().join("custom-target/kotlin/code-kotlin-engine.jar");

        let overrides = ResolverOverrides {
            current_exe_override: Some(fake_bin),
            workspace_jar_override: Some(workspace_fallback.clone()),
            cargo_target_jar_override: Some(target_fallback.clone()),
            ..Default::default()
        };

        let err = resolve_with_overrides(overrides).unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains(workspace_fallback.to_str().unwrap()));
        assert!(msg.contains(target_fallback.to_str().unwrap()));
    }
}
