use std::{
    error::Error,
    time::{SystemTime, UNIX_EPOCH},
};

use codex_o_lib::secrets::{
    ProviderSecretId, SecretStore, SecretStoreErrorCode, SecretValue, SystemSecretStore,
};

fn main() -> Result<(), Box<dyn Error>> {
    let store = SystemSecretStore::new();
    let provider_id = ProviderSecretId::new(format!(
        "t0-probe-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ))?;
    let cleanup = CleanupGuard {
        store: &store,
        provider_id: &provider_id,
    };

    let result = run_probe(&store, &provider_id);
    drop(cleanup);
    verify_cleanup(&store, &provider_id)?;
    result?;

    println!("keyring probe: set=get=overwrite=delete=not_found ok");
    Ok(())
}

fn run_probe(
    store: &SystemSecretStore,
    provider_id: &ProviderSecretId,
) -> Result<(), Box<dyn Error>> {
    let _ = store.delete(provider_id);
    store.set(provider_id, SecretValue::new("codex-o-probe-initial"))?;
    ensure_matches(
        store.get(provider_id)?,
        SecretValue::new("codex-o-probe-initial"),
    )?;

    store.set(provider_id, SecretValue::new("codex-o-probe-overwrite"))?;
    ensure_matches(
        store.get(provider_id)?,
        SecretValue::new("codex-o-probe-overwrite"),
    )?;

    store.delete(provider_id)?;
    match store.get(provider_id) {
        Err(error) if error.code == SecretStoreErrorCode::NotFound => {}
        _ => return Err("delete verification failed".into()),
    }

    Ok(())
}

fn verify_cleanup(
    store: &SystemSecretStore,
    provider_id: &ProviderSecretId,
) -> Result<(), Box<dyn Error>> {
    match store.get(provider_id) {
        Err(error) if error.code == SecretStoreErrorCode::NotFound => Ok(()),
        _ => Err("cleanup verification failed".into()),
    }
}

fn ensure_matches(actual: SecretValue, expected: SecretValue) -> Result<(), Box<dyn Error>> {
    if actual == expected {
        Ok(())
    } else {
        Err("credential round-trip verification failed".into())
    }
}

struct CleanupGuard<'a> {
    store: &'a SystemSecretStore,
    provider_id: &'a ProviderSecretId,
}

impl Drop for CleanupGuard<'_> {
    fn drop(&mut self) {
        self.store.cleanup(self.provider_id);
    }
}
