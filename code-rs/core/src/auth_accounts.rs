use chrono::{DateTime, Utc};
use code_app_server_protocol::AuthMode;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use tracing::warn;
use uuid::Uuid;

use crate::auth;
use crate::config::resolve_code_path_for_read;
use crate::token_data::TokenData;

const ACCOUNTS_FILE_NAME: &str = "auth_accounts.json";
const SLOT_PREFIX: &str = "slot";
const MAX_SLOT_DEPTH: usize = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StoredAccount {
    pub id: String,
    pub mode: AuthMode,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai_api_key: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<TokenData>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_refresh: Option<DateTime<Utc>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct AccountsFile {
    #[serde(default = "default_version")]
    version: u32,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_account_id: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    accounts: Vec<StoredAccount>,
}

impl Default for AccountsFile {
    fn default() -> Self {
        Self {
            version: default_version(),
            active_account_id: None,
            accounts: Vec::new(),
        }
    }
}

fn default_version() -> u32 {
    1
}

fn accounts_file_path(code_home: &Path) -> PathBuf {
    code_home.join(ACCOUNTS_FILE_NAME)
}

fn read_accounts_file(path: &Path) -> io::Result<AccountsFile> {
    match File::open(path) {
        Ok(mut file) => {
            let mut contents = String::new();
            file.read_to_string(&mut contents)?;
            let parsed: AccountsFile = serde_json::from_str(&contents)?;
            Ok(parsed)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(AccountsFile::default()),
        Err(e) => Err(e),
    }
}

fn write_accounts_file(path: &Path, data: &AccountsFile) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let json = serde_json::to_string_pretty(data)?;
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

fn normalize_email(email: &str) -> String {
    email.trim().to_ascii_lowercase()
}

fn now() -> DateTime<Utc> {
    Utc::now()
}

fn next_id() -> String {
    Uuid::new_v4().to_string()
}

fn match_chatgpt_account(existing: &StoredAccount, tokens: &TokenData) -> bool {
    if existing.mode != AuthMode::ChatGPT {
        return false;
    }

    let existing_tokens = match &existing.tokens {
        Some(tokens) => tokens,
        None => return false,
    };

    let account_id_matches = match (&existing_tokens.account_id, &tokens.account_id) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    };

    let email_matches = match (
        existing_tokens.id_token.email.as_ref(),
        tokens.id_token.email.as_ref(),
    ) {
        (Some(a), Some(b)) => normalize_email(a) == normalize_email(b),
        _ => false,
    };

    account_id_matches && email_matches
}

fn match_api_key_account(existing: &StoredAccount, api_key: &str) -> bool {
    existing.mode == AuthMode::ApiKey
        && existing
            .openai_api_key
            .as_ref()
            .is_some_and(|stored| stored == api_key)
}

fn touch_account(account: &mut StoredAccount, used: bool) {
    if account.created_at.is_none() {
        account.created_at = Some(now());
    }
    if used {
        account.last_used_at = Some(now());
    }
}

fn upsert_account(mut data: AccountsFile, mut new_account: StoredAccount) -> (AccountsFile, StoredAccount) {
    let existing_idx = match new_account.mode {
        AuthMode::ChatGPT => new_account
            .tokens
            .as_ref()
            .and_then(|tokens| data.accounts.iter().position(|acc| match_chatgpt_account(acc, tokens))),
        AuthMode::ApiKey => new_account
            .openai_api_key
            .as_ref()
            .and_then(|api_key| data.accounts.iter().position(|acc| match_api_key_account(acc, api_key))),
    };

    if let Some(idx) = existing_idx {
        let mut account = data.accounts[idx].clone();
        if new_account.label.is_some() {
            account.label = new_account.label;
        }
        if new_account.last_refresh.is_some() {
            account.last_refresh = new_account.last_refresh;
        }
        if let Some(tokens) = new_account.tokens {
            account.tokens = Some(tokens);
        }
        if let Some(api_key) = new_account.openai_api_key {
            account.openai_api_key = Some(api_key);
        }
        if let Some(last_used) = new_account.last_used_at {
            account.last_used_at = Some(last_used);
        }
        data.accounts[idx] = account.clone();
        return (data, account);
    }

    if new_account.created_at.is_none() {
        new_account.created_at = Some(now());
    }

    data.accounts.push(new_account.clone());
    (data, new_account)
}

pub fn list_accounts(code_home: &Path) -> io::Result<Vec<StoredAccount>> {
    let path = accounts_file_path(code_home);
    let data = read_accounts_file(&path)?;
    let mut accounts = data.accounts;
    match discover_slot_accounts(code_home) {
        Ok(mut slots) => accounts.append(&mut slots),
        Err(err) => warn!(?err, "failed to load slot-based accounts"),
    }
    Ok(accounts)
}

pub fn get_active_account_id(code_home: &Path) -> io::Result<Option<String>> {
    let path = accounts_file_path(code_home);
    let data = read_accounts_file(&path)?;
    Ok(data.active_account_id)
}

pub fn find_account(code_home: &Path, account_id: &str) -> io::Result<Option<StoredAccount>> {
    let path = accounts_file_path(code_home);
    let data = read_accounts_file(&path)?;
    if let Some(account) = data
        .accounts
        .into_iter()
        .find(|acc| acc.id == account_id)
    {
        return Ok(Some(account));
    }

    match discover_slot_accounts(code_home) {
        Ok(slots) => Ok(slots.into_iter().find(|acc| acc.id == account_id)),
        Err(err) => {
            warn!(?err, "failed to load slot-based accounts");
            Ok(None)
        }
    }
}

pub fn set_active_account_id(
    code_home: &Path,
    account_id: Option<String>,
) -> io::Result<Option<StoredAccount>> {
    let path = accounts_file_path(code_home);
    let mut data = read_accounts_file(&path)?;

    data.active_account_id = account_id.clone();

    if let Some(id) = account_id {
        if let Some(account) = data.accounts.iter_mut().find(|acc| acc.id == id) {
            touch_account(account, true);
            let updated = account.clone();
            write_accounts_file(&path, &data)?;
            return Ok(Some(updated));
        }
        write_accounts_file(&path, &data)?;
        Ok(None)
    } else {
        write_accounts_file(&path, &data)?;
        Ok(None)
    }
}

pub fn remove_account(code_home: &Path, account_id: &str) -> io::Result<Option<StoredAccount>> {
    let path = accounts_file_path(code_home);
    let mut data = read_accounts_file(&path)?;

    let removed = if let Some(pos) = data.accounts.iter().position(|acc| acc.id == account_id) {
        Some(data.accounts.remove(pos))
    } else {
        None
    };

    if data
        .active_account_id
        .as_ref()
        .is_some_and(|active| active == account_id)
    {
        data.active_account_id = None;
    }

    write_accounts_file(&path, &data)?;
    Ok(removed)
}

pub fn upsert_api_key_account(
    code_home: &Path,
    api_key: String,
    label: Option<String>,
    make_active: bool,
) -> io::Result<StoredAccount> {
    let path = accounts_file_path(code_home);
    let data = read_accounts_file(&path)?;

    let new_account = StoredAccount {
        id: next_id(),
        mode: AuthMode::ApiKey,
        label,
        openai_api_key: Some(api_key),
        tokens: None,
        last_refresh: None,
        created_at: None,
        last_used_at: None,
    };

    let (mut data, mut stored) = upsert_account(data, new_account);

    if make_active {
        data.active_account_id = Some(stored.id.clone());
        if let Some(account) = data
            .accounts
            .iter_mut()
            .find(|acc| acc.id == stored.id)
        {
            touch_account(account, true);
            stored = account.clone();
        }
    }

    write_accounts_file(&path, &data)?;
    Ok(stored)
}

fn discover_slot_accounts(code_home: &Path) -> io::Result<Vec<StoredAccount>> {
    let mut accounts = Vec::new();
    let mut seen_ids = HashSet::new();
    for root in slot_roots(code_home) {
        if let Err(err) = scan_slot_root(&root, &mut seen_ids, &mut accounts) {
            if err.kind() == ErrorKind::NotFound {
                continue;
            }
            return Err(err);
        }
    }

    accounts.sort_by(|a, b| slot_display_key(a).cmp(&slot_display_key(b)));
    Ok(accounts)
}

fn slot_roots(code_home: &Path) -> Vec<PathBuf> {
    let mut roots = vec![code_home.to_path_buf()];
    let read_path = resolve_code_path_for_read(code_home, Path::new("auth.json"));
    if let Some(parent) = read_path.parent() {
        if !roots.iter().any(|root| root == parent) {
            roots.push(parent.to_path_buf());
        }
    }
    roots
}

fn scan_slot_root(
    root: &Path,
    seen_ids: &mut HashSet<String>,
    out: &mut Vec<StoredAccount>,
) -> io::Result<()> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
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
        let name = entry
            .file_name()
            .to_string_lossy()
            .into_owned();
        if !name.to_ascii_lowercase().starts_with(SLOT_PREFIX) {
            continue;
        }
        scan_slot_dir(entry.path(), vec![name], 0, seen_ids, out)?;
    }

    Ok(())
}

