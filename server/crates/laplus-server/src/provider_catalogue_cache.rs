//! Durable, secret-free memory of authoritative OpenCode catalogues.

use crate::config::ProviderModel;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs, io,
    path::Path,
    sync::{Mutex, OnceLock},
};

const FILE: &str = "provider-catalogues.json";
const VERSION: u32 = 1;
const MAX_BYTES: u64 = 2 * 1024 * 1024;
const MAX_MODELS: usize = 512;
static WRITES: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Cache {
    version: u32,
    entries: BTreeMap<String, Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Entry {
    pub instance_id: String,
    pub driver: String,
    pub identity_fingerprint: String,
    pub version: Option<String>,
    pub checked_at: String,
    pub models: Vec<ProviderModel>,
}

pub fn fingerprint(identity: &str) -> String {
    let digest = Sha256::digest(identity.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn external_identity(url: &str) -> Option<String> {
    let mut url = reqwest::Url::parse(url).ok()?;
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    Some(fingerprint(url.as_str()))
}

pub fn executable_identity(path: &Path) -> String {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut identity = path.to_string_lossy().into_owned();
    if let Ok(metadata) = fs::metadata(&path) {
        identity.push_str(&format!("\0{}", metadata.len()));
        if let Ok(modified) = metadata.modified().and_then(|time| {
            time.duration_since(std::time::UNIX_EPOCH)
                .map_err(io::Error::other)
        }) {
            identity.push_str(&format!("\0{}", modified.as_nanos()));
        }
    }
    fingerprint(&identity)
}

pub fn load(directory: &Path, instance_id: &str, driver: &str, identity: &str) -> Option<Entry> {
    let path = directory.join(FILE);
    if fs::metadata(&path).ok()?.len() > MAX_BYTES {
        return None;
    }
    let cache: Cache = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    if cache.version != VERSION {
        return None;
    }
    let entry = cache.entries.get(instance_id)?.clone();
    (entry.instance_id == instance_id
        && entry.driver == driver
        && entry.identity_fingerprint == identity
        && entry.models.len() <= MAX_MODELS
        && entry.models.iter().all(|model| !model.is_custom))
    .then_some(entry)
}

pub fn store(directory: &Path, mut entry: Entry) -> io::Result<()> {
    let _write = WRITES
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    entry.models.retain(|model| !model.is_custom);
    if entry.models.len() > MAX_MODELS {
        entry.models.truncate(MAX_MODELS);
    }
    let path = directory.join(FILE);
    let mut cache = fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Cache>(&bytes).ok())
        .filter(|cache| cache.version == VERSION)
        .unwrap_or(Cache {
            version: VERSION,
            entries: BTreeMap::new(),
        });
    cache.entries.insert(entry.instance_id.clone(), entry);
    fs::create_dir_all(directory)?;
    let bytes = serde_json::to_vec(&cache).map_err(io::Error::other)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(io::Error::other("provider catalogue cache is too large"));
    }
    atomic_replace(&path, &bytes)?;
    Ok(())
}

pub fn remove(directory: &Path, instance_id: &str) -> io::Result<()> {
    let _write = WRITES
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let path = directory.join(FILE);
    let Some(mut cache) = fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Cache>(&bytes).ok())
    else {
        return Ok(());
    };
    if cache.version != VERSION || cache.entries.remove(instance_id).is_none() {
        return Ok(());
    }
    let bytes = serde_json::to_vec(&cache).map_err(io::Error::other)?;
    atomic_replace(&path, &bytes)?;
    Ok(())
}

fn atomic_replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use io::Write;
    let mut file = atomic_write_file::AtomicWriteFile::options().open(path)?;
    file.write_all(bytes)?;
    file.commit()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn model(slug: &str) -> ProviderModel {
        ProviderModel {
            slug: slug.into(),
            name: slug.into(),
            is_custom: false,
            sub_provider: None,
            is_default: None,
            capabilities: Some(serde_json::json!({"optionDescriptors":[]})),
        }
    }

    #[test]
    fn cache_round_trip_is_versioned_identity_bound_and_omits_custom_models() {
        let directory = tempfile::tempdir().unwrap();
        store(
            directory.path(),
            Entry {
                instance_id: "work".into(),
                driver: "opencode".into(),
                identity_fingerprint: "one".into(),
                version: Some("1.2.3".into()),
                checked_at: "2026-08-13T00:00:00Z".into(),
                models: vec![
                    model("openai/gpt"),
                    ProviderModel {
                        is_custom: true,
                        ..model("mine/local")
                    },
                ],
            },
        )
        .unwrap();
        assert!(load(directory.path(), "work", "opencode", "wrong").is_none());
        let loaded = load(directory.path(), "work", "opencode", "one").unwrap();
        assert_eq!(loaded.models, vec![model("openai/gpt")]);
        let text = fs::read_to_string(directory.path().join(FILE)).unwrap();
        assert!(!text.contains("mine/local"));
    }

    #[test]
    fn external_identity_drops_credentials_query_and_fragment() {
        assert_eq!(
            external_identity("https://user:secret@example.test/api?token=hush#x"),
            external_identity("https://example.test/api")
        );
    }

    #[test]
    fn malformed_and_wrong_version_files_are_ignored_and_a_later_write_replaces_them() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(FILE);
        fs::write(&path, b"not json").unwrap();
        assert!(load(directory.path(), "work", "opencode", "one").is_none());
        fs::write(&path, br#"{"version":999,"entries":{}}"#).unwrap();
        assert!(load(directory.path(), "work", "opencode", "one").is_none());
        let entry = Entry {
            instance_id: "work".into(),
            driver: "opencode".into(),
            identity_fingerprint: "one".into(),
            version: None,
            checked_at: "2026-08-13T00:00:00Z".into(),
            models: vec![model("openai/first")],
        };
        store(directory.path(), entry.clone()).unwrap();
        store(
            directory.path(),
            Entry {
                models: vec![model("openai/second")],
                ..entry
            },
        )
        .unwrap();
        assert_eq!(
            load(directory.path(), "work", "opencode", "one")
                .unwrap()
                .models,
            vec![model("openai/second")]
        );
    }
}
