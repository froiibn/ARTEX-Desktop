#![forbid(unsafe_code)]
//! Offline migration only: normal desktop execution requires no legacy database service.
use artex_core::{Error, Result};
use artex_host::SecretVault;
use artex_importer::{LegacyOptions, PendingSecrets, SecretOutcome, SecretSink};
use std::{collections::BTreeMap, path::PathBuf};
const HELP: &str = "artex-migrate --destination NEW_ABSOLUTE_DIRECTORY (--json EXPORT.json | --archive ARCHIVE | --postgres-env ENV_NAME --source-instance STABLE_NAME) [--traffic-sqlite FILE --traffic-blobs DIRECTORY] [--pending-secrets]\n\nSource files/databases are read-only. Destination must not exist. Failed imports resume in a deterministic staging directory. Credentials use the ARTEX OS vault; --pending-secrets discards plaintext and requires re-entry. PostgreSQL uses a read-only repeatable-read snapshot without TLS: use only a local database or a secured tunnel. Never pass a DSN on the command line. JSON/archive source identity is preserved. Close ARTEX before opening the imported workspace. Report: DESTINATION/backups/migration-report.json.";
#[derive(Debug)]
struct Options {
    destination: PathBuf,
    values: BTreeMap<String, String>,
    pending: bool,
}
fn options(args: impl IntoIterator<Item = String>) -> Result<Option<Options>> {
    let mut args = args.into_iter();
    let mut values = BTreeMap::new();
    let mut pending = false;
    while let Some(key) = args.next() {
        if key == "--help" || key == "-h" {
            return Ok(None);
        }
        if key == "--pending-secrets" {
            if pending {
                return Err(Error::new("INVALID_PARAMS", "Repeated option"));
            }
            pending = true;
            continue;
        }
        if ![
            "--destination",
            "--json",
            "--archive",
            "--postgres-env",
            "--source-instance",
            "--traffic-sqlite",
            "--traffic-blobs",
        ]
        .contains(&key.as_str())
        {
            return Err(Error::new("INVALID_PARAMS", "Unknown migration option"));
        }
        let value = args
            .next()
            .filter(|v| !v.is_empty() && !v.starts_with("--"))
            .ok_or_else(|| Error::new("INVALID_PARAMS", "Missing option value"))?;
        if values.insert(key, value).is_some() {
            return Err(Error::new("INVALID_PARAMS", "Repeated option"));
        }
    }
    let destination = PathBuf::from(
        values
            .get("--destination")
            .ok_or_else(|| Error::new("INVALID_PARAMS", "Destination required"))?,
    );
    if !destination.is_absolute() {
        return Err(Error::new("INVALID_PARAMS", "Destination must be absolute"));
    }
    if ["--json", "--archive", "--postgres-env"]
        .iter()
        .filter(|k| values.contains_key(**k))
        .count()
        != 1
    {
        return Err(Error::new(
            "INVALID_PARAMS",
            "Exactly one primary source required",
        ));
    }
    if values.contains_key("--postgres-env") && !values.contains_key("--source-instance") {
        return Err(Error::new(
            "INVALID_PARAMS",
            "Stable PostgreSQL source identity required",
        ));
    }
    if values.contains_key("--traffic-sqlite") != values.contains_key("--traffic-blobs") {
        return Err(Error::new(
            "INVALID_PARAMS",
            "Traffic database and blob directory must be supplied together",
        ));
    }
    Ok(Some(Options {
        destination,
        values,
        pending,
    }))
}
#[derive(Default)]
struct NativeSecrets(SecretVault);
impl SecretSink for NativeSecrets {
    fn import_secret(&mut self, key: &str, plain: &str) -> Result<SecretOutcome> {
        let reference = self.0.set(key, "legacy-migration", plain)?;
        let available = SecretVault::persistent(&reference);
        Ok(SecretOutcome {
            reference,
            available,
        })
    }
    fn reference_available(&mut self, reference: &serde_json::Value) -> Result<bool> {
        Ok(reference["secret_ref"]
            .as_str()
            .is_some_and(|id| self.0.resolve(id).is_ok()))
    }
}
fn execute(o: Options) -> Result<()> {
    if o.destination.exists() {
        return Err(Error::new(
            "VERSION_CONFLICT",
            "Migration destination already exists",
        ));
    }
    let identity = o
        .values
        .get("--source-instance")
        .map(String::as_str)
        .unwrap_or("legacy-archive");
    let mut source = if let Some(file) = o.values.get("--json") {
        artex_importer::read_json_export(&PathBuf::from(file))?
    } else if let Some(file) = o.values.get("--archive") {
        artex_importer::read_archive(&PathBuf::from(file), identity)?
    } else {
        let name = &o.values["--postgres-env"];
        let dsn = std::env::var(name).map_err(|_| {
            Error::new(
                "INVALID_PARAMS",
                "PostgreSQL environment variable unavailable",
            )
        })?;
        artex_importer::read_postgres(&dsn, identity)?
    };
    if source.source_instance.is_empty() {
        return Err(Error::new(
            "INVALID_PARAMS",
            "Export source identity is empty",
        ));
    }
    if let Some(file) = o.values.get("--traffic-sqlite") {
        let traffic = artex_importer::read_traffic_sqlite(
            &PathBuf::from(file),
            &PathBuf::from(&o.values["--traffic-blobs"]),
            &source.source_instance,
        )?;
        source.merge(traffic)?;
    }
    let mut sink: Box<dyn SecretSink> = if o.pending {
        Box::new(PendingSecrets)
    } else {
        Box::<NativeSecrets>::default()
    };
    let report = artex_importer::migrate_new(
        &o.destination,
        &source,
        sink.as_mut(),
        LegacyOptions::default(),
    )?;
    // No source rows, source DSN, extension values, or credential values on stdout.
    println!(
        "{}",
        serde_json::json!({"complete":report.complete,"counts":report.counts,"pending_credentials":report.pending_secret_refs.len(),"report":"backups/migration-report.json"})
    );
    Ok(())
}
fn main() {
    let result = options(std::env::args().skip(1)).and_then(|o| match o {
        Some(o) => execute(o),
        None => {
            println!("{HELP}");
            Ok(())
        }
    });
    if let Err(error) = result {
        eprintln!(
            "Migration failed: {}. Source preserved; retain staging directory for retry.",
            error.code
        );
        std::process::exit(1);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn options_require_a_single_source_and_absolute_destination() {
        assert!(
            options(["--destination", "relative", "--json", "export"].map(str::to_owned)).is_err()
        );
        assert!(options(["--help"].map(str::to_owned)).unwrap().is_none());
    }
    #[test]
    fn migration_cli_publishes_verified_workspace_and_preserves_source() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("export.json");
        let destination = dir.path().join("workspace");
        let bytes=serde_json::to_vec(&serde_json::json!({"source_instance":"cli-fixture","format_version":1,"schema":{},"tables":{"companies":[{"id":9007199254740993u64,"name":"한국어 회사","nkey":"한국어 회사"}]},"blobs":{}})).unwrap();
        std::fs::write(&source, &bytes).unwrap();
        let args = vec![
            "--destination".into(),
            destination.to_string_lossy().into_owned(),
            "--json".into(),
            source.to_string_lossy().into_owned(),
            "--pending-secrets".into(),
        ];
        execute(options(args.clone()).unwrap().unwrap()).unwrap();
        assert_eq!(std::fs::read(&source).unwrap(), bytes);
        let store = artex_store::Store::open(&destination).unwrap();
        let snapshot = store.snapshot();
        assert_eq!(snapshot.records("companies").len(), 1);
        assert_eq!(
            snapshot.records("companies")[0].header.legacy_key,
            "9007199254740993"
        );
        assert!(destination.join("backups/migration-report.json").is_file());
        drop(store);
        assert_eq!(
            execute(options(args).unwrap().unwrap()).unwrap_err().code,
            "VERSION_CONFLICT"
        );
    }
}
