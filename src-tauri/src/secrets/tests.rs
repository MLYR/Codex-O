use std::{cell::RefCell, collections::HashMap, io};

use super::{
    cleanup_secret, map_keyring_error, CredentialBackend, ProviderSecretId, SecretStore,
    SecretStoreError, SecretStoreErrorCode, SecretValue,
};

#[test]
fn provider_secret_id_accepts_logical_provider_ids() {
    let identifier = ProviderSecretId::new("openai.default-1").unwrap();

    assert_eq!(identifier.account_name(), "ai-provider:openai.default-1");
}

#[test]
fn provider_secret_id_rejects_path_and_control_characters() {
    for value in [
        "",
        "../provider",
        "provider/name",
        "provider\\name",
        "line\nbreak",
    ] {
        let error = ProviderSecretId::new(value).unwrap_err();
        assert_eq!(error.code, SecretStoreErrorCode::InvalidIdentifier);
    }
}

#[test]
fn provider_secret_id_rejects_values_longer_than_the_keyring_boundary() {
    let error = ProviderSecretId::new("a".repeat(129)).unwrap_err();

    assert_eq!(error.code, SecretStoreErrorCode::InvalidIdentifier);
}

#[test]
fn secret_value_debug_and_display_are_always_redacted() {
    let value = SecretValue::new("fixture-value");

    assert_eq!(format!("{value:?}"), "SecretValue([redacted])");
    assert_eq!(value.to_string(), "[redacted]");
}

#[test]
fn keyring_not_found_is_mapped_without_backend_details() {
    let error = map_keyring_error(keyring::Error::NoEntry);

    assert_eq!(error.code, SecretStoreErrorCode::NotFound);
    assert_eq!(error.to_string(), "not_found");
}

#[test]
fn keyring_access_errors_do_not_expose_backend_messages() {
    let raw_error = keyring::Error::NoStorageAccess(Box::new(io::Error::other(
        "fixture-value service=com.zreo.codexo account=ai-provider:test-provider",
    )));
    let error = map_keyring_error(raw_error);

    assert_eq!(error.code, SecretStoreErrorCode::AccessDenied);
    assert_eq!(error.to_string(), "access_denied");
    assert_eq!(format!("{error:?}"), "SecretStoreError(access_denied)");
}

#[test]
fn isolated_backend_supports_set_get_exists_and_delete() {
    let store = FixtureSecretStore::default();
    let identifier = ProviderSecretId::new("test-provider").unwrap();

    store
        .set(&identifier, SecretValue::new("fixture-value"))
        .unwrap();
    assert!(store.exists(&identifier).unwrap());
    assert_eq!(
        store.get(&identifier).unwrap(),
        SecretValue::new("fixture-value")
    );

    store.delete(&identifier).unwrap();
    assert!(!store.exists(&identifier).unwrap());
}

#[test]
fn isolated_backend_returns_not_found_after_delete() {
    let store = FixtureSecretStore::default();
    let identifier = ProviderSecretId::new("test-provider").unwrap();

    let error = store.get(&identifier).unwrap_err();

    assert_eq!(error.code, SecretStoreErrorCode::NotFound);
}

#[test]
fn cleanup_removes_a_fixture_credential_after_a_failed_operation() {
    let store = FixtureSecretStore::default();
    let identifier = ProviderSecretId::new("test-provider").unwrap();
    store
        .set(&identifier, SecretValue::new("fixture-value"))
        .unwrap();

    let failed_operation: Result<(), SecretStoreError> = Err(SecretStoreError {
        code: SecretStoreErrorCode::Unavailable,
    });
    assert_eq!(
        failed_operation.unwrap_err().code,
        SecretStoreErrorCode::Unavailable
    );
    cleanup_secret(&store, &identifier);

    assert!(!store.exists(&identifier).unwrap());
}

#[derive(Default)]
struct FixtureSecretStore {
    backend: FixtureCredentialBackend,
}

impl SecretStore for FixtureSecretStore {
    fn set(
        &self,
        provider_id: &ProviderSecretId,
        secret: SecretValue,
    ) -> Result<(), SecretStoreError> {
        self.backend
            .set(&provider_id.account_name(), secret.expose())
    }

    fn get(&self, provider_id: &ProviderSecretId) -> Result<SecretValue, SecretStoreError> {
        self.backend
            .get(&provider_id.account_name())
            .map(SecretValue::new)
    }

    fn delete(&self, provider_id: &ProviderSecretId) -> Result<(), SecretStoreError> {
        self.backend.delete(&provider_id.account_name())
    }
}

#[derive(Default)]
struct FixtureCredentialBackend {
    values: RefCell<HashMap<String, String>>,
}

impl CredentialBackend for FixtureCredentialBackend {
    fn set(&self, account: &str, secret: &str) -> Result<(), SecretStoreError> {
        self.values
            .borrow_mut()
            .insert(account.to_owned(), secret.to_owned());
        Ok(())
    }

    fn get(&self, account: &str) -> Result<String, SecretStoreError> {
        self.values
            .borrow()
            .get(account)
            .cloned()
            .ok_or(SecretStoreError {
                code: SecretStoreErrorCode::NotFound,
            })
    }

    fn delete(&self, account: &str) -> Result<(), SecretStoreError> {
        self.values.borrow_mut().remove(account);
        Ok(())
    }
}
