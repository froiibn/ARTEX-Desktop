use crate::LegacyExport;
use artex_core::{Error, Result, data_model};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path};
const EXPORT_LIMIT: usize = 512 * 1024 * 1024;
pub(crate) fn checked_path(path: &Path) -> Result<()> {
    let mut current = std::path::PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        if matches!(component, std::path::Component::Prefix(_)) {
            continue;
        }
        if current.exists() {
            let metadata = fs::symlink_metadata(&current)?;
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(Error::new(
                        "PERMISSION_DENIED",
                        "Import paths may not contain reparse points",
                    ));
                }
            }
            if metadata.file_type().is_symlink() {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Import paths may not contain symbolic links",
                ));
            }
        }
    }
    Ok(())
}
fn err(message: &str) -> Error {
    Error::new("EXTERNAL_FAILURE", message)
}
pub fn read_json_export(path: &Path) -> Result<LegacyExport> {
    checked_path(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || metadata.len() > EXPORT_LIMIT as u64 {
        return Err(Error::new(
            "SIZE_LIMIT",
            "Legacy export is a link or exceeds size limit",
        ));
    }
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
pub fn read_postgres(dsn: &str, source_instance: &str) -> Result<LegacyExport> {
    let mut client = postgres::Client::connect(dsn, postgres::NoTls)
        .map_err(|_| err("PostgreSQL connection failed"))?;
    read_postgres_client(&mut client, source_instance)
}
/// Caller can supply an authenticated TLS PostgreSQL client. Every query uses
/// the same read-only repeatable-read transaction; no SQL dump is executed.
pub fn read_postgres_client(
    client: &mut postgres::Client,
    source_instance: &str,
) -> Result<LegacyExport> {
    let mut tx = client
        .build_transaction()
        .isolation_level(postgres::IsolationLevel::RepeatableRead)
        .read_only(true)
        .start()
        .map_err(|_| err("PostgreSQL snapshot failed"))?;
    let mut export = LegacyExport {
        source_instance: source_instance.into(),
        format_version: 1,
        ..Default::default()
    };
    let mut used = 0;
    for table in data_model()["tables"].as_array().unwrap() {
        if table["source"] == "traffic/traffic.go" {
            continue;
        }
        let name = table["name"].as_str().unwrap();
        let columns=tx.query("SELECT column_name FROM information_schema.columns WHERE table_schema=current_schema() AND table_name=$1 ORDER BY ordinal_position",&[&name]).map_err(|_|err("PostgreSQL schema inspection failed"))?.iter().map(|r|r.get::<_,String>(0)).collect::<Vec<_>>();
        if columns.is_empty() {
            continue;
        }
        let query = format!("SELECT row_to_json(t)::text FROM \"{name}\" t");
        let mut rows = Vec::new();
        let mut cursor = tx
            .query_raw(&query, std::iter::empty::<&str>())
            .map_err(|_| err("PostgreSQL table export failed"))?;
        while let Some(row) = postgres::fallible_iterator::FallibleIterator::next(&mut cursor)
            .map_err(|_| err("PostgreSQL row export failed"))?
        {
            let raw: String = row.get(0);
            used += raw.len();
            if used > EXPORT_LIMIT {
                return Err(Error::new(
                    "SIZE_LIMIT",
                    "PostgreSQL export exceeds configured memory boundary",
                ));
            }
            rows.push(serde_json::from_str::<Value>(&raw)?);
        }
        export.schema.insert(name.into(), columns);
        export.tables.insert(name.into(), rows);
    }
    tx.commit()
        .map_err(|_| err("PostgreSQL read snapshot completion failed"))?;
    Ok(export)
}
fn binary(bytes: Vec<u8>) -> Value {
    use base64::Engine;
    json!({"legacy_bytes":base64::engine::general_purpose::STANDARD.encode(bytes)})
}
fn valid_digest(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn load_legacy_blob(root: &Path, hash: &str) -> Result<Vec<u8>> {
    if !valid_digest(hash) {
        return Err(Error::new("INVALID_PARAMS", "Legacy blob hash invalid"));
    }
    for relative in [
        format!("_blobs/sha256/{}/{}.bin", &hash[..2], hash),
        format!("_blobs/sha256/{}/{}/{}.bin", &hash[..2], &hash[2..4], hash),
        format!("blobs/{hash}.bin"),
    ] {
        let path = root.join(relative);
        checked_path(&path)?;
        if path.is_file() {
            let meta = fs::symlink_metadata(&path)?;
            if meta.file_type().is_symlink() || meta.len() > EXPORT_LIMIT as u64 {
                return Err(Error::new("SIZE_LIMIT", "Legacy blob link or size invalid"));
            }
            let bytes = fs::read(path)?;
            use sha2::Digest;
            if hex::encode(sha2::Sha256::digest(&bytes)) != hash {
                return Err(Error::new("CORRUPT_STORE", "Legacy blob checksum mismatch"));
            }
            return Ok(bytes);
        }
    }
    Err(Error::new("NOT_FOUND", "Legacy blob file missing"))
}
pub fn read_traffic_sqlite(
    path: &Path,
    blob_root: &Path,
    source_instance: &str,
) -> Result<LegacyExport> {
    checked_path(path)?;
    checked_path(blob_root)?;
    let connection = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| err("Traffic SQLite open failed"))?;
    connection
        .execute_batch("PRAGMA query_only=ON; BEGIN DEFERRED TRANSACTION;")
        .map_err(|_| err("Traffic SQLite snapshot failed"))?;
    let mut export = LegacyExport {
        source_instance: source_instance.into(),
        format_version: 1,
        ..Default::default()
    };
    let mut used = 0;
    for name in ["exchanges", "exchange_bodies", "blob_refs"] {
        let mut columns = Vec::new();
        {
            let mut statement = connection
                .prepare(&format!("PRAGMA table_info({name})"))
                .map_err(|_| err("Traffic schema inspection failed"))?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(1))
                .map_err(|_| err("Traffic schema inspection failed"))?;
            for c in rows {
                columns.push(c.map_err(|_| err("Traffic schema inspection failed"))?);
            }
        }
        if columns.is_empty() {
            continue;
        }
        let mut stmt = connection
            .prepare(&format!("SELECT * FROM {name}"))
            .map_err(|_| err("Traffic table export failed"))?;
        let mut result = stmt
            .query([])
            .map_err(|_| err("Traffic row export failed"))?;
        let mut rows = Vec::new();
        while let Some(row) = result
            .next()
            .map_err(|_| err("Traffic row export failed"))?
        {
            let mut data = serde_json::Map::new();
            for (i, column) in columns.iter().enumerate() {
                let value = match row
                    .get_ref(i)
                    .map_err(|_| err("Traffic field export failed"))?
                {
                    rusqlite::types::ValueRef::Null => Value::Null,
                    rusqlite::types::ValueRef::Integer(n) => json!(n),
                    rusqlite::types::ValueRef::Real(n) => json!(n),
                    rusqlite::types::ValueRef::Text(t) => {
                        json!(std::str::from_utf8(t).map_err(|_| Error::new(
                            "PARSE_ERROR",
                            "SQLite metadata is not UTF-8"
                        ))?)
                    }
                    rusqlite::types::ValueRef::Blob(b) => binary(b.to_vec()),
                };
                data.insert(column.clone(), value);
            }
            if name == "exchange_bodies" {
                for side in ["req", "resp"] {
                    let blob = data
                        .get(&format!("{side}_blob"))
                        .and_then(Value::as_str)
                        .filter(|h| !h.is_empty())
                        .map(str::to_owned);
                    if let Some(h) = blob {
                        if !export.blobs.contains_key(&h) {
                            let bytes = load_legacy_blob(blob_root, &h)?;
                            used += bytes.len();
                            if used > EXPORT_LIMIT {
                                return Err(Error::new(
                                    "SIZE_LIMIT",
                                    "Traffic blobs exceed import memory boundary",
                                ));
                            }
                            export.blobs.insert(h.clone(), bytes);
                        }
                        data.insert(format!("{side}_body"), json!(h));
                    }
                }
            }
            let value = Value::Object(data);
            used += serde_json::to_vec(&value)?.len();
            if used > EXPORT_LIMIT {
                return Err(Error::new(
                    "SIZE_LIMIT",
                    "Traffic export exceeds memory boundary",
                ));
            }
            rows.push(value);
        }
        export.schema.insert(name.into(), columns);
        export.tables.insert(name.into(), rows);
    }
    connection
        .execute_batch("ROLLBACK;")
        .map_err(|_| err("Traffic read snapshot completion failed"))?;
    Ok(export)
}
pub(crate) fn traffic_archive(
    value: &Value,
    files: &BTreeMap<String, Vec<u8>>,
    out: &mut LegacyExport,
) -> Result<()> {
    if value["version"] != 1 {
        return Err(Error::new(
            "VERSION_MISMATCH",
            "Legacy traffic archive version unsupported",
        ));
    }
    let mut exchanges = Vec::new();
    let mut bodies = Vec::new();
    let mut refs = Vec::new();
    for row in value["exchanges"]
        .as_array()
        .ok_or_else(|| Error::new("PARSE_ERROR", "Traffic exchange list missing"))?
    {
        let mut ex = row.as_object().unwrap().clone();
        let mut body = serde_json::Map::new();
        body.insert("id".into(), row["id"].clone());
        for side in ["req", "resp"] {
            let field = format!("{side}_body");
            let head = format!("{side}_head");
            let blob = format!("{side}_blob");
            body.insert(head.clone(), row.get(&head).cloned().unwrap_or(json!("")));
            body.insert(blob.clone(), row.get(&blob).cloned().unwrap_or(Value::Null));
            let value = if let Some(hash) = row[&blob].as_str().filter(|h| !h.is_empty()) {
                let path = format!("traffic/blobs/{hash}.bin");
                let bytes = files
                    .get(&path)
                    .or_else(|| files.get(&format!("blobs/{hash}.bin")))
                    .ok_or_else(|| Error::new("NOT_FOUND", "Archive traffic blob missing"))?;
                out.blobs.insert(hash.into(), bytes.clone());
                refs.push(json!({"hash":hash,"exchange_id":row["id"]}));
                json!(hash)
            } else if let Some(bytes) = row[&field].as_str() {
                json!({"legacy_bytes":bytes})
            } else {
                Value::Null
            };
            body.insert(field.clone(), value);
            for f in [&field, &head, &blob] {
                ex.remove(f);
            }
        }
        ex.insert("path".into(), json!(""));
        exchanges.push(Value::Object(ex));
        bodies.push(Value::Object(body));
    }
    out.tables.insert("exchanges".into(), exchanges);
    out.tables.insert("exchange_bodies".into(), bodies);
    out.tables.insert("blob_refs".into(), refs);
    Ok(())
}
