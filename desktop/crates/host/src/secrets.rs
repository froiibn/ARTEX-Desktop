use artex_core::{Error, Result};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::Mutex};
/// OS credentials with a visible memory-only fallback when Linux Secret Service
/// is absent/locked. Plaintext is never written to a fallback file.
#[derive(Default)]
pub struct SecretVault {
    session: Mutex<HashMap<String, String>>,
    // Serialize check/write/delete and retain only fingerprints for immutable
    // command inputs, independent of an OS backend's transient lookup state.
    commands: Mutex<HashMap<String, [u8; 32]>>,
}
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
fn credential_error(stage: &str, code: &str, message: &str, cause: keyring::Error) -> Error {
    // Never format the backend's arbitrary attributes, credential objects, or
    // BadEncoding bytes: diagnostic metadata must not leak secret material.
    let category = match &cause {
        keyring::Error::PlatformFailure(_) => "platform_failure",
        keyring::Error::NoStorageAccess(_) => "no_storage_access",
        keyring::Error::NoEntry => "no_entry",
        keyring::Error::BadEncoding(_) => "bad_encoding",
        keyring::Error::TooLong(_, _) => "too_long",
        keyring::Error::Invalid(_, _) => "invalid",
        keyring::Error::Ambiguous(_) => "ambiguous",
        _ => "unknown",
    };
    let mut error = Error::new(code, message);
    error.details = json!({"stage":stage,"backend_category":category});
    #[cfg(target_os = "windows")]
    if let Some(native) = std::error::Error::source(&cause)
        .and_then(|source| source.downcast_ref::<keyring::windows::Error>())
    {
        error.details["native_code"] = json!(native.0);
    }
    error
}
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
fn confirm_password(
    expected: &str,
    retry_missing_after_write: bool,
    mut read: impl FnMut() -> keyring::Result<String>,
    mut wait: impl FnMut(std::time::Duration),
) -> Result<()> {
    // A successful Windows write has been observed to return NoEntry on the
    // immediate confirmation read.
    // Only that missing-entry observation receives bounded read-only retries;
    // permission/encoding/storage failures and changed values stop immediately.
    const BACKOFF_MS: [u64; 4] = [10, 20, 40, 60]; // total requested wait: 130ms
    let mut retries = 0;
    loop {
        match read() {
            Ok(value) if value == expected => return Ok(()),
            Ok(_) => {
                return Err(Error::new(
                    "VERSION_CONFLICT",
                    "OS credential changed during write confirmation",
                ));
            }
            Err(keyring::Error::NoEntry)
                if retry_missing_after_write && retries < BACKOFF_MS.len() =>
            {
                wait(std::time::Duration::from_millis(BACKOFF_MS[retries]));
                retries += 1;
            }
            Err(cause) => {
                return Err(credential_error(
                    "confirm_write",
                    "IO_FAILURE",
                    "OS credential write could not be confirmed",
                    cause,
                ));
            }
        }
    }
}
impl SecretVault {
    pub fn set(&self, command: &str, field: &str, value: &str) -> Result<Value> {
        if command.is_empty() || field.is_empty() || value.is_empty() || value.len() > 65536 {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Bounded secret command, field, value required",
            ));
        }
        let id = format!(
            "secret_{}",
            &hex::encode(Sha256::digest(format!("{command}\0{field}").as_bytes()))[..32]
        );
        let fingerprint: [u8; 32] = Sha256::digest(value.as_bytes()).into();
        let mut commands = self.commands.lock().unwrap();
        if commands.get(&id).is_some_and(|old| old != &fingerprint) {
            return Err(Error::new(
                "VERSION_CONFLICT",
                "Secret command reused with changed value",
            ));
        }
        if let Some(old) = self.session.lock().unwrap().get(&id) {
            if old != value {
                return Err(Error::new(
                    "VERSION_CONFLICT",
                    "Secret command reused with changed value",
                ));
            }
            commands.insert(id.clone(), fingerprint);
            return Ok(json!({"secret_ref":id,"hint":"session only; re-enter after restart"}));
        }
        #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
        {
            let persisted = (|| {
                let entry = keyring::Entry::new("ARTEX", &id).map_err(|cause| {
                    credential_error(
                        "create_entry",
                        "FEATURE_DISABLED",
                        "OS credential store unavailable",
                        cause,
                    )
                })?;
                let wrote = match entry.get_password() {
                    Ok(old) => {
                        if old != value {
                            return Err(Error::new(
                                "VERSION_CONFLICT",
                                "Secret command reused with changed value",
                            ));
                        }
                        commands.insert(id.clone(), fingerprint);
                        false
                    }
                    Err(keyring::Error::NoEntry) => {
                        // Failed confirmation does not prove a successful
                        // write had no effect. Pin its input before writing so
                        // a retry cannot replace it with another credential.
                        commands.insert(id.clone(), fingerprint);
                        entry.set_password(value).map_err(|cause| {
                            credential_error(
                                "write",
                                "FEATURE_DISABLED",
                                "OS credential store rejected secret",
                                cause,
                            )
                        })?;
                        true
                    }
                    Err(cause) => {
                        return Err(credential_error(
                            "read_before_write",
                            "FEATURE_DISABLED",
                            "OS credential store unavailable",
                            cause,
                        ));
                    }
                };
                confirm_password(
                    value,
                    cfg!(target_os = "windows") && wrote,
                    || entry.get_password(),
                    std::thread::sleep,
                )?;
                Ok(json!({"secret_ref":id,"hint":"configured"}))
            })();
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            {
                if persisted.is_ok() {
                    commands.insert(id.clone(), fingerprint);
                }
                return persisted;
            }
            #[cfg(target_os = "linux")]
            match persisted {
                Ok(value) => {
                    commands.insert(id.clone(), fingerprint);
                    return Ok(value);
                }
                Err(error) if error.code != "FEATURE_DISABLED" => return Err(error),
                Err(_) => {}
            }
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            self.session
                .lock()
                .unwrap()
                .insert(id.clone(), value.into());
            commands.insert(id.clone(), fingerprint);
            Ok(json!({"secret_ref":id,"hint":"session only; re-enter after restart"}))
        }
    }
    /// Only private engine IPC may resolve a reference.
    pub fn resolve(&self, id: &str) -> Result<String> {
        if let Some(value) = self.session.lock().unwrap().get(id) {
            return Ok(value.clone());
        }
        #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
        {
            keyring::Entry::new("ARTEX", id)
                .and_then(|e| e.get_password())
                .map_err(|_| {
                    Error::new(
                        "FEATURE_DISABLED",
                        "Secret unavailable; re-enter if it was session only",
                    )
                })
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            Err(Error::new(
                "FEATURE_DISABLED",
                "Session secret must be entered again",
            ))
        }
    }
    pub fn persistent(reference: &Value) -> bool {
        reference["hint"] == "configured"
    }
    pub fn delete(&self, id: &str) -> Result<Value> {
        let _commands = self.commands.lock().unwrap();
        let session = self.session.lock().unwrap().remove(id).is_some();
        #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
        {
            if !session {
                let entry = keyring::Entry::new("ARTEX", id).map_err(|_| {
                    Error::new("FEATURE_DISABLED", "OS credential store unavailable")
                })?;
                match entry.delete_credential() {
                    Ok(()) | Err(keyring::Error::NoEntry) => {}
                    Err(_) => {
                        return Err(Error::new("FEATURE_DISABLED", "Cannot delete credential"));
                    }
                }
            }
        }
        let _ = session;
        Ok(json!({"deleted":true}))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    #[test]
    fn successful_write_confirmation_tolerates_only_bounded_temporary_missing_reads() {
        let mut reads = 0;
        let mut elapsed = std::time::Duration::ZERO;
        confirm_password(
            "fixture",
            true,
            || {
                reads += 1;
                if reads <= 2 {
                    Err(keyring::Error::NoEntry)
                } else {
                    Ok("fixture".into())
                }
            },
            |delay| elapsed += delay,
        )
        .unwrap();
        assert_eq!(reads, 3);
        assert!(
            elapsed > std::time::Duration::ZERO && elapsed <= std::time::Duration::from_millis(150)
        );
        let mut reads = 0;
        let mut elapsed = std::time::Duration::ZERO;
        let error = confirm_password(
            "fixture",
            true,
            || {
                reads += 1;
                Err(keyring::Error::NoEntry)
            },
            |delay| elapsed += delay,
        )
        .unwrap_err();
        assert_eq!(error.code, "IO_FAILURE");
        assert_eq!(error.details["backend_category"], "no_entry");
        assert!(reads <= 5 && elapsed <= std::time::Duration::from_millis(150));
        assert!(elapsed > std::time::Duration::ZERO);
    }
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    #[test]
    fn confirmation_does_not_retry_other_errors_mismatch_or_reads_without_a_successful_write() {
        for cause in [
            keyring::Error::BadEncoding(b"private-fixture-secret".to_vec()),
            keyring::Error::PlatformFailure(Box::new(std::io::Error::other("fixture backend"))),
            keyring::Error::NoStorageAccess(Box::new(std::io::Error::other(
                "fixture locked store",
            ))),
        ] {
            let mut next = Some(cause);
            let mut reads = 0;
            let error = confirm_password(
                "fixture",
                true,
                || {
                    reads += 1;
                    Err(next
                        .take()
                        .expect("non-missing backend errors must not be retried"))
                },
                |_| panic!("non-missing backend error must not wait"),
            )
            .unwrap_err();
            assert_eq!(reads, 1);
            assert_eq!(error.code, "IO_FAILURE");
            assert!(
                !serde_json::to_string(&error)
                    .unwrap()
                    .contains("private-fixture-secret")
            );
        }
        let error = confirm_password(
            "fixture",
            true,
            || Ok("changed-private-value".into()),
            |_| panic!("mismatch must not wait"),
        )
        .unwrap_err();
        assert_eq!(error.code, "VERSION_CONFLICT");
        assert!(!error.to_string().contains("changed-private-value"));
        let mut reads = 0;
        let error = confirm_password(
            "fixture",
            false,
            || {
                reads += 1;
                Err(keyring::Error::NoEntry)
            },
            |_| panic!("no successful new write means no retry"),
        )
        .unwrap_err();
        assert_eq!(reads, 1);
        assert_eq!(error.code, "IO_FAILURE");
    }
    #[test]
    fn native_credentials_are_deduplicated_and_removed() {
        let vault = SecretVault::default();
        let key = uuid::Uuid::new_v4().to_string();
        let secret = vault
            .set(&key, "fixture", "temporary-fixture-value")
            .unwrap();
        assert_eq!(
            vault
                .resolve(secret["secret_ref"].as_str().unwrap())
                .unwrap(),
            "temporary-fixture-value"
        );
        assert_eq!(
            vault
                .set(&key, "fixture", "temporary-fixture-value")
                .unwrap(),
            secret
        );
        assert_eq!(
            vault.set(&key, "fixture", "changed").unwrap_err().code,
            "VERSION_CONFLICT"
        );
        vault
            .delete(secret["secret_ref"].as_str().unwrap())
            .unwrap();
        assert!(
            vault
                .resolve(secret["secret_ref"].as_str().unwrap())
                .is_err()
        );
    }
    #[test]
    fn concurrent_native_secret_command_cannot_replace_confirmed_input() {
        use std::sync::{Arc, Barrier};
        let vault = Arc::new(SecretVault::default());
        let command = uuid::Uuid::new_v4().to_string();
        let barrier = Arc::new(Barrier::new(2));
        let workers: Vec<_> = ["first-temporary-value", "second-temporary-value"]
            .into_iter()
            .map(|value| {
                let vault = vault.clone();
                let command = command.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    (value, vault.set(&command, "concurrent-fixture", value))
                })
            })
            .collect();
        let outcomes: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        let diagnostic: Vec<_> = outcomes
            .iter()
            .enumerate()
            .map(|(index, (_, result))| match result {
                Ok(_) => format!("worker {index}: confirmed"),
                Err(error) => format!(
                    "worker {index}: {} {} {}",
                    error.code, error.message, error.details
                ),
            })
            .collect();
        assert_eq!(
            outcomes.iter().filter(|(_, result)| result.is_ok()).count(),
            1,
            "OS credential outcomes: {diagnostic:?}"
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|(_, result)| result
                    .as_ref()
                    .is_err_and(|error| error.code == "VERSION_CONFLICT"))
                .count(),
            1,
            "OS credential outcomes: {diagnostic:?}"
        );
        let (value, result) = outcomes.iter().find(|(_, result)| result.is_ok()).unwrap();
        let reference = result.as_ref().unwrap()["secret_ref"].as_str().unwrap();
        assert_eq!(vault.resolve(reference).unwrap(), *value);
        vault.delete(reference).unwrap();
        assert!(vault.resolve(reference).is_err());
    }
}
