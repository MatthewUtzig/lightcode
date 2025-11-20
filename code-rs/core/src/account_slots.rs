use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use code_app_server_protocol::AuthMode;
use dirs::home_dir;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::auth;
use crate::auth::AuthDotJson;
use crate::auth_accounts::StoredAccount;
use crate::config::resolve_code_path_for_read;

const SLOT_REGISTRY_FILE: &str = "slot_registry.json";
pub(crate) const SLOT_PREFIX: &str = "slot";
pub(crate) const MAX_SLOT_DEPTH: usize = 2;
const DEFAULT_SLOT_ID: &str = "slot-default";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSlot {
    pub id: String,
    pub label: Option<String>,
    pub path: PathBuf,
    pub has_auth_file: bool,
    pub is_default: bool,
}

impl AccountSlot {
    fn new(id: String, label: Option<String>, path: PathBuf, is_default: bool) -> Self {
        let has_auth_file = path.join("auth.json").is_file();
        Self { id, label, path, has_auth_file, is_default }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SlotRegistryEntry {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SlotRegistryFile {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    slots: Vec<SlotRegistryEntry>,
}

impl Default for SlotRegistryFile {
    fn default() -> Self {
        Self { version: default_version(), slots: Vec::new() }
    }
}

fn default_version() -> u32 {
    1
}

impl SlotRegistryFile {
    fn load(code_home: &Path) -> io::Result<Self> {
        let path = registry_path(code_home);
        match File::open(path) {
            Ok(mut file) => {
                let mut contents = String::new();
                file.read_to_string(&mut contents)?;
                let parsed: SlotRegistryFile = serde_json::from_str(&contents)?;
                Ok(parsed)
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err),
        }
    }

    fn save(&self, code_home: &Path) -> io::Result<()> {
        let path = registry_path(code_home);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        let mut options = OpenOptions::new();
        options.truncate(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        file.write_all(json.as_bytes())?;
        file.flush()?;
        Ok(())
    }

    fn ids(&self) -> HashSet<String> {
        self.slots.iter().map(|entry| entry.id.clone()).collect()
    }

    fn hydrate_from_filesystem(&mut self, code_home: &Path) -> io::Result<bool> {
        let mut dirty = false;
        let mut known_ids = self.ids();
        let discovered = scan_slot_dirs(code_home)?;
        for slot in discovered {
            if self
                .slots
                .iter()
                .any(|entry| resolve_entry_path(entry, code_home) == slot.path)
            {
                continue;
            }
            if !known_ids.insert(slot.id.clone()) {
                continue;
            }
            self.slots.push(SlotRegistryEntry {
                id: slot.id.clone(),
                label: slot.label,
                path: Some(relativize_path(code_home, &slot.path)),
            });
            dirty = true;
        }
        Ok(dirty)
    }

    fn remove(&mut self, slot_id: &str) -> Option<SlotRegistryEntry> {
        if let Some(idx) = self.slots.iter().position(|entry| entry.id == slot_id) {
            Some(self.slots.remove(idx))
        } else {
            None
        }
    }

    fn entry_mut(&mut self, slot_id: &str) -> Option<&mut SlotRegistryEntry> {
        self.slots.iter_mut().find(|entry| entry.id == slot_id)
    }

    fn entry(&self, slot_id: &str) -> Option<&SlotRegistryEntry> {
        self.slots.iter().find(|entry| entry.id == slot_id)
    }

    fn to_slots(&self, code_home: &Path) -> Vec<AccountSlot> {
        self.slots
            .iter()
            .map(|entry| {
                let resolved = resolve_entry_path(entry, code_home);
                AccountSlot::new(entry.id.clone(), entry.label.clone(), resolved, false)
            })
            .collect()
    }

    fn label_map(&self) -> HashMap<String, Option<String>> {
        self.slots
            .iter()
            .map(|entry| (entry.id.clone(), entry.label.clone()))
            .collect()
    }

    fn path_map(&self, code_home: &Path) -> HashMap<PathBuf, String> {
        self.slots
            .iter()
            .map(|entry| (resolve_entry_path(entry, code_home), entry.id.clone()))
            .collect()
    }
}

fn registry_path(code_home: &Path) -> PathBuf {
    code_home.join(SLOT_REGISTRY_FILE)
}

fn relativize_path(code_home: &Path, path: &Path) -> String {
    if let Ok(relative) = path.strip_prefix(code_home) {
        if relative.as_os_str().is_empty() {
            return String::from(".");
        }
        return relative.to_string_lossy().to_string();
    }
    path.to_string_lossy().to_string()
}

fn resolve_entry_path(entry: &SlotRegistryEntry, code_home: &Path) -> PathBuf {
    let raw = entry.path.as_deref().unwrap_or(&entry.id);
    if raw.starts_with('/') || raw.contains(':') {
        PathBuf::from(raw)
    } else if raw == "." {
        code_home.to_path_buf()
    } else {
        code_home.join(raw)
    }
}

/// Returns all known account slots, including the virtual default slot.
pub fn list_slots(code_home: &Path) -> io::Result<Vec<AccountSlot>> {
    let mut registry = SlotRegistryFile::load(code_home)?;
    let dirty = registry.hydrate_from_filesystem(code_home)?;
    if dirty {
        registry.save(code_home)?;
    }

    let mut slots = registry.to_slots(code_home);
    slots.push(default_slot(code_home));
    slots.sort_by(|a, b| slot_sort_key(a).cmp(&slot_sort_key(b)));
    Ok(slots)
}

fn slot_sort_key(slot: &AccountSlot) -> (bool, String, String) {
    let label = slot.label.clone().unwrap_or_else(|| slot.id.clone());
    (slot.id != DEFAULT_SLOT_ID, label.to_ascii_lowercase(), slot.id.clone())
}

fn default_slot(code_home: &Path) -> AccountSlot {
    let label = Some(slot_label(&["default".to_string()]));
    AccountSlot::new(DEFAULT_SLOT_ID.to_string(), label, code_home.to_path_buf(), true)
}

/// Adds a new slot rooted under `code_home` and records it in the registry.
pub fn add_slot(code_home: &Path, label: Option<&str>) -> io::Result<AccountSlot> {
    let mut registry = SlotRegistryFile::load(code_home)?;
    let mut existing_ids = registry.ids();
    let discovered = scan_slot_dirs(code_home)?;
    for slot in discovered {
        existing_ids.insert(slot.id);
    }

    let cleaned_label = label.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
    });

    let slug_component = cleaned_label
        .as_deref()
        .map(sanitize_slot_component)
        .filter(|slug| !slug.is_empty())
        .unwrap_or_else(|| "custom".to_string());
    let base_id = make_slot_id_slug(&[slug_component]);
    let unique_id = ensure_unique_slot_id(&base_id, &mut existing_ids);
    let dir_path = code_home.join(&unique_id);
    fs::create_dir_all(&dir_path)?;

    registry.slots.push(SlotRegistryEntry {
        id: unique_id.clone(),
        label: cleaned_label.clone(),
        path: Some(relativize_path(code_home, &dir_path)),
    });
    registry.save(code_home)?;

    Ok(AccountSlot::new(unique_id, cleaned_label, dir_path, false))
}

/// Removes a slot directory and registry entry. The default slot cannot be removed.
pub fn remove_slot(code_home: &Path, slot_id: &str) -> io::Result<Option<AccountSlot>> {
    if slot_id == DEFAULT_SLOT_ID {
        return Ok(None);
    }

    let mut registry = SlotRegistryFile::load(code_home)?;
    let entry = match registry.remove(slot_id) {
        Some(entry) => entry,
        None => return Ok(None),
    };
    registry.save(code_home)?;

    let path = resolve_entry_path(&entry, code_home);
    if path.exists() {
        let _ = fs::remove_dir_all(&path);
    }

    Ok(Some(AccountSlot::new(entry.id, entry.label, path, false)))
}

/// Renames a slot by updating its registry label. Returns the updated slot, if found.
pub fn rename_slot(code_home: &Path, slot_id: &str, new_label: Option<&str>) -> io::Result<Option<AccountSlot>> {
    if slot_id == DEFAULT_SLOT_ID {
        return Ok(None);
    }

    let mut registry = SlotRegistryFile::load(code_home)?;
    let Some(entry) = registry.entry_mut(slot_id) else {
        return Ok(None);
    };
    entry.label = new_label.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
    });
    let (id, label, path) = (
        entry.id.clone(),
        entry.label.clone(),
        resolve_entry_path(entry, code_home),
    );
    registry.save(code_home)?;

