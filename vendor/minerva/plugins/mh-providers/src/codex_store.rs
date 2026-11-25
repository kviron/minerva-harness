//! Chunk OAuth records below Windows Credential Manager's per-entry limit.
use super::*;
use rig_core::providers::chatgpt::auth::{FileTokenStore, StoreLock};
use serde::Deserialize;
type Future<'a, T> = rig_core::wasm_compat::WasmBoxedFuture<'a, Result<T, rig_core::error::BoxError>>;
const MAX_RECORD: usize = 128 * 1024;
const CHUNK_CHARS: usize = 512;
#[derive(Serialize, Deserialize)]
struct Manifest {
    generation: String,
    chunks: usize,
}
#[derive(Clone)]
pub(super) struct Store {
    credentials: Arc<dyn CredentialStore>,
    scope: CredentialScope,
    locks: FileTokenStore,
    name: String,
}
impl Store {
    pub fn new(path: PathBuf, scope: CredentialScope, credentials: Arc<dyn CredentialStore>) -> Self {
        let name = format!("minerva-chatgpt-oauth:{}:{}", scope.as_str(), path.display());
        Self { credentials, scope, locks: FileTokenStore::new(path), name }
    }
    fn manifest(value: &str) -> Result<Manifest, String> {
        let manifest: Manifest = serde_json::from_str(value).map_err(|_| "Invalid Codex credential manifest")?;
        if manifest.chunks == 0
            || manifest.chunks > 256
            || manifest.generation.len() > 80
            || !manifest.generation.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err("Invalid Codex credential manifest".into());
        }
        Ok(manifest)
    }
    fn chunk_name(&self, manifest: &Manifest, index: usize) -> String {
        format!("{}:{}:{index}", self.name, manifest.generation)
    }
    fn cleanup(&self, manifest: &Manifest) {
        for index in 0..manifest.chunks {
            let _ = self.credentials.delete(&self.scope, &self.chunk_name(manifest, index));
        }
    }
    pub fn read(&self) -> Result<Option<String>, String> {
        for _ in 0..3 {
            let Some(value) = self.credentials.get(&self.scope, &self.name)? else { return Ok(None) };
            let manifest = Self::manifest(&value)?;
            let record = self.read_chunks(&manifest)?;
            if self.credentials.get(&self.scope, &self.name)?.as_deref() != Some(value.as_str()) {
                continue;
            }
            return record.map(Some).ok_or_else(|| "Incomplete Codex credentials; sign in again".into());
        }
        Err("Codex authorization changed while reading; please try again".into())
    }
    fn read_chunks(&self, manifest: &Manifest) -> Result<Option<String>, String> {
        let mut record = String::new();
        for index in 0..manifest.chunks {
            let Some(chunk) = self.credentials.get(&self.scope, &self.chunk_name(manifest, index))? else {
                return Ok(None);
            };
            if chunk.chars().count() > CHUNK_CHARS || record.len() + chunk.len() > MAX_RECORD {
                return Err("Oversized Codex credentials".into());
            }
            record.push_str(&chunk);
        }
        Ok(Some(record))
    }
    fn write(&self, record: &str) -> Result<(), String> {
        if record.is_empty() || record.len() > MAX_RECORD {
            return Err("Invalid Codex credential size".into());
        }
        let previous =
            self.credentials.get(&self.scope, &self.name)?.map(|value| Self::manifest(&value)).transpose()?;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let generation = format!(
            "{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| "System clock unavailable")?
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let chars: Vec<_> = record.chars().collect();
        let manifest = Manifest { generation, chunks: chars.len().div_ceil(CHUNK_CHARS) };
        for (index, chars) in chars.chunks(CHUNK_CHARS).enumerate() {
            if self
                .credentials
                .put(&self.scope, &self.chunk_name(&manifest, index), &chars.iter().collect::<String>())
                .is_err()
            {
                self.cleanup(&manifest);
                return Err("Could not store Codex authorization".into());
            }
        }
        let pointer = serde_json::to_string(&manifest).map_err(|_| "Invalid Codex credential manifest")?;
        if let Err(error) = self.credentials.put(&self.scope, &self.name, &pointer) {
            self.cleanup(&manifest);
            return Err(error);
        }
        if let Some(previous) = previous {
            self.cleanup(&previous);
        }
        Ok(())
    }
    pub fn clear(&self) -> Result<(), String> {
        let manifest = self.credentials.get(&self.scope, &self.name)?.and_then(|value| Self::manifest(&value).ok());
        self.credentials.delete(&self.scope, &self.name)?;
        if let Some(manifest) = manifest {
            self.cleanup(&manifest);
        }
        Ok(())
    }
}
impl TokenStore for Store {
    fn load<'a>(&'a self, _: &'a str) -> Future<'a, Option<String>> {
        Box::pin(async { self.read().map_err(Into::into) })
    }
    fn save<'a>(&'a self, _: &'a str, record: &'a str) -> Future<'a, ()> {
        Box::pin(async move { self.write(record).map_err(Into::into) })
    }
    fn delete<'a>(&'a self, _: &'a str) -> Future<'a, ()> {
        Box::pin(async { self.clear().map_err(Into::into) })
    }
    fn lock<'a>(&'a self, key: &'a str) -> Future<'a, StoreLock> {
        self.locks.lock(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential_store::MemoryCredentialStore;

    type ReadHook = (String, Box<dyn FnOnce() + Send>);
    #[derive(Default)]
    struct TestStore {
        inner: MemoryCredentialStore,
        after_read: Mutex<Option<ReadHook>>,
        fail_after: Mutex<Option<usize>>,
        written: Mutex<Vec<String>>,
    }
    impl CredentialStore for TestStore {
        fn get(&self, scope: &CredentialScope, name: &str) -> Result<Option<String>, String> {
            let result = self.inner.get(scope, name);
            let hook = {
                let mut hook = self.after_read.lock().unwrap();
                if hook.as_ref().is_some_and(|(key, _)| key == name) {
                    hook.take()
                } else {
                    None
                }
            };
            if let Some((_, hook)) = hook {
                hook();
            }
            result
        }
        fn put(&self, scope: &CredentialScope, name: &str, secret: &str) -> Result<(), String> {
            if let Some(remaining) = self.fail_after.lock().unwrap().as_mut() {
                if *remaining == 0 {
                    return Err("Credential storage unavailable".into());
                }
                *remaining -= 1;
            }
            self.written.lock().unwrap().push(name.into());
            self.inner.put(scope, name, secret)
        }
        fn delete(&self, scope: &CredentialScope, name: &str) -> Result<(), String> {
            self.inner.delete(scope, name)
        }
    }

    #[test]
    fn concurrent_refresh_and_logout_retry_only_a_changed_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = Arc::new(TestStore::default());
        let store = Store::new(dir.path().join("locks"), CredentialScope::new("test").unwrap(), credentials.clone());
        store.write("old-record").unwrap();
        *credentials.after_read.lock().unwrap() = Some((
            store.name.clone(),
            Box::new({
                let store = store.clone();
                move || store.write("new-record").unwrap()
            }),
        ));
        assert_eq!(store.read().unwrap().as_deref(), Some("new-record"));
        *credentials.after_read.lock().unwrap() = Some((
            store.name.clone(),
            Box::new({
                let store = store.clone();
                move || store.clear().unwrap()
            }),
        ));
        assert!(store.read().unwrap().is_none());
        store.write("incomplete-record").unwrap();
        let manifest = Store::manifest(&credentials.inner.get(&store.scope, &store.name).unwrap().unwrap()).unwrap();
        credentials.inner.delete(&store.scope, &store.chunk_name(&manifest, 0)).unwrap();
        assert!(store.read().unwrap_err().contains("Incomplete"));
    }

    #[test]
    fn failed_chunk_write_keeps_previous_record_and_removes_partial_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = Arc::new(TestStore::default());
        let store = Store::new(dir.path().join("locks"), CredentialScope::new("test").unwrap(), credentials.clone());
        store.write("previous-record").unwrap();
        credentials.written.lock().unwrap().clear();
        *credentials.fail_after.lock().unwrap() = Some(1);
        assert!(store.write(&"x".repeat(CHUNK_CHARS * 3)).is_err());
        assert_eq!(store.read().unwrap().as_deref(), Some("previous-record"));
        let written = credentials.written.lock().unwrap();
        assert_eq!(written.len(), 1);
        assert!(credentials.inner.get(&store.scope, &written[0]).unwrap().is_none());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn large_record_roundtrips_through_windows_keyring() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(
            dir.path().join("disposable"),
            CredentialScope::new("test").unwrap(),
            Arc::new(crate::credential_store::LocalKeyring),
        );
        let record = format!("{{\"access_token\":\"{}\"}}", "fixture-opaque-token".repeat(800));
        let result = store.write(&record).and_then(|_| store.read());
        let cleanup = store.clear();
        assert_eq!(result.unwrap().as_deref(), Some(record.as_str()));
        cleanup.unwrap();
        assert!(store.read().unwrap().is_none());
    }
}
