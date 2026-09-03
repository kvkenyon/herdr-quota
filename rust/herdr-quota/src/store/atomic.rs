//! Private atomic file operations for bounded local documents.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
#[cfg(unix)]
use std::time::{Duration, Instant};

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);
#[cfg(unix)]
const LOCK_TIMEOUT: Duration = Duration::from_millis(500);
#[cfg(unix)]
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(10);

/// An error from the atomic storage boundary.
#[derive(Debug)]
pub enum AtomicError {
    /// The target is not a safe regular file that this process can replace.
    UnsafeTarget,
    /// The target has no file name.
    InvalidPath,
    /// A file system operation failed.
    Io(io::Error),
}

impl std::fmt::Display for AtomicError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsafeTarget => formatter.write_str("unsafe storage target"),
            Self::InvalidPath => formatter.write_str("storage path has no file name"),
            Self::Io(error) => write!(formatter, "storage operation failed: {error}"),
        }
    }
}

impl std::error::Error for AtomicError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::UnsafeTarget | Self::InvalidPath => None,
        }
    }
}

impl From<io::Error> for AtomicError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// An atomic replacement error.
#[derive(Debug)]
pub enum ReplaceError<E> {
    /// The storage boundary failed.
    Storage(AtomicError),
    /// The caller rejected the current target contents.
    Validation(E),
}

impl<E> From<AtomicError> for ReplaceError<E> {
    fn from(error: AtomicError) -> Self {
        Self::Storage(error)
    }
}

impl<E> From<io::Error> for ReplaceError<E> {
    fn from(error: io::Error) -> Self {
        Self::Storage(AtomicError::Io(error))
    }
}