fn scan_slot_dir(
    path: PathBuf,
    components: Vec<String>,
    depth: usize,
    seen_ids: &mut HashSet<String>,
    out: &mut Vec<StoredAccount>,
) -> io::Result<()> {
    if depth > MAX_SLOT_DEPTH {
        return Ok(());
    }

    let auth_path = path.join("auth.json");
    if auth_path.is_file() {
        match auth::try_read_auth_json(&auth_path) {
            Ok(auth_json) => push_slot_account(auth_json, &components, seen_ids, out),
            Err(err) => warn!(?auth_path, ?err, "failed to read slot auth file"),
        }
        return Ok(());
    }

    if depth == MAX_SLOT_DEPTH {
        return Ok(());
    }

    let entries = match fs::read_dir(&path) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
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

fn push_slot_account(
    auth_json: auth::AuthDotJson,
    components: &[String],
    seen_ids: &mut HashSet<String>,
    out: &mut Vec<StoredAccount>,
) {
    let base_id = make_slot_id_slug(components);
    let id = ensure_unique_slot_id(&base_id, seen_ids);
    let label = format!("Slot {}", slot_label(components));
    let mode = if auth_json.tokens.is_some() {
        AuthMode::ChatGPT
    } else {
        AuthMode::ApiKey
    };

    out.push(StoredAccount {
        id,
        mode,
        label: Some(label),
        openai_api_key: auth_json.openai_api_key,
        tokens: auth_json.tokens,
        last_refresh: auth_json.last_refresh,
        created_at: None,
        last_used_at: None,
    });
}

fn slot_display_key(account: &StoredAccount) -> String {
    account
        .label
        .clone()
        .unwrap_or_else(|| account.id.clone())
        .to_ascii_lowercase()
}

fn slot_label(components: &[String]) -> String {
    if components.is_empty() {
        return "account".to_string();
    }
    components.join(" / ")
}

fn make_slot_id_slug(components: &[String]) -> String {
    let parts: Vec<String> = components
        .iter()
        .map(|component| sanitize_slot_component(component))
        .filter(|component| !component.is_empty())
        .collect();
    let slug = if parts.is_empty() {
        "slot".to_string()
    } else {
        parts.join("-")
    };
    format!("slot-{slug}")
}

fn sanitize_slot_component(component: &str) -> String {
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

fn ensure_unique_slot_id(base: &str, seen_ids: &mut HashSet<String>) -> String {
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

pub fn upsert_chatgpt_account(
    code_home: &Path,
    tokens: TokenData,
    last_refresh: DateTime<Utc>,
    label: Option<String>,
    make_active: bool,
) -> io::Result<StoredAccount> {
    let path = accounts_file_path(code_home);
    let data = read_accounts_file(&path)?;

    let new_account = StoredAccount {
        id: next_id(),
        mode: AuthMode::ChatGPT,
        label,
        openai_api_key: None,
        tokens: Some(tokens),
        last_refresh: Some(last_refresh),
        created_at: None,
        last_used_at: None,
    };

    let (mut data, mut stored) = upsert_account(data, new_account);

    if make_active {
        data.active_account_id = Some(stored.id.clone());
        if let Some(account) = data
            .accounts
            .iter_mut()
            .find(|acc| acc.id == stored.id)
        {
            touch_account(account, true);
            stored = account.clone();
        }
    }

    write_accounts_file(&path, &data)?;
    Ok(stored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use crate::auth::{write_auth_json, AuthDotJson};
    use crate::token_data::{IdTokenInfo, TokenData};
    use tempfile::tempdir;

    fn make_chatgpt_tokens(account_id: Option<&str>, email: Option<&str>) -> TokenData {
        fn fake_jwt(account_id: Option<&str>, email: Option<&str>, plan: &str) -> String {
            #[derive(Serialize)]
            struct Header {
                alg: &'static str,
                typ: &'static str,
            }
            let header = Header {
                alg: "none",
                typ: "JWT",
            };
            let payload = serde_json::json!({
                "email": email,
                "https://api.openai.com/auth": {
                    "chatgpt_plan_type": plan,
                    "chatgpt_account_id": account_id.unwrap_or("acct"),
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
                email: email.map(|s| s.to_string()),
                chatgpt_plan_type: None,
                raw_jwt: fake_jwt(account_id, email, "pro"),
            },
            access_token: "access".to_string(),
            refresh_token: "refresh".to_string(),
            account_id: account_id.map(|s| s.to_string()),
        }
    }

    #[test]
    fn upsert_api_key_creates_and_updates() {
        let home = tempdir().expect("tempdir");
        let api_key = "sk-test".to_string();
        let stored = upsert_api_key_account(home.path(), api_key.clone(), None, true)
            .expect("upsert api key");

        assert_eq!(stored.mode, AuthMode::ApiKey);
        assert_eq!(stored.openai_api_key.as_deref(), Some("sk-test"));

        let again = upsert_api_key_account(home.path(), api_key, None, false)
            .expect("upsert same key");
        assert_eq!(stored.id, again.id);

        let accounts = list_accounts(home.path()).expect("list accounts");
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].id, stored.id);
    }

    #[test]
    fn upsert_chatgpt_dedupes_by_account_id() {
        let home = tempdir().expect("tempdir");
        let tokens = make_chatgpt_tokens(Some("acct-1"), Some("user@example.com"));
        let stored = upsert_chatgpt_account(
            home.path(),
            tokens.clone(),
            Utc::now(),
            None,
            true,
        )
        .expect("insert chatgpt");

        let tokens_updated = make_chatgpt_tokens(Some("acct-1"), Some("user@example.com"));
        let again = upsert_chatgpt_account(
            home.path(),
            tokens_updated,
            Utc::now(),
            None,
            false,
        )
        .expect("update chatgpt");

        assert_eq!(stored.id, again.id);
        let accounts = list_accounts(home.path()).expect("list accounts");
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].id, stored.id);
    }

    #[test]
    fn chatgpt_accounts_with_same_email_but_different_ids_are_distinct() {
        let home = tempdir().expect("tempdir");

        let personal = make_chatgpt_tokens(Some("acct-personal"), Some("user@example.com"));
        let personal_id = upsert_chatgpt_account(
            home.path(),
            personal,
            Utc::now(),
            None,
            true,
        )
        .expect("insert personal account")
        .id;

        let team = make_chatgpt_tokens(Some("acct-team"), Some("user@example.com"));
        let team_id = upsert_chatgpt_account(
            home.path(),
            team,
            Utc::now(),
            None,
            false,
        )
        .expect("insert team account")
        .id;

        assert_ne!(personal_id, team_id, "accounts with different IDs should not be merged");

        let accounts = list_accounts(home.path()).expect("list accounts");
        assert_eq!(accounts.len(), 2, "both accounts should remain listed");
    }

    #[test]
    fn remove_account_clears_active() {
        let home = tempdir().expect("tempdir");
        let tokens = make_chatgpt_tokens(Some("acct-remove"), Some("user@example.com"));
        let stored = upsert_chatgpt_account(
            home.path(),
            tokens,
            Utc::now(),
            None,
            true,
        )
        .expect("insert chatgpt");

        let active_before = get_active_account_id(home.path()).expect("active id");
        assert_eq!(active_before.as_deref(), Some(stored.id.as_str()));

        let removed = remove_account(home.path(), &stored.id).expect("remove");
        assert!(removed.is_some());

        let active_after = get_active_account_id(home.path()).expect("active id");
        assert!(active_after.is_none());
    }

    #[test]
    fn list_accounts_includes_slot_directories() {
        let home = tempdir().expect("tempdir");
        let slot_dir = home.path().join("slot-one");
        std::fs::create_dir_all(&slot_dir).expect("slot dir");

        let tokens = make_chatgpt_tokens(Some("acct-slot"), Some("slot@example.com"));
        let auth = AuthDotJson {
            openai_api_key: None,
            tokens: Some(tokens),
            last_refresh: Some(Utc::now()),
        };
        write_auth_json(&slot_dir.join("auth.json"), &auth).expect("write auth");

        let accounts = list_accounts(home.path()).expect("list");
        let slot_account = accounts
            .iter()
            .find(|acc| acc.id.starts_with("slot-"))
            .expect("missing slot account");
        assert_eq!(slot_account.mode, AuthMode::ChatGPT);
        assert_eq!(
            slot_account
                .tokens
                .as_ref()
                .and_then(|tokens| tokens.account_id.as_deref()),
            Some("acct-slot"),
        );
        assert!(slot_account
            .label
            .as_ref()
            .is_some_and(|label| label.contains("Slot")));
    }

    #[test]
    fn nested_slot_directories_are_discovered() {
        let home = tempdir().expect("tempdir");
        let nested = home.path().join("slot").join("beta");
        std::fs::create_dir_all(&nested).expect("nested slot dir");

        let auth = AuthDotJson {
            openai_api_key: Some("sk-slot".to_string()),
            tokens: None,
            last_refresh: None,
        };
        write_auth_json(&nested.join("auth.json"), &auth).expect("write auth");

        let accounts = list_accounts(home.path()).expect("list");
        let slot_account = accounts
            .iter()
            .find(|acc| acc.openai_api_key.is_some())
            .expect("slot not discovered");
        assert_eq!(slot_account.mode, AuthMode::ApiKey);
        assert!(slot_account.id.starts_with("slot-"));
    }
}