    Ok(Some(AccountSlot::new(id, label, path, false)))
}

/// Resolves the filesystem directory that should hold auth artifacts for the provided slot.
pub fn slot_auth_dir(code_home: &Path, slot_id: &str) -> io::Result<PathBuf> {
    if slot_id == DEFAULT_SLOT_ID {
        return Ok(code_home.to_path_buf());
    }

    let registry = SlotRegistryFile::load(code_home)?;
    let path = registry
        .entry(slot_id)
        .map(|entry| resolve_entry_path(entry, code_home))
        .unwrap_or_else(|| code_home.join(slot_id));
    fs::create_dir_all(&path)?;
    Ok(path)
}

pub(crate) fn slot_label(components: &[String]) -> String {
    if components.is_empty() {
        return "account".to_string();
    }
    format!("Slot {}", components.join(" / "))
}

pub(crate) fn sanitize_slot_component(component: &str) -> String {
    let mut slug = String::new();
    for ch in component.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_matches('-').to_string()
}

pub(crate) fn make_slot_id_slug(components: &[String]) -> String {
    let parts: Vec<String> = components
        .iter()
        .map(|component| sanitize_slot_component(component))
        .filter(|component| !component.is_empty())
        .collect();
    let slug = if parts.is_empty() { "slot".to_string() } else { parts.join("-") };
    format!("{SLOT_PREFIX}-{slug}")
}

