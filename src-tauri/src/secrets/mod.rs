use std::fmt;

use keyring::Entry;

pub const KEYRING_SERVICE: &str = "com.zreo.codexo";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderSecretId(String);

impl ProviderSecretId {
    pub fn new(value: impl Into<String>) -> Result<Self, SecretStoreError> {
        let value = value.into();

        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(SecretStoreError::new(
                SecretStoreErrorCode::InvalidIdentifier,
            ));
        }

        Ok(Self(value))
    }

    pub fn account_name(&self) -> String {
        format!("ai-provider:{}", self.0)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SecretValue(String);

impl SecretValue {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretValue([redacted])")
    }
}

impl fmt::Display for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[redacted]")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretStoreErrorCode {
    AccessDenied,
    InvalidIdentifier,
    NotFound,
    Unavailable,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SecretStoreError {
    pub code: SecretStoreErrorCode,
}

impl SecretStoreError {
    const fn new(code: SecretStoreErrorCode) -> Self {
        Self { code }
    }
}

impl fmt::Debug for SecretStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "SecretStoreError({})", self)
    }
}

impl fmt::Display for SecretStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = match self.code {
            SecretStoreErrorCode::AccessDenied => "access_denied",
            SecretStoreErrorCode::InvalidIdentifier => "invalid_identifier",
            SecretStoreErrorCode::NotFound => "not_found",
            SecretStoreErrorCode::Unavailable => "unavailable",
        };
        formatter.write_str(code)
    }
}

impl std::error::Error for SecretStoreError {}

pub trait SecretStore {
    fn set(
        &self,
        provider_id: &ProviderSecretId,
        secret: SecretValue,
    ) -> Result<(), SecretStoreError>;
    fn get(&self, provider_id: &ProviderSecretId) -> Result<SecretValue, SecretStoreError>;
    fn delete(&self, provider_id: &ProviderSecretId) -> Result<(), SecretStoreError>;

    fn exists(&self, provider_id: &ProviderSecretId) -> Result<bool, SecretStoreError> {
        match self.get(provider_id) {
            Ok(_) => Ok(true),
            Err(SecretStoreError {
                code: SecretStoreErrorCode::NotFound,
            }) => Ok(false),
            Err(error) => Err(error),
        }
    }
}

pub fn cleanup_secret(store: &impl SecretStore, provider_id: &ProviderSecretId) {
    let _ = store.delete(provider_id);
}

pub struct SystemSecretStore {
    backend: KeyringCredentialBackend,
}

impl SystemSecretStore {
    pub fn new() -> Self {
        Self {
            backend: KeyringCredentialBackend,
        }
    }

    /// Removes a probe credential even if the public delete path has failed.
    pub fn cleanup(&self, provider_id: &ProviderSecretId) {
        let _ = self.backend.delete(&provider_id.account_name());
    }
}

impl Default for SystemSecretStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretStore for SystemSecretStore {
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

trait CredentialBackend {
    fn set(&self, account: &str, secret: &str) -> Result<(), SecretStoreError>;
    fn get(&self, account: &str) -> Result<String, SecretStoreError>;
    fn delete(&self, account: &str) -> Result<(), SecretStoreError>;
}

struct KeyringCredentialBackend;

impl KeyringCredentialBackend {
    fn entry(&self, account: &str) -> Result<Entry, SecretStoreError> {
        Entry::new(KEYRING_SERVICE, account).map_err(map_keyring_error)
    }
}

impl CredentialBackend for KeyringCredentialBackend {
    fn set(&self, account: &str, secret: &str) -> Result<(), SecretStoreError> {
        self.entry(account)?
            .set_password(secret)
            .map_err(map_keyring_error)
    }

    fn get(&self, account: &str) -> Result<String, SecretStoreError> {
        self.entry(account)?
            .get_password()
            .map_err(map_keyring_error)
    }

    fn delete(&self, account: &str) -> Result<(), SecretStoreError> {
        self.entry(account)?
            .delete_credential()
            .map_err(map_keyring_error)
    }
}

fn map_keyring_error(error: keyring::Error) -> SecretStoreError {
    let code = match error {
        keyring::Error::NoEntry => SecretStoreErrorCode::NotFound,
        keyring::Error::NoStorageAccess(_) => SecretStoreErrorCode::AccessDenied,
        keyring::Error::Invalid(_, _) | keyring::Error::TooLong(_, _) => {
            SecretStoreErrorCode::InvalidIdentifier
        }
        keyring::Error::PlatformFailure(_)
        | keyring::Error::NoDefaultStore
        | keyring::Error::NotSupportedByStore(_)
        | keyring::Error::BadEncoding(_)
        | keyring::Error::BadDataFormat(_, _)
        | keyring::Error::BadStoreFormat(_)
        | keyring::Error::Ambiguous(_) => SecretStoreErrorCode::Unavailable,
        _ => SecretStoreErrorCode::Unavailable,
    };

    SecretStoreError::new(code)
}

#[cfg(test)]
mod tests;