/// Read a regular file without following a final symlink.
pub fn read_regular(path: &Path) -> Result<Option<Vec<u8>>, AtomicError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(AtomicError::UnsafeTarget);
    }

    let mut file = open_for_read(path)?;
    if !file.metadata()?.is_file() {
        return Err(AtomicError::UnsafeTarget);
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

/// Replace a document through a private sibling file.
///
/// The validator runs before the temporary file is created and again before
/// the rename. It can preserve a document that the current program cannot use.
pub fn replace_atomically<E, F>(
    path: &Path,
    bytes: &[u8],
    validate_existing: F,
) -> Result<(), ReplaceError<E>>
where
    F: FnMut(Option<&[u8]>) -> Result<(), E>,
{
    replace_atomically_with_hook(path, bytes, validate_existing, |_| Ok(()))
}

/// Move a regular file only when it still has the inspected contents.
pub fn quarantine_if_unchanged(
    path: &Path,
    expected: &[u8],
) -> Result<Option<PathBuf>, AtomicError> {
    let _lock = lock_parent(path)?;
    let Some(current) = read_replaceable(path)? else {
        return Ok(None);
    };
    if current != expected {
        return Ok(None);
    }
    let quarantine_path = unique_sibling(path, "invalid")?;
    fs::rename(path, &quarantine_path)?;
    set_private_permissions(&quarantine_path).ok();
    Ok(Some(quarantine_path))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WriteStage {
    Created,
    Written,
    Synced,
    BeforeRename,
}

fn replace_atomically_with_hook<E, F, H>(
    path: &Path,
    bytes: &[u8],
    mut validate_existing: F,
    mut hook: H,
) -> Result<(), ReplaceError<E>>
where
    F: FnMut(Option<&[u8]>) -> Result<(), E>,
    H: FnMut(WriteStage) -> io::Result<()>,
{
    create_private_parent(path).map_err(AtomicError::from)?;
    let _lock = lock_parent(path)?;
    let existing = read_replaceable(path)?;
    validate_existing(existing.as_deref()).map_err(ReplaceError::Validation)?;

    let (temporary_path, mut temporary_file) = create_private_sibling(path)?;
    let mut renamed = false;
    let result = (|| {
        hook(WriteStage::Created)?;
        temporary_file.write_all(bytes)?;
        hook(WriteStage::Written)?;
        temporary_file.sync_all()?;
        hook(WriteStage::Synced)?;
        drop(temporary_file);

        let existing = read_replaceable(path)?;
        validate_existing(existing.as_deref()).map_err(ReplaceError::Validation)?;
        hook(WriteStage::BeforeRename)?;
        fs::rename(&temporary_path, path)?;
        renamed = true;
        Ok(())
    })();

    if !renamed {
        fs::remove_file(&temporary_path).ok();
    }
    result
}

#[cfg(unix)]
struct SiblingLock(File);

#[cfg(unix)]
impl Drop for SiblingLock {
    fn drop(&mut self) {
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

#[cfg(unix)]
fn lock_parent(path: &Path) -> Result<SiblingLock, AtomicError> {
    let file = File::open(parent_directory(path))?;
    let deadline = Instant::now() + LOCK_TIMEOUT;
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(SiblingLock(file));
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::WouldBlock && error.kind() != io::ErrorKind::Interrupted {
            return Err(error.into());
        }
        if Instant::now() >= deadline {
            return Err(
                io::Error::new(io::ErrorKind::TimedOut, "storage lock is unavailable").into(),
            );
        }
        std::thread::sleep(LOCK_RETRY_DELAY);
    }
}

#[cfg(not(unix))]
struct SiblingLock;

#[cfg(not(unix))]
fn lock_parent(_path: &Path) -> Result<SiblingLock, AtomicError> {
    Ok(SiblingLock)
}

fn create_private_parent(path: &Path) -> io::Result<()> {
    let parent = parent_directory(path);
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(parent)
}

fn read_replaceable(path: &Path) -> Result<Option<Vec<u8>>, AtomicError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(AtomicError::UnsafeTarget);
    }

    let mut file = open_for_read(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || !metadata_is_replaceable(&metadata) {
        return Err(AtomicError::UnsafeTarget);
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

#[cfg(unix)]
fn metadata_is_replaceable(metadata: &fs::Metadata) -> bool {
    metadata_is_replaceable_by(metadata, unsafe { libc::geteuid() })
}

#[cfg(unix)]
fn metadata_is_replaceable_by(metadata: &fs::Metadata, effective_uid: u32) -> bool {
    metadata.uid() == effective_uid && metadata.mode() & 0o200 != 0
}

#[cfg(not(unix))]
fn metadata_is_replaceable(metadata: &fs::Metadata) -> bool {
    !metadata.permissions().readonly()
}

fn open_for_read(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    options.open(path)
}

fn create_private_sibling(path: &Path) -> Result<(PathBuf, File), AtomicError> {
    for _ in 0..128 {
        let temporary_path = unique_sibling(path, "tmp")?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        match options.open(&temporary_path) {
            Ok(file) => {
                if let Err(error) = set_private_file_permissions(&file) {
                    drop(file);
                    fs::remove_file(&temporary_path).ok();
                    return Err(error.into());
                }
                return Ok((temporary_path, file));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not create a unique temporary file",
    )
    .into())
}

fn unique_sibling(path: &Path, label: &str) -> Result<PathBuf, AtomicError> {
    let file_name = path.file_name().ok_or(AtomicError::InvalidPath)?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut name = OsString::from(file_name);
    name.push(format!(
        ".{label}-{}-{timestamp}-{sequence}",
        std::process::id()
    ));
    if label == "tmp" {
        name.push(".tmp");
    }
    Ok(parent_directory(path).join(name))
}

fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

#[cfg(unix)]
fn set_private_file_permissions(file: &File) -> io::Result<()> {
    file.set_permissions(fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_private_file_permissions(_file: &File) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{WriteStage, quarantine_if_unchanged, replace_atomically_with_hook};
    use std::convert::Infallible;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "herdr-quota-atomic-test-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("create test directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn interruption_preserves_the_target_and_removes_the_temporary_file() {
        for failed_stage in [
            WriteStage::Created,
            WriteStage::Written,
            WriteStage::Synced,
            WriteStage::BeforeRename,
        ] {
            let directory = TestDirectory::new();
            let path = directory.path().join("settings.json");
            fs::write(&path, b"previous\n").expect("write initial file");

            let result = replace_atomically_with_hook(
                &path,
                b"next\n",
                |_| Ok::<(), Infallible>(()),
                |stage| {
                    if stage == failed_stage {
                        Err(io::Error::new(io::ErrorKind::Interrupted, "test stop"))
                    } else {
                        Ok(())
                    }
                },
            );

            assert!(result.is_err(), "{failed_stage:?}");
            assert_eq!(fs::read(&path).expect("read target"), b"previous\n");
            assert_eq!(
                fs::read_dir(directory.path())
                    .expect("read directory")
                    .filter_map(Result::ok)
                    .count(),
                1,
                "{failed_stage:?}"
            );
        }
    }

    #[test]
    fn quarantine_preserves_a_replacement_that_differs_from_the_inspected_bytes() {
        let directory = TestDirectory::new();
        let path = directory.path().join("settings.json");
        let malformed = b"{truncated";
        let replacement = b"{\"schemaVersion\":4,\"future\":true}\n";
        fs::write(&path, malformed).expect("write malformed settings");
        fs::write(&path, replacement).expect("install replacement settings");

        let quarantined =
            quarantine_if_unchanged(&path, malformed).expect("check quarantine target");

        assert!(quarantined.is_none());
        assert_eq!(fs::read(&path).expect("read replacement"), replacement);
    }

    #[cfg(unix)]
    #[test]
    fn a_competing_replacement_waits_between_validation_and_rename() {
        use super::replace_atomically;
        use std::sync::mpsc;

        let directory = TestDirectory::new();
        let path = directory.path().join("settings.json");
        fs::write(&path, b"current\n").expect("write current settings");
        let competing_path = path.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let mut competitor = None;

        replace_atomically_with_hook(
            &path,
            b"next\n",
            |_| Ok::<(), Infallible>(()),
            |stage| {
                if stage == WriteStage::BeforeRename {
                    competitor = Some(std::thread::spawn({
                        let competing_path = competing_path.clone();
                        let started_tx = started_tx.clone();
                        move || {
                            started_tx.send(()).expect("signal competing save");
                            replace_atomically(&competing_path, b"future\n", |_| {
                                Ok::<(), Infallible>(())
                            })
                            .expect("write competing settings");
                        }
                    }));
                    started_rx.recv().expect("wait for competing save");
                }
                Ok(())
            },
        )
        .expect("write next settings");
        competitor
            .expect("start competing save")
            .join()
            .expect("join competing save");

        assert_eq!(fs::read(&path).expect("read final settings"), b"future\n");
    }

    #[cfg(unix)]
    #[test]
    #[ignore]
    fn hold_parent_lock_until_terminated() {
        let path = PathBuf::from(std::env::var_os("HERDR_LOCK_TEST_PATH").expect("test path"));
        let marker = PathBuf::from(std::env::var_os("HERDR_LOCK_TEST_MARKER").expect("marker"));
        let _lock = super::lock_parent(&path).expect("lock parent");
        fs::write(marker, b"locked").expect("write marker");
        loop {
            std::thread::park();
        }
    }

    #[cfg(unix)]
    #[test]
    fn process_termination_releases_the_storage_lock() {
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        let directory = TestDirectory::new();
        let path = directory.path().join("settings.json");
        let marker = directory.path().join("locked");
        fs::write(&path, b"current\n").expect("write current settings");
        let mut child = Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--ignored",
                "--exact",
                "store::atomic::tests::hold_parent_lock_until_terminated",
            ])
            .env("HERDR_LOCK_TEST_PATH", &path)
            .env("HERDR_LOCK_TEST_MARKER", &marker)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start lock holder");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(marker.exists(), "lock holder did not start");
        child.kill().expect("terminate lock holder");
        child.wait().expect("wait for lock holder");

        super::replace_atomically(&path, b"next\n", |_| Ok::<(), Infallible>(()))
            .expect("replace after termination");
        assert_eq!(fs::read(path).expect("read settings"), b"next\n");
    }

    #[cfg(unix)]
    #[test]
    fn lock_acquisition_times_out_without_changing_the_target() {
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        let directory = TestDirectory::new();
        let path = directory.path().join("settings.json");
        let marker = directory.path().join("locked");
        fs::write(&path, b"current\n").expect("write current settings");
        let mut child = Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--ignored",
                "--exact",
                "store::atomic::tests::hold_parent_lock_until_terminated",
            ])
            .env("HERDR_LOCK_TEST_PATH", &path)
            .env("HERDR_LOCK_TEST_MARKER", &marker)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start lock holder");
        let start_deadline = Instant::now() + Duration::from_secs(2);
        while !marker.exists() && Instant::now() < start_deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(marker.exists(), "lock holder did not start");

        let started = Instant::now();
        let result = super::replace_atomically(&path, b"next\n", |_| Ok::<(), Infallible>(()));
        let elapsed = started.elapsed();
        child.kill().expect("terminate lock holder");
        child.wait().expect("wait for lock holder");

        assert!(result.is_err());
        assert!(elapsed >= super::LOCK_TIMEOUT);
        assert!(elapsed < Duration::from_secs(2));
        assert_eq!(fs::read(path).expect("read settings"), b"current\n");
    }

    #[cfg(unix)]
    #[test]
    fn foreign_owner_metadata_is_not_replaceable() {
        use super::metadata_is_replaceable_by;
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let directory = TestDirectory::new();
        let path = directory.path().join("settings.json");
        fs::write(&path, b"previous\n").expect("write initial file");
        let metadata = fs::metadata(path).expect("read metadata");

        assert!(metadata_is_replaceable_by(&metadata, metadata.uid()));
        assert!(!metadata_is_replaceable_by(
            &metadata,
            metadata.uid().wrapping_add(1)
        ));

        fs::set_permissions(
            directory.path().join("settings.json"),
            fs::Permissions::from_mode(0o400),
        )
        .expect("make file read only");
        let metadata =
            fs::metadata(directory.path().join("settings.json")).expect("read read-only metadata");
        assert!(!metadata_is_replaceable_by(&metadata, metadata.uid()));
    }
}