pub(crate) fn ensure_unique_slot_id(base: &str, seen_ids: &mut HashSet<String>) -> String {
    if seen_ids.insert(base.to_string()) {
        return base.to_string();
    }

    let mut counter = 2usize;
    loop {
        let candidate = format!("{base}-{counter}");
        if seen_ids.insert(candidate.clone()) {
            return candidate;
        }
        counter += 1;
    }
}

#[derive(Debug, Clone)]
struct SlotDir {
    id: String,
    path: PathBuf,
    label: Option<String>,
    auth: Option<AuthDotJson>,
    components: Vec<String>,
}

fn scan_slot_dirs(code_home: &Path) -> io::Result<Vec<SlotDir>> {
    let mut slots = Vec::new();
    let mut seen_ids = HashSet::new();
    for root in slot_roots(code_home) {
        scan_slot_root(&root, Vec::new(), 0, &mut seen_ids, &mut slots)?;
    }
    Ok(slots)
}

fn slot_roots(code_home: &Path) -> Vec<PathBuf> {
    fn push_root(roots: &mut Vec<PathBuf>, candidate: PathBuf) {
        if candidate.exists() && !roots.iter().any(|root| root == &candidate) {
            roots.push(candidate);
        }
    }

    let mut roots = vec![code_home.to_path_buf()];
    let read_path = resolve_code_path_for_read(code_home, Path::new("auth.json"));
    if let Some(parent) = read_path.parent() {
        push_root(&mut roots, parent.to_path_buf());
    }
    if let Some(legacy) = legacy_code_home_dir() {
        push_root(&mut roots, legacy);
    }
    roots
}

fn legacy_code_home_dir() -> Option<PathBuf> {
    if env_overrides_present() {
        return None;
    }
    let home = home_dir()?;
    let candidate = home.join(".codex");
    if candidate.exists() {
        Some(candidate)
    } else {
        None
    }
}

fn env_overrides_present() -> bool {
    matches!(std::env::var("CODE_HOME"), Ok(ref v) if !v.trim().is_empty())
        || matches!(std::env::var("CODEX_HOME"), Ok(ref v) if !v.trim().is_empty())
}

fn scan_slot_root(
    root: &Path,
    components: Vec<String>,
    depth: usize,
    seen_ids: &mut HashSet<String>,
    out: &mut Vec<SlotDir>,
) -> io::Result<()> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.to_ascii_lowercase().starts_with(SLOT_PREFIX) {
            continue;
        }
        let mut next_components = components.clone();
        next_components.push(name.clone());
        scan_slot_dir(entry.path(), next_components, depth, seen_ids, out)?;
    }

    Ok(())
}

