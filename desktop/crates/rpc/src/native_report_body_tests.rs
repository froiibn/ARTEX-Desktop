//! Native report pages preserve full-source integrity and bounded UTF-8 views.
use super::*;

fn opened(path: &Path) -> (Router, WorkspaceScope) {
    let mut router = Router::new(Default::default());
    let (workspace, _) = router
        .native_open(&OpenOptions {
            path: path.to_owned(),
            mode: OpenMode::Existing,
            read_only: true,
        })
        .unwrap();
    (router, workspace.scope)
}
fn page(router: &mut Router, scope: &WorkspaceScope, body: &Value, offset: u64) -> Result<Value> {
    match router.native_query(
        &NativeTicket::new(),
        scope,
        NativeQuery::BodyPage {
            body: body.clone(),
            offset,
        },
    )? {
        NativeDispatch::Complete(reply) => {
            assert_eq!(reply.scope.as_ref(), Some(scope));
            Ok(reply.value)
        }
        NativeDispatch::Deferred => panic!("Body pages must be immutable synchronous reads"),
    }
}
fn files(root: &Path) -> BTreeMap<PathBuf, (u64, String)> {
    fn visit(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, (u64, String)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, out);
            } else {
                let bytes = std::fs::read(&path).unwrap();
                out.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    (bytes.len() as u64, hex::encode(Sha256::digest(bytes))),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

#[test]
fn body_page_reads_persisted_source_above_old_body_limit_without_source_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    let mut store = Store::create(&path, "Large report page fixture").unwrap();
    let content = format!(
        "LARGE-REPORT-START{}LARGE-REPORT-END",
        "a".repeat(MAX_QUEUE_BYTES + MAX_CHUNK)
    );
    let reference = store
        .blob_put(content.as_bytes(), "text/markdown; charset=utf-8")
        .unwrap();
    let body = json!({"text":null,"blob":reference});
    store.checkpoint().unwrap();
    drop(store);
    let before = files(&path);
    let (mut router, scope) = opened(&path);
    let snapshot = router.snapshot().unwrap();
    let old = router.native_query(
        &NativeTicket::new(),
        &scope,
        NativeQuery::Body { body: body.clone() },
    );
    assert_eq!(
        match old {
            Err(error) => error.code,
            _ => panic!("Existing Body memory limit must remain in force"),
        },
        "SIZE_LIMIT"
    );
    for offset in [0, MAX_QUEUE_BYTES as u64, (content.len() - 32) as u64] {
        let result = page(&mut router, &scope, &body, offset).unwrap();
        let text = result["text"].as_str().unwrap();
        assert!(!text.is_empty());
        assert!(text.len() <= MAX_CHUNK);
        assert_eq!(
            text,
            &content[offset as usize..(offset as usize + MAX_CHUNK).min(content.len())]
        );
        assert_eq!(result["total_length"], content.len().to_string());
    }
    assert_eq!(router.snapshot().unwrap().snapshot, snapshot.snapshot);
    router.native_close(&scope).unwrap();
    assert_eq!(
        files(&path),
        before,
        "Read-only report paging must not alter any source bytes"
    );
}

#[test]
fn query_cache_pages_reconstruct_unicode_at_exact_byte_boundaries_and_reject_bad_offsets() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    drop(Store::create(&path, "Unicode pages").unwrap());
    let (mut router, scope) = opened(&path);
    let content = format!(
        "{}가🌏é{}끝",
        "a".repeat(MAX_CHUNK - 1),
        "b".repeat(MAX_CHUNK)
    );
    let body = router
        .externalize_query(artex_core::body(&content))
        .unwrap();
    assert!(body["blob"].is_object());
    let first = page(&mut router, &scope, &body, 0).unwrap();
    assert_eq!(first["text"].as_str().unwrap().len(), MAX_CHUNK - 1);
    assert_eq!(first["next_offset"], (MAX_CHUNK - 1).to_string());
    let mut reconstructed = String::new();
    let mut offset = 0;
    loop {
        let result = page(&mut router, &scope, &body, offset).unwrap();
        let text = result["text"].as_str().unwrap();
        assert!(text.len() <= MAX_CHUNK);
        reconstructed.push_str(text);
        let Some(next) = result["next_offset"].as_str() else {
            break;
        };
        let next = next.parse::<u64>().unwrap();
        assert_eq!(next, offset + text.len() as u64);
        offset = next;
    }
    assert_eq!(reconstructed, content);
    assert_eq!(
        page(&mut router, &scope, &body, MAX_CHUNK as u64)
            .unwrap_err()
            .code,
        "INVALID_PARAMS",
        "Offset inside Korean character must be rejected"
    );
    assert_eq!(
        page(&mut router, &scope, &body, content.len() as u64).unwrap(),
        json!({"text":"","next_offset":null,"total_length":content.len().to_string()})
    );
    assert_eq!(
        page(&mut router, &scope, &body, content.len() as u64 + 1)
            .unwrap_err()
            .code,
        "INVALID_PARAMS"
    );
    let mut stale = scope.clone();
    stale.open_generation = new_id("open");
    assert_eq!(
        page(&mut router, &stale, &body, 0).unwrap_err().code,
        "STALE_EPOCH"
    );
    router.native_close(&scope).unwrap();
}

#[test]
fn body_page_verifies_hash_after_visible_slice_and_rejects_invalid_or_truncated_utf8() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    let mut store = Store::create(&path, "Page integrity").unwrap();
    let bytes = vec![b'z'; MAX_CHUNK * 3];
    let good = store.blob_put(&bytes, "text/plain").unwrap();
    let invalid = store.blob_put(&[b'a', 0xff, b'b'], "text/plain").unwrap();
    let truncated = store.blob_put(&[b'a', 0xe3, 0x81], "text/plain").unwrap();
    drop(store);
    let (mut router, scope) = opened(&path);
    for reference in [invalid, truncated] {
        assert_eq!(
            page(
                &mut router,
                &scope,
                &json!({"text":null,"blob":reference}),
                0
            )
            .unwrap_err()
            .code,
            "CORRUPT_STORE"
        );
    }
    let hash = good["sha256"].as_str().unwrap();
    let location = path.join("objects/blobs").join(&hash[..2]).join(hash);
    let mut damaged = bytes;
    *damaged.last_mut().unwrap() = b'x';
    std::fs::write(location, damaged).unwrap();
    assert_eq!(
        page(&mut router, &scope, &json!({"text":null,"blob":good}), 0)
            .unwrap_err()
            .code,
        "CORRUPT_STORE",
        "Corruption after the visible page must prevent publication of that page"
    );
    router.native_close(&scope).unwrap();
}

#[test]
fn page_window_retains_at_most_one_chunk_and_utf8_lookahead_while_consuming_whole_source() {
    use std::io::Write;
    let mut window = BodyPageWindow::new(MAX_CHUNK as u64 + 1);
    let block = vec![b'a'; MAX_CHUNK];
    for _ in 0..32 {
        window.write_all(&block).unwrap();
    }
    assert_eq!(window.position, (MAX_CHUNK * 32) as u64);
    assert_eq!(window.bytes.len(), MAX_CHUNK + BodyPageWindow::LOOKAHEAD);
    let result = window.reply((MAX_CHUNK * 32) as u64).unwrap();
    assert_eq!(result["text"].as_str().unwrap().len(), MAX_CHUNK);
}
