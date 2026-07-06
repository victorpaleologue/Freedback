//! The account key: a self-signed P-256 identity persisted as PKCS#8 PEM.
//!
//! **The key IS the account** (INVARIANT 4a): it is minted locally on first
//! use — no signup — and the same key on another device is the same account.
//! Export/import moves the PEM; losing it means losing the ability to edit or
//! erase what it signed (the server only obeys the creating key, ADR 0021).

use std::path::{Path, PathBuf};

use freedback_protocol::Identity;
use thiserror::Error;

/// Identity storage errors.
#[derive(Debug, Error)]
pub enum IdentityError {
    /// No key file exists yet (and the operation refuses to mint one — e.g.
    /// erasure, where a fresh key could never own the annotation).
    #[error("no account key at {0} — import the key that signed this feedback")]
    Missing(PathBuf),
    /// The provided PEM does not parse as a PKCS#8 P-256 private key.
    #[error("invalid key PEM: {0}")]
    InvalidPem(String),
    #[error("io: {0}")]
    Io(String),
}

/// Manages the key file at a fixed path.
pub struct IdentityKeeper {
    path: PathBuf,
}

impl IdentityKeeper {
    /// A keeper for the key file at `path` (usually `<data_dir>/identity.pem`).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Where the key lives.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether a key has been minted/imported already.
    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// Load the key, or mint-and-persist a fresh one on first use.
    pub fn load_or_create(&self) -> Result<Identity, IdentityError> {
        match self.load() {
            Ok(id) => Ok(id),
            Err(IdentityError::Missing(_)) => {
                let id = Identity::generate();
                let pem = id
                    .to_pkcs8_pem()
                    .map_err(|e| IdentityError::InvalidPem(e.to_string()))?;
                self.persist(&pem)?;
                Ok(id)
            }
            Err(e) => Err(e),
        }
    }

    /// Load the key; [`IdentityError::Missing`] if none exists.
    pub fn load(&self) -> Result<Identity, IdentityError> {
        let pem = match std::fs::read_to_string(&self.path) {
            Ok(pem) => pem,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(IdentityError::Missing(self.path.clone()))
            }
            Err(e) => return Err(IdentityError::Io(e.to_string())),
        };
        Identity::from_pkcs8_pem(&pem).map_err(|e| IdentityError::InvalidPem(e.to_string()))
    }

    /// Validate `pem` and persist it, REPLACING any existing key. Returns the
    /// imported identity. Garbage never overwrites a working key.
    pub fn import_pem(&self, pem: &str) -> Result<Identity, IdentityError> {
        let id = Identity::from_pkcs8_pem(pem.trim())
            .map_err(|e| IdentityError::InvalidPem(e.to_string()))?;
        // Persist the canonical re-encoding, not the raw input (normalizes
        // line endings / stray whitespace).
        let canonical = id
            .to_pkcs8_pem()
            .map_err(|e| IdentityError::InvalidPem(e.to_string()))?;
        self.persist(&canonical)?;
        Ok(id)
    }

    /// Write the PEM with owner-only permissions (it is the account secret).
    fn persist(&self, pem: &str) -> Result<(), IdentityError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| IdentityError::Io(e.to_string()))?;
        }
        std::fs::write(&self.path, pem).map_err(|e| IdentityError::Io(e.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| IdentityError::Io(e.to_string()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_reports_missing() {
        let dir = tempfile::tempdir().unwrap();
        let keeper = IdentityKeeper::new(dir.path().join("identity.pem"));
        assert!(!keeper.exists());
        assert!(matches!(keeper.load(), Err(IdentityError::Missing(_))));
    }

    #[test]
    fn mint_once_then_reuse() {
        let dir = tempfile::tempdir().unwrap();
        let keeper = IdentityKeeper::new(dir.path().join("identity.pem"));
        let first = keeper.load_or_create().unwrap();
        assert!(keeper.exists());
        let second = keeper.load_or_create().unwrap();
        assert_eq!(
            first.issuer_id().unwrap(),
            second.issuer_id().unwrap(),
            "second use loads the SAME key"
        );
    }

    #[test]
    fn import_garbage_is_a_typed_error_and_preserves_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let keeper = IdentityKeeper::new(dir.path().join("identity.pem"));
        let original = keeper.load_or_create().unwrap();
        let err = match keeper.import_pem("not a pem at all") {
            Err(e) => e,
            Ok(_) => panic!("garbage PEM must not import"),
        };
        assert!(matches!(err, IdentityError::InvalidPem(_)));
        assert_eq!(
            keeper.load().unwrap().issuer_id().unwrap(),
            original.issuer_id().unwrap(),
            "a failed import never clobbers the existing key"
        );
    }

    #[test]
    fn import_replaces_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let keeper = IdentityKeeper::new(dir.path().join("identity.pem"));
        keeper.load_or_create().unwrap();

        let other = Identity::generate();
        let imported = keeper.import_pem(&other.to_pkcs8_pem().unwrap()).unwrap();
        assert_eq!(
            imported.issuer_id().unwrap(),
            other.issuer_id().unwrap(),
            "import returns the imported identity"
        );
        assert_eq!(
            keeper.load().unwrap().issuer_id().unwrap(),
            other.issuer_id().unwrap(),
            "and persists it"
        );
    }
}