fn scan_slot_dir(
    path: PathBuf,
    components: Vec<String>,
    depth: usize,
    seen_ids: &mut HashSet<String>,
    out: &mut Vec<SlotDir>,
) -> io::Result<()> {
    if depth > MAX_SLOT_DEPTH {
        return Ok(());
    }

    let auth_path = path.join("auth.json");
    if auth_path.is_file() {
        match auth::try_read_auth_json(&auth_path) {
            Ok(auth_json) => {
                let id = ensure_unique_slot_id(&make_slot_id_slug(&components), seen_ids);
                let label = derive_label_from_auth(&auth_json, &components);
                out.push(SlotDir {
                    id,
                    path,
                    label: Some(label),
                    auth: Some(auth_json),
                    components,
                });
            }
            Err(err) => warn!(?auth_path, ?err, "failed to read slot auth file"),
        }
        return Ok(());
    }

    if depth == MAX_SLOT_DEPTH {
        return Ok(());
    }

    let entries = match fs::read_dir(&path) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let mut next_components = components.clone();
        next_components.push(name);
        scan_slot_dir(entry.path(), next_components, depth + 1, seen_ids, out)?;
    }

    Ok(())
}

fn derive_label_from_auth(auth_json: &AuthDotJson, components: &[String]) -> String {
    if let Some(tokens) = auth_json.tokens.as_ref() {
        if let Some(email) = tokens.id_token.email.as_deref() {
            let trimmed = email.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    slot_label(components)
}

/// Discovers slot-backed accounts, mirroring the previous auth discovery logic.
pub(crate) fn discover_slot_accounts(code_home: &Path) -> io::Result<Vec<StoredAccount>> {
    let registry = SlotRegistryFile::load(code_home)?;
    let overrides = registry.label_map();
    let id_by_path = registry.path_map(code_home);
    let mut accounts = Vec::new();
    let mut seen_ids = HashSet::new();

    for mut slot in scan_slot_dirs(code_home)? {
        let Some(auth_json) = slot.auth else {
            continue;
        };
        if let Some(custom_id) = id_by_path.get(&slot.path) {
            slot.id = custom_id.clone();
        }
        let id = slot.id.clone();
        let mut account = stored_account_from_auth(&id, auth_json, slot.label.clone(), slot.components.clone());
        if let Some(label) = overrides.get(&id).and_then(|value| value.clone()) {
            account.label = Some(label);
        }
        seen_ids.insert(id);
        accounts.push(account);
    }

    if let Some(default_account) = load_default_slot_account(code_home)? {
        let id = default_account.id.clone();
        let mut account = default_account;
        if let Some(label) = overrides.get(&id).and_then(|value| value.clone()) {
            account.label = Some(label);
        }
        if !seen_ids.contains(&id) {
            accounts.push(account);
        }
    }

    accounts.sort_by(|a, b| slot_display_key(a).cmp(&slot_display_key(b)));
    Ok(accounts)
}

fn stored_account_from_auth(
    id: &str,
    auth_json: AuthDotJson,
    label_hint: Option<String>,
    components: Vec<String>,
) -> StoredAccount {
    let mut tokens = auth_json.tokens.clone();
    let mode = if auth_json.tokens.is_some() { AuthMode::ChatGPT } else { AuthMode::ApiKey };

    if let (AuthMode::ChatGPT, Some(tokens_ref)) = (&mode, auth_json.tokens.as_ref()) {
        if tokens_ref.account_id.is_none() {
            tokens = Some(tokens_ref.clone());
        }
    }

    StoredAccount {
        id: id.to_string(),
        mode,
        label: label_hint.or_else(|| Some(derive_label_from_auth(&auth_json, &components))),
        openai_api_key: auth_json.openai_api_key,
        tokens,
        last_refresh: auth_json.last_refresh,
        created_at: None,
        last_used_at: None,
    }
}

fn load_default_slot_account(code_home: &Path) -> io::Result<Option<StoredAccount>> {
    let Some(auth_json) = auth::load_default_chatgpt_auth(code_home)? else {
        return Ok(None);
    };
    if auth_json.tokens.is_none() && auth_json.openai_api_key.is_none() {
        return Ok(None);
    }
    let label = slot_label(&["default".to_string()]);
    let mut account = stored_account_from_auth(
        DEFAULT_SLOT_ID,
        auth_json,
        Some(label.clone()),
        vec!["default".to_string()],
    );
    account.label = Some(label);
    Ok(Some(account))
}

fn slot_display_key(account: &StoredAccount) -> String {
    account
        .label
        .clone()
        .unwrap_or_else(|| account.id.clone())
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    #[allow(unused_imports)]
    use chrono::Utc;
    use crate::auth::{write_auth_json, AuthDotJson};
    use crate::token_data::{IdTokenInfo, TokenData};
    use tempfile::tempdir;

    fn fake_tokens(account_id: &str, email: &str) -> TokenData {
        fn fake_jwt(account_id: &str, email: &str) -> String {
            #[derive(Serialize)]
            struct Header {
                alg: &'static str,
                typ: &'static str,
            }
            let header = Header { alg: "none", typ: "JWT" };
            let payload = serde_json::json!({
                "email": email,
                "https://api.openai.com/auth": {
                    "chatgpt_plan_type": "pro",
                    "chatgpt_account_id": account_id,
                    "chatgpt_user_id": "user-12345",
                    "user_id": "user-12345",
                }
            });
            let b64 = |value: &serde_json::Value| {
                base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .encode(serde_json::to_vec(value).expect("json to vec"))
            };
            let header_b64 = b64(&serde_json::to_value(header).expect("header value"));
            let payload_b64 = b64(&payload);
            let signature_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"sig");
            format!("{header_b64}.{payload_b64}.{signature_b64}")
        }

        TokenData {
            id_token: IdTokenInfo {
                email: Some(email.to_string()),
                chatgpt_plan_type: None,
                raw_jwt: fake_jwt(account_id, email),
            },
            access_token: "access".to_string(),
            refresh_token: "refresh".to_string(),
            account_id: Some(account_id.to_string()),
        }
    }

    #[test]
    fn add_and_list_slots_round_trip() {
        let home = tempdir().expect("tempdir");
        let slots = list_slots(home.path()).expect("list");
        assert!(slots.iter().any(|slot| slot.id == DEFAULT_SLOT_ID));

        let created = add_slot(home.path(), Some("Work".into())).expect("add slot");
        assert_eq!(created.label.as_deref(), Some("Work"));

        let slots = list_slots(home.path()).expect("list");
        assert!(slots.iter().any(|slot| slot.id == created.id));
    }

    #[test]
    fn rename_slot_updates_registry() {
        let home = tempdir().expect("tempdir");
        let created = add_slot(home.path(), Some("Work".into())).expect("add slot");
        let renamed = rename_slot(home.path(), &created.id, Some("Personal".into())).expect("rename");
        let renamed = renamed.expect("slot exists");
        assert_eq!(renamed.label.as_deref(), Some("Personal"));

        let slots = list_slots(home.path()).expect("list");
        let slot = slots.iter().find(|slot| slot.id == created.id).expect("slot");
        assert_eq!(slot.label.as_deref(), Some("Personal"));
    }

    #[test]
    fn remove_slot_deletes_directory() {
        let home = tempdir().expect("tempdir");
        let created = add_slot(home.path(), Some("Work".into())).expect("add slot");
        let dir = created.path.clone();
        assert!(dir.exists());
        remove_slot(home.path(), &created.id).expect("remove");
        assert!(!dir.exists());
    }

    #[test]
    fn discover_slot_accounts_uses_custom_labels() {
        let home = tempdir().expect("tempdir");
        let created = add_slot(home.path(), Some("My Slot".into())).expect("add slot");
        let auth_path = created.path.join("auth.json");
        let auth = AuthDotJson {
            openai_api_key: None,
            tokens: Some(fake_tokens("acct-slot", "slot@example.com")),
            last_refresh: Some(Utc::now()),
        };
        write_auth_json(&auth_path, &auth).expect("write auth");

        let accounts = discover_slot_accounts(home.path()).expect("discover");
        let slot_account = accounts.iter().find(|acc| acc.id == created.id).expect("slot account");
        assert_eq!(slot_account.label.as_deref(), Some("My Slot"));
    }
}
