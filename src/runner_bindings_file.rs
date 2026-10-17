//! Explicit Runner credential provisioning from owner-protected TOML.
//!
//! The Server stores only salted-free SHA-256 hashes consistent with its
//! existing token verifier. The plaintext Runner credential is stored on its
//! own machine in a separate owner-private file, never in this configuration.
use crate::auth::{
    validate_allowed_client_id, validate_token_prefix, validate_username, AGENT_SCOPES,
};
use crate::models::{ApiKeyRecord, UserRecord, TOKEN_KIND_AGENT};
use crate::Database;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;

const MAX_BINDINGS_BYTES: u64 = 64 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Bindings {
    runners: Vec<Binding>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    username: String,
    client_id: String,
    token_hash: String,
    token_prefix: String,
}

fn read_file(path: &Path) -> Result<Vec<Binding>, String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err("Runner bindings path must be absolute without parent traversal".into());
    }
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| "Runner bindings TOML cannot be opened".to_string())?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_BINDINGS_BYTES
    {
        return Err("Runner bindings TOML must be a bounded regular non-symlink file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.permissions().mode() & 0o077 != 0 || metadata.nlink() != 1 {
            return Err("Runner bindings TOML must have owner-only permissions".into());
        }
    }
    let source = std::fs::read_to_string(path)
        .map_err(|_| "Runner bindings TOML cannot be read".to_string())?;
    let content: Bindings = toml::from_str(&source)
        .map_err(|_| "Runner bindings TOML contains invalid entries or fields".to_string())?;
    if content.runners.is_empty() || content.runners.len() > 64 {
        return Err("Runner bindings TOML must contain 1-64 runners".into());
    }
    let mut identities = HashSet::new();
    let mut hashes = HashSet::new();
    for item in &content.runners {
        validate_username(&item.username)?;
        validate_allowed_client_id(&item.client_id)?;
        validate_token_prefix(&item.token_prefix, "wc_agent_")?;
        if item.token_prefix.len() != 16
            || item.token_hash.len() != 64
            || !item
                .token_hash
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err("Runner token prefix/hash has invalid shape".into());
        }
        if !identities.insert((item.username.clone(), item.client_id.clone()))
            || !hashes.insert(item.token_hash.clone())
        {
            return Err("Runner bindings contain duplicate identities or hashes".into());
        }
    }
    Ok(content.runners)
}

/// Validate the configured Runner binding identities without modifying the
/// database. Used by `webcodex-server --check-config`.
pub(crate) fn validate_bindings_file(path: &Path) -> Result<(), String> {
    read_file(path).map(|_| ())
}

