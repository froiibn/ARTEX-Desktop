use artex_core::{Error, Result, new_id, valid_id};
use fs2::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
/// Native advisory lock, held for the entire app lifetime. The PID is diagnostic
/// only; a stale lock file is not evidence of a running application.
pub struct ProfileInstanceLock {
    file: File,
    profile: PathBuf,
    session: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchRequest {
    pub request_id: String,
    pub workspace: Option<PathBuf>,
    pub read_only: bool,
}
pub struct LaunchListener {
    rx: mpsc::Receiver<Vec<LaunchRequest>>,
    stop: Arc<AtomicBool>,
}
impl LaunchListener {
    pub fn poll(&self) -> Vec<LaunchRequest> {
        self.rx.try_recv().unwrap_or_default()
    }
}
impl Drop for LaunchListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
const MAX_MAILBOX: usize = 32;
const MAX_MESSAGE: usize = 65536;
const MAGIC: &[u8; 8] = b"ARTXLCH2";
impl ProfileInstanceLock {
    /// Profile I/O stays off the UI thread. The wake closure may restore a
    /// hidden viewport; opening/switching still needs normal UI confirmation.
    pub fn start_launch_listener(
        self: &Arc<Self>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<LaunchListener> {
        let (tx, rx) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let owner = Arc::downgrade(self);
        std::thread::Builder::new()
            .name("artex-native-launch-inbox".into())
            .spawn(move || {
                let mut pending = None;
                while !worker_stop.load(Ordering::Acquire) {
                    if pending.is_none() {
                        let Some(owner) = owner.upgrade() else { break };
                        if let Ok(batch) = owner.drain_launch_requests() {
                            if !batch.is_empty() {
                                pending = Some(batch);
                            }
                        }
                    }
                    if let Some(batch) = pending.take() {
                        match tx.try_send(batch) {
                            Ok(()) => wake(),
                            Err(mpsc::TrySendError::Full(batch)) => pending = Some(batch),
                            Err(mpsc::TrySendError::Disconnected(_)) => break,
                        }
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            })
            .map_err(Error::io)?;
        Ok(LaunchListener { rx, stop })
    }
    pub fn acquire(profile_directory: &Path) -> Result<Self> {
        if !profile_directory.is_absolute() {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Native profile must be an absolute directory",
            ));
        }
        fs::create_dir_all(profile_directory).map_err(Error::io)?;
        let mut file = open_profile_lock(&profile_directory.join("desktop.instance.lock"), true)?;
        file.try_lock_exclusive().map_err(|_| {
            Error::new(
                "BUSY",
                "ARTEX Desktop is already running for this user profile",
            )
        })?;
        let profile = profile_directory.canonicalize().map_err(Error::io)?;
        let session = new_id("instance");
        let inbox = mailbox(&profile)?;
        let _queue = lock_queue(&inbox)?;
        // Only the exclusive profile owner retires launch files from previous
        // sessions. This directory is distinct from every user workspace.
        for entry in fs::read_dir(&inbox).map_err(Error::io)?.take(128) {
            let entry = entry.map_err(Error::io)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if launch_name(&name) && entry.file_type().map_err(Error::io)?.is_file() {
                fs::remove_file(entry.path()).map_err(Error::io)?;
            }
        }
        file.set_len(0)
            .and_then(|_| file.write_all(format!("pid={}\n", std::process::id()).as_bytes()))
            .and_then(|_| file.sync_all())
            .map_err(Error::io)?;
        // Windows LockFileEx denies reads through every other handle in the
        // locked range. Publish the nonce separately, while holding the same
        // queue lock as senders; never ask a sender to read the lifetime lock.
        let descriptor = inbox.join("session.info");
        match fs::symlink_metadata(&descriptor) {
            Ok(_) => {
                drop(open_private_file(&descriptor, false, false)?);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(Error::io(error)),
        }
        crate::tools::atomic_write(
            &descriptor,
            format!("session={session}\nlaunch_protocol=2\n").as_bytes(),
        )?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&descriptor, fs::Permissions::from_mode(0o600))
                .map_err(Error::io)?;
        }
        Ok(Self {
            file,
            profile,
            session,
        })
    }
    /// A second launcher submits a bounded launch intent only while another
    /// process actually holds the native profile lock. No engine or network IPC.
    pub fn handoff(profile: &Path, workspace: Option<&Path>) -> Result<String> {
        Self::handoff_with_mode(profile, workspace, false)
    }
    pub fn handoff_with_mode(
        profile: &Path,
        workspace: Option<&Path>,
        read_only: bool,
    ) -> Result<String> {
        let profile = profile.canonicalize().map_err(Error::io)?;
        let lock = open_profile_lock(&profile.join("desktop.instance.lock"), false)?;
        match lock.try_lock_exclusive() {
            Ok(()) => {
                let _ = FileExt::unlock(&lock);
                return Err(Error::new(
                    "NOT_FOUND",
                    "No active native instance owns this profile",
                ));
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {}
            Err(error) => return Err(Error::io(error)),
        }
        let workspace = workspace.map(validate_workspace).transpose()?;
        let request_id = new_id("launch");
        let request = LaunchRequest {
            request_id: request_id.clone(),
            workspace,
            read_only,
        };
        let inbox = mailbox(&profile)?;
        let _queue = lock_queue(&inbox)?;
        match lock.try_lock_exclusive() {
            Ok(()) => {
                let _ = FileExt::unlock(&lock);
                return Err(Error::new(
                    "NOT_FOUND",
                    "Native instance retired before handoff",
                ));
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {}
            Err(error) => return Err(Error::io(error)),
        }
        // Read the session after acquiring the same queue lock as startup, so
        // an overlapping first-launch cannot accept then discard an old nonce.
        let mut session_file = open_private_file(&inbox.join("session.info"), false, false)
            .map_err(|error| {
                if error.details.get("kind").and_then(|kind| kind.as_str()) == Some("NotFound") {
                    Error::new(
                        "FEATURE_DISABLED",
                        "Running instance has no native launch descriptor",
                    )
                } else {
                    error
                }
            })?;
        if session_file.metadata().map_err(Error::io)?.len() > 1024 {
            return Err(Error::new(
                "SIZE_LIMIT",
                "Native launch descriptor exceeds its limit",
            ));
        }
        let mut descriptor = String::new();
        Read::by_ref(&mut session_file)
            .take(1025)
            .read_to_string(&mut descriptor)
            .map_err(Error::io)?;
        if descriptor.len() > 1024 {
            return Err(Error::new(
                "SIZE_LIMIT",
                "Native launch descriptor exceeds its limit",
            ));
        }
        let session = descriptor
            .lines()
            .find_map(|line| line.strip_prefix("session="))
            .filter(|id| valid_id(id, "instance"))
            .ok_or_else(|| {
                Error::new(
                    "FEATURE_DISABLED",
                    "The running application does not support native launch handoff",
                )
            })?;
        if !descriptor.lines().any(|line| line == "launch_protocol=2") {
            return Err(Error::new(
                "FEATURE_DISABLED",
                "Running native instance uses another launch protocol",
            ));
        }
        if fs::read_dir(&inbox)
            .map_err(Error::io)?
            .filter_map(|e| e.ok())
            .filter(|e| launch_name(&e.file_name().to_string_lossy()))
            .take(MAX_MAILBOX)
            .count()
            >= MAX_MAILBOX
        {
            return Err(Error::new("BUSY", "Native launch request queue is full"));
        }
        let payload = encode(session, &request)?;
        crate::tools::atomic_write(&inbox.join(format!("{request_id}.bin")), &payload)?;
        Ok(request_id)
    }
    /// Poll on the UI tick. Paths remain suggestions until the UI confirms a
    /// workspace switch; neither admission nor the live kernel is changed here.
    pub fn drain_launch_requests(&self) -> Result<Vec<LaunchRequest>> {
        let inbox = mailbox(&self.profile)?;
        let _queue = lock_queue(&inbox)?;
        let mut entries = fs::read_dir(&inbox)
            .map_err(Error::io)?
            .take(128)
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(Error::io)?;
        entries.sort_by_key(|entry| entry.file_name());
        let mut requests = Vec::new();
        for entry in entries
            .into_iter()
            .filter(|e| launch_name(&e.file_name().to_string_lossy()))
            .take(MAX_MAILBOX)
        {
            let meta = fs::symlink_metadata(entry.path()).map_err(Error::io)?;
            if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > MAX_MESSAGE as u64 {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Native launch request is not a bounded regular file",
                ));
            }
            let bytes = fs::read(entry.path()).map_err(Error::io)?;
            // Consume malformed/stale intents rather than repeatedly acting on
            // them. No caller can ask this path to delete a user workspace.
            fs::remove_file(entry.path()).map_err(Error::io)?;
            if let Ok(Some(request)) = decode(&bytes, &self.session) {
                if entry.file_name().to_string_lossy() == format!("{}.bin", request.request_id) {
                    requests.push(request);
                }
            }
        }
        Ok(requests)
    }
}
fn open_profile_lock(path: &Path, create: bool) -> Result<File> {
    open_private_file(path, true, create)
}
fn open_private_file(path: &Path, write: bool, create: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(write)
        .create(create)
        .truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW).mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000);
    }
    let file = options.open(path).map_err(Error::io)?;
    let meta = file.metadata().map_err(Error::io)?;
    #[cfg(windows)]
    let reparse = {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    };
    #[cfg(not(windows))]
    let reparse = false;
    if !meta.is_file() || reparse {
        return Err(Error::new(
            "PERMISSION_DENIED",
            "Native profile control file must be a regular file without links",
        ));
    }
    Ok(file)
}
fn lock_queue(inbox: &Path) -> Result<File> {
    let file = open_profile_lock(&inbox.join("queue.lock"), true)?;
    file.try_lock_exclusive()
        .map_err(|_| Error::new("BUSY", "Native launch queue is busy"))?;
    Ok(file)
}
fn launch_name(name: &str) -> bool {
    name.strip_suffix(".bin")
        .is_some_and(|id| valid_id(id, "launch"))
}
fn mailbox(profile: &Path) -> Result<PathBuf> {
    // Normalize both sides: Windows canonical paths use a verbatim prefix,
    // which cannot be compared component-wise with the caller's DOS path.
    let profile = profile.canonicalize().map_err(Error::io)?;
    let path = profile.join("native-launch");
    if let Ok(meta) = fs::symlink_metadata(&path) {
        #[cfg(windows)]
        let reparse = {
            use std::os::windows::fs::MetadataExt;
            meta.file_attributes() & 0x400 != 0
        };
        #[cfg(not(windows))]
        let reparse = false;
        if !meta.is_dir()
            || meta.file_type().is_symlink()
            || reparse
            || !path
                .canonicalize()
                .map_err(Error::io)?
                .starts_with(&profile)
        {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "Native launch directory escaped its profile",
            ));
        }
    }
    fs::create_dir_all(&path).map_err(Error::io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).map_err(Error::io)?;
    }
    Ok(path)
}
fn validate_workspace(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        return Err(Error::new(
            "INVALID_PARAMS",
            "Launch workspace must be absolute",
        ));
    }
    let path = path.canonicalize().map_err(Error::io)?;
    let project = path.join("project.json");
    let meta = fs::symlink_metadata(&project).map_err(Error::io)?;
    if !path.is_dir()
        || !meta.is_file()
        || meta.file_type().is_symlink()
        || meta.len() > 1024 * 1024
    {
        return Err(Error::new(
            "INVALID_PARAMS",
            "Launch workspace requires a bounded regular project.json",
        ));
    }
    Ok(path)
}
fn encode(session: &str, request: &LaunchRequest) -> Result<Vec<u8>> {
    use sha2::{Digest, Sha256};
    let path = request
        .workspace
        .as_ref()
        .map(|p| {
            p.to_str()
                .ok_or_else(|| Error::new("INVALID_PARAMS", "Launch path must be Unicode"))
        })
        .transpose()?
        .unwrap_or("");
    if path.len() > 32768 || path.contains('\0') {
        return Err(Error::new(
            "SIZE_LIMIT",
            "Native launch path limit exceeded",
        ));
    }
    let mut bytes = MAGIC.to_vec();
    bytes.push(u8::from(request.read_only));
    for field in [session, request.request_id.as_str(), path] {
        bytes.extend_from_slice(&(field.len() as u32).to_le_bytes());
        bytes.extend_from_slice(field.as_bytes());
    }
    let digest = Sha256::digest(&bytes);
    bytes.extend_from_slice(&digest);
    Ok(bytes)
}
fn decode(bytes: &[u8], session: &str) -> Result<Option<LaunchRequest>> {
    use sha2::{Digest, Sha256};
    if bytes.len() < 9 + 12 + 32 || bytes.len() > MAX_MESSAGE || &bytes[..8] != MAGIC {
        return Err(Error::new("INVALID_PARAMS", "Invalid launch intent"));
    }
    let payload = &bytes[..bytes.len() - 32];
    if Sha256::digest(payload)[..] != bytes[bytes.len() - 32..] {
        return Err(Error::new("INVALID_PARAMS", "Launch intent hash mismatch"));
    }
    let read_only = match payload[8] {
        0 => false,
        1 => true,
        _ => {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Invalid launch read-only flag",
            ));
        }
    };
    let mut offset = 9;
    let mut fields = Vec::new();
    for _ in 0..3 {
        if offset + 4 > payload.len() {
            return Err(Error::new("INVALID_PARAMS", "Truncated launch intent"));
        }
        let len = u32::from_le_bytes(payload[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        if len > 32768 || offset + len > payload.len() {
            return Err(Error::new("SIZE_LIMIT", "Launch field exceeds limit"));
        }
        fields.push(
            std::str::from_utf8(&payload[offset..offset + len])
                .map_err(|_| Error::new("INVALID_PARAMS", "Launch fields require Unicode"))?,
        );
        offset += len;
    }
    if offset != payload.len() || !valid_id(fields[1], "launch") {
        return Err(Error::new("INVALID_PARAMS", "Launch identity mismatch"));
    }
    if fields[0] != session {
        return Ok(None);
    }
    let workspace = if fields[2].is_empty() {
        None
    } else {
        Some(validate_workspace(Path::new(fields[2]))?)
    };
    Ok(Some(LaunchRequest {
        request_id: fields[1].into(),
        workspace,
        read_only,
    }))
}
impl Drop for ProfileInstanceLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_lock_is_native_and_stale_file_can_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let first = ProfileInstanceLock::acquire(dir.path()).unwrap();
        assert_eq!(
            ProfileInstanceLock::acquire(dir.path()).err().unwrap().code,
            "BUSY"
        );
        drop(first);
        assert!(ProfileInstanceLock::acquire(dir.path()).is_ok());
    }
    #[test]
    fn launch_handoff_is_bounded_session_fenced_and_does_not_open_workspace() {
        let profile = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        fs::write(workspace.path().join("project.json"), "{}").unwrap();
        let owner = ProfileInstanceLock::acquire(profile.path()).unwrap();
        let descriptor =
            fs::read_to_string(profile.path().join("native-launch/session.info")).unwrap();
        assert!(descriptor.contains(&format!("session={}\n", owner.session)));
        let id = ProfileInstanceLock::handoff(profile.path(), Some(workspace.path())).unwrap();
        let requests = owner.drain_launch_requests().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].request_id, id);
        assert!(!requests[0].read_only);
        assert_eq!(
            requests[0].workspace,
            Some(workspace.path().canonicalize().unwrap())
        );
        assert!(owner.drain_launch_requests().unwrap().is_empty());
        ProfileInstanceLock::handoff_with_mode(profile.path(), Some(workspace.path()), true)
            .unwrap();
        assert!(owner.drain_launch_requests().unwrap()[0].read_only);
        assert!(ProfileInstanceLock::handoff(profile.path(), Some(Path::new("relative"))).is_err());
        assert_eq!(
            fs::read_to_string(workspace.path().join("project.json")).unwrap(),
            "{}"
        );
        let old_session = owner.session.clone();
        drop(owner);
        let fresh = ProfileInstanceLock::acquire(profile.path()).unwrap();
        let stale = LaunchRequest {
            request_id: new_id("launch"),
            workspace: None,
            read_only: false,
        };
        let raw = encode(&old_session, &stale).unwrap();
        crate::tools::atomic_write(
            &mailbox(profile.path())
                .unwrap()
                .join(format!("{}.bin", stale.request_id)),
            &raw,
        )
        .unwrap();
        assert!(fresh.drain_launch_requests().unwrap().is_empty());
        for _ in 0..MAX_MAILBOX {
            ProfileInstanceLock::handoff(profile.path(), None).unwrap();
        }
        assert_eq!(
            ProfileInstanceLock::handoff(profile.path(), None)
                .unwrap_err()
                .code,
            "BUSY"
        );
        assert_eq!(fresh.drain_launch_requests().unwrap().len(), MAX_MAILBOX);
        drop(fresh);
        assert_eq!(
            ProfileInstanceLock::handoff(profile.path(), None)
                .unwrap_err()
                .code,
            "NOT_FOUND"
        );
    }
    #[test]
    fn unlocked_session_descriptor_is_bounded_and_missing_is_not_accepted() {
        let profile = tempfile::tempdir().unwrap();
        let owner = ProfileInstanceLock::acquire(profile.path()).unwrap();
        let descriptor = profile.path().join("native-launch/session.info");
        fs::write(&descriptor, vec![b'x'; 1025]).unwrap();
        assert_eq!(
            ProfileInstanceLock::handoff(profile.path(), None)
                .unwrap_err()
                .code,
            "SIZE_LIMIT"
        );
        fs::remove_file(&descriptor).unwrap();
        assert_eq!(
            ProfileInstanceLock::handoff(profile.path(), None)
                .unwrap_err()
                .code,
            "FEATURE_DISABLED"
        );
        assert_eq!(
            ProfileInstanceLock::acquire(profile.path())
                .err()
                .unwrap()
                .code,
            "BUSY"
        );
        drop(owner);
        let fresh = ProfileInstanceLock::acquire(profile.path()).unwrap();
        ProfileInstanceLock::handoff(profile.path(), None).unwrap();
        assert_eq!(fresh.drain_launch_requests().unwrap().len(), 1);
    }
    #[cfg(unix)]
    #[test]
    fn session_descriptor_cannot_follow_a_symlink() {
        let profile = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        fs::write(outside.path(), "keep").unwrap();
        let owner = ProfileInstanceLock::acquire(profile.path()).unwrap();
        let descriptor = profile.path().join("native-launch/session.info");
        fs::remove_file(&descriptor).unwrap();
        std::os::unix::fs::symlink(outside.path(), &descriptor).unwrap();
        assert!(ProfileInstanceLock::handoff(profile.path(), None).is_err());
        drop(owner);
        assert!(ProfileInstanceLock::acquire(profile.path()).is_err());
        assert_eq!(fs::read_to_string(outside.path()).unwrap(), "keep");
    }
}
