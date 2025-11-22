use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

fn main() {
    println!("cargo:rerun-if-env-changed=CODE_SKIP_KOTLIN_JAR_BUILD");
    if env::var("DOCS_RS").is_ok() {
        return;
    }
    if env::var("CODE_SKIP_KOTLIN_JAR_BUILD").map(|v| v == "1").unwrap_or(false) {
        return;
    }

    if let Err(err) = ensure_kotlin_jar() {
        panic!("failed to build Kotlin engine jar: {err}");
    }
}

fn ensure_kotlin_jar() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let repo_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .ok_or("failed to locate repo root")?
        .to_path_buf();

    let target_dir = env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| repo_root.join("code-rs/target"));
    let jar_path = target_dir.join("kotlin/code-kotlin-engine.jar");

    println!(
        "cargo:rerun-if-changed={}",
        repo_root.join("scripts/build-kotlin-engine.sh").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        repo_root.join("core-kotlin/build.gradle.kts").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        repo_root.join("core-kotlin/settings.gradle.kts").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        repo_root.join("core-kotlin/src").display()
    );

    if jar_is_fresh(&jar_path, &repo_root.join("core-kotlin/src"))? {
        return Ok(());
    }

    let status = Command::new(repo_root.join("scripts/build-kotlin-engine.sh"))
        .current_dir(&repo_root)
        .status()?;
    if !status.success() {
        return Err("scripts/build-kotlin-engine.sh failed".into());
    }

    Ok(())
}

fn jar_is_fresh(jar_path: &Path, src_dir: &Path) -> Result<bool, Box<dyn std::error::Error>> {
    let jar_meta = match fs::metadata(jar_path) {
        Ok(meta) => meta,
        Err(_) => return Ok(false),
    };
    let jar_mtime = jar_meta.modified()?;
    let src_mtime = newest_mtime(src_dir)?;
    Ok(jar_mtime >= src_mtime)
}

fn newest_mtime(path: &Path) -> Result<SystemTime, Box<dyn std::error::Error>> {
    let mut newest = SystemTime::UNIX_EPOCH;
    if path.is_file() {
        newest = fs::metadata(path)?.modified()?;
        return Ok(newest);
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        let mtime = if meta.is_dir() {
            newest_mtime(&entry.path())?
        } else {
            meta.modified()?
        };
        if mtime > newest {
            newest = mtime;
        }
    }
    Ok(newest)
}