/// Preprovision one or more exact Runner identities, with no plaintext or
/// network pairing. Never revive revoked tokens or silently rotate bindings.
pub(crate) fn provision_from_file(db: &Database, path: &Path) -> Result<(), String> {
    let bindings = read_file(path)?;
    for binding in &bindings {
        let user = db
            .get_user_by_username(&binding.username)
            .map_err(|_| "Runner binding user lookup failed".to_string())?;
        if user.as_ref().is_some_and(UserRecord::is_disabled) {
            return Err("configured Runner user is disabled".into());
        }
        let existing = db
            .get_api_key_by_hash(&binding.token_hash)
            .map_err(|_| "Runner binding lookup failed".to_string())?;
        if db
            .get_account_credential_by_hash(&binding.token_hash)
            .map_err(|_| "Runner binding credential collision lookup failed".to_string())?
            .is_some()
        {
            return Err("Runner binding conflicts with account credential".into());
        }
        if let Some(token) = &existing {
            if !token.is_agent_token()
                || token.allowed_client_id() != Some(&binding.client_id)
                || token.key_prefix != binding.token_prefix
                || token.is_expired(chrono::Utc::now().timestamp())
                || token
                    .scopes_vec()
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    != AGENT_SCOPES
                || user.as_ref().map(|u| u.id.as_str()) != Some(token.user_id.as_str())
            {
                return Err("Runner binding conflicts with stored authorization".into());
            }
        }
        if let Some(user) = &user {
            let prior = db
                .list_agent_api_keys_by_user(&user.id)
                .map_err(|_| "Runner binding uniqueness lookup failed".to_string())?;
            if prior.iter().any(|old| {
                old.allowed_client_id() == Some(&binding.client_id)
                    && !old.is_revoked()
                    && !old.is_expired(chrono::Utc::now().timestamp())
                    && existing.as_ref().is_none_or(|current| current.id != old.id)
            }) {
                return Err(
                    "Runner identity has a different active token; explicit revocation required"
                        .into(),
                );
            }
        }
    }
    for binding in bindings {
        if db
            .get_api_key_by_hash(&binding.token_hash)
            .map_err(|_| "Runner binding token lookup failed".to_string())?
            .is_some()
        {
            continue;
        }
        let user = match db
            .get_user_by_username(&binding.username)
            .map_err(|_| "Runner binding user lookup failed".to_string())?
        {
            Some(user) => user,
            None => {
                let now = chrono::Utc::now().timestamp();
                let user = UserRecord {
                    id: uuid::Uuid::new_v4().to_string(),
                    username: binding.username.clone(),
                    created_at: now,
                    disabled: 0,
                    display_name: None,
                    role: "user".into(),
                    disabled_at: None,
                    updated_at: Some(now),
                };
                db.create_user(&user)
                    .map_err(|_| "Runner binding cannot create user".to_string())?;
                user
            }
        };
        let record = ApiKeyRecord {
            id: uuid::Uuid::new_v4().to_string(),
            user_id: user.id,
            name: format!("config:{}", binding.client_id),
            key_prefix: binding.token_prefix,
            created_at: chrono::Utc::now().timestamp(),
            last_used_at: None,
            revoked_at: None,
            scopes: AGENT_SCOPES.join(" "),
            expires_at: None,
            kind: TOKEN_KIND_AGENT.to_string(),
            allowed_client_id: Some(binding.client_id),
        };
        db.insert_api_key(&record, &binding.token_hash)
            .map_err(|_| {
                "Runner binding registration failed or revoked hash was reused".to_string()
            })?;
    }
    Ok(())
}

/// Apply optional Server TOML runner-binding reference after database open.
pub(crate) fn provision_from_environment(db: &Database) -> Result<(), String> {
    let Some(path) = std::env::var_os("WEBCODEX_RUNNER_BINDINGS_FILE") else {
        return Ok(());
    };
    provision_from_file(db, Path::new(&path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn setup() -> (tempfile::TempDir, Database, String, String) {
        let tmp = tempfile::tempdir().unwrap();
        let db = Database::open(&tmp.path().join("test.db")).unwrap();
        let token = format!("wc_agent_{}", "a".repeat(64));
        let hash = format!("{:x}", Sha256::digest(token.as_bytes()));
        (tmp, db, token, hash)
    }
    fn fixture(path: &Path, hash: &str) {
        std::fs::write(path, format!(
            "[[runners]]\nusername = \"alice\"\nclient_id = \"alice-laptop\"\ntoken_hash = \"{hash}\"\ntoken_prefix = \"wc_agent_aaaaaaa\"\n"
        )).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    #[test]
    fn registers_exact_identity_and_is_idempotent() {
        let (tmp, db, token, hash) = setup();
        let file = tmp.path().join("runner-bindings.toml");
        fixture(&file, &hash);
        provision_from_file(&db, &file).unwrap();
        provision_from_file(&db, &file).unwrap();
        let record = db.get_api_key_by_hash(&hash).unwrap().unwrap();
        assert_eq!(record.key_prefix, &token[..16]);
        assert_eq!(record.allowed_client_id(), Some("alice-laptop"));
        assert_eq!(record.kind(), TOKEN_KIND_AGENT);
        assert_eq!(
            record
                .scopes_vec()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            AGENT_SCOPES
        );
        assert_eq!(
            db.list_agent_api_keys_by_user(&record.user_id)
                .unwrap()
                .len(),
            1
        );
    }
    #[test]
    fn rejects_unprotected_and_revoked_identity() {
        let (tmp, db, _, hash) = setup();
        let file = tmp.path().join("runner-bindings.toml");
        fixture(&file, &hash);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(provision_from_file(&db, &file).is_err());
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        provision_from_file(&db, &file).unwrap();
        let record = db.get_api_key_by_hash(&hash).unwrap().unwrap();
        db.revoke_api_key(&record.id, chrono::Utc::now().timestamp())
            .unwrap();
        assert!(provision_from_file(&db, &file).is_err());
    }
}
