//! Writing a file so that a failure costs nothing that was already on disk.
//!
//! Every file this server owns - the world, the admin store, the audit log, the wizard's config -
//! is something an operator would rather have stale than half-written. The rule is the same in all
//! of them and it is collected here so it cannot be got right in one place and forgotten in
//! another: write the new bytes to a temporary file beside the target, get them onto the disk,
//! rename over the target, and sync the directory entry. A rename is atomic on every filesystem
//! this runs on, so at no instant does the target hold a prefix of the new content.
//!
//! The failure paths matter as much as the happy one:
//!
//! - A failed write **removes its own temporary file**. Nothing else would ever clean it up, and
//!   the next attempt has to be able to use the name.
//! - A failed write **never touches the target**, so the previous file is byte-identical
//!   afterwards. That property is what the tests at the bottom of this module pin.
//! - Every failure is **explained**, in the style [`crate::net::listener::bind`] already uses for a
//!   refused bind: the [`std::io::ErrorKind`] is preserved so a caller matching on it still can,
//!   and the message gains a sentence saying what the operator should actually go and look at. A
//!   log line reading `Os { code: 28 }` tells nobody anything; one reading "the filesystem holding
//!   /srv/worlds is full" tells them where to go.
//!
//! What this does not promise: durability against a power cut for anything the caller did not ask
//! to be synced, and atomicity across a rename that crosses filesystems (the temporary file is
//! always beside the target, so it never does).

use std::io::Write as _;
use std::path::{Path, PathBuf};

/// The temporary file a write to `path` goes through: the target's own name with `.new` appended.
///
/// Appended to the whole file name rather than swapped in as an extension, so `world.wld` becomes
/// `world.wld.new` rather than clobbering its own `.wld` suffix, and so a directory listing sorts
/// the scratch file next to the thing it belongs to.
pub fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".new");
    PathBuf::from(name)
}

/// Rewrite an I/O failure into something an operator can act on, keeping the original
/// [`std::io::ErrorKind`] so a caller that matches on the kind still can.
///
/// `doing` names the attempt in the operator's terms ("saving the world", "rotating the audit
/// log"), because the kind alone does not distinguish a full disk found while writing a backup
/// from the same disk found while writing the world itself.
pub fn explain(doing: &str, path: &Path, e: &std::io::Error) -> std::io::Error {
    use std::io::ErrorKind;
    let where_ = path.display();
    // ENOSPC arrives as `StorageFull` on the platforms std knows it on, and as a bare errno
    // elsewhere; both mean the same thing here, unlike on a socket bind where error 28 is socket
    // exhaustion rather than the disk (see `net::listener::explain_bind_failure`).
    let full = e.kind() == ErrorKind::StorageFull || e.raw_os_error() == Some(28);
    let advice = if full {
        format!(
            "{doing}: the filesystem holding {where_} is full (error 28, \"no space left on \
             device\"). Nothing was overwritten - the previous file is still there and intact. \
             Free some space, or point the server at a disk with room, and it will retry."
        )
    } else {
        match e.kind() {
            ErrorKind::PermissionDenied => format!(
                "{doing}: not permitted to write {where_}. Check that the directory is writable by \
                 the account this server runs as; the previous file was left untouched."
            ),
            ErrorKind::ReadOnlyFilesystem => format!(
                "{doing}: {where_} is on a read-only filesystem. Remount it read-write (or move \
                 the world somewhere writable); the previous file was left untouched."
            ),
            ErrorKind::NotFound => format!(
                "{doing}: {where_} has nowhere to go - its directory no longer exists. Something \
                 moved or deleted it while the server was running. Recreate the directory, or \
                 point the server at one that exists."
            ),
            ErrorKind::QuotaExceeded => format!(
                "{doing}: the disk quota for {where_} is exhausted. Raise the quota or free space \
                 under it; the previous file was left untouched."
            ),
            ErrorKind::NotADirectory => format!(
                "{doing}: part of the path to {where_} is a file, not a directory. Check the \
                 configured path."
            ),
            ErrorKind::IsADirectory => {
                format!("{doing}: {where_} is a directory, not a file. Check the configured path.")
            }
            ErrorKind::CrossesDevices => format!(
                "{doing}: {where_} and its temporary file are on different filesystems, so the \
                 write could not be finished atomically. This should not happen - the temporary \
                 file is always created beside the target - and suggests a bind mount or symlink \
                 in the path."
            ),
            _ => format!("{doing}: could not write {where_}: {e}"),
        }
    };
    std::io::Error::new(e.kind(), advice)
}

/// Write `bytes` to `path` so that a failure leaves whatever was already there byte-identical.
///
/// The bytes go to [`temp_path`], are flushed to the disk rather than only to the page cache, and
/// are then renamed over the target; the directory entry is synced afterwards so the replacement
/// survives a power cut and not merely a process crash. A failure at any step removes the
/// temporary file and returns an [`explain`]ed error.
pub fn write_atomic(doing: &str, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temp = temp_path(path);
    if let Err(e) = write_and_sync(&temp, bytes) {
        // Nothing else will ever clean this up, and the next attempt needs the name back.
        let _ = std::fs::remove_file(&temp);
        return Err(explain(doing, path, &e));
    }
    if let Err(e) = std::fs::rename(&temp, path) {
        // On Windows a rename fails outright if anything else holds the destination open. The
        // previous file is untouched either way, which is the whole point.
        let _ = std::fs::remove_file(&temp);
        return Err(explain(doing, path, &e));
    }
    sync_parent_dir(path);
    Ok(())
}

/// Write a file and get it onto the disk, rather than into the page cache.
///
/// `std::fs::write` returns as soon as the kernel has the bytes, so a crash of the *machine* - as
/// opposed to the process - can leave a renamed file whose contents never landed. That is the
/// classic false durability: atomic with respect to a process crash, and not with respect to a
/// power cut.
///
/// Errors are raw here, not [`explain`]ed: the caller knows which path the operator cares about
/// (the target, not the scratch file it went through) and explains against that one.
pub fn write_and_sync(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Make a rename durable by syncing the directory that now holds the new entry.
///
/// Best effort on purpose. A filesystem that refuses to open or sync a directory (some network
/// mounts, and Windows generally) is not a reason to report a save that did land as failed.
pub fn sync_parent_dir(path: &Path) {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty())
        && let Ok(dir) = std::fs::File::open(parent)
    {
        let _ = dir.sync_all();
    }
}

/// Copy `from` over `to` without ever leaving `to` as a prefix of the copy.
///
/// The same temp-then-rename shape as [`write_atomic`], for the two places that copy a whole file
/// rather than serialise one: making a world backup, and putting one back. A plain
/// `std::fs::copy` truncates the destination first and then fills it, so a disk that runs out
/// halfway leaves a healthy backup replaced by a fragment of a world - which is precisely the file
/// somebody reaches for on the worst day they have.
/// Flush a freshly copied temporary file to the disk, by whatever means the platform allows.
///
/// `std::fs::copy` goes through the page cache like any other write, so the same durability
/// argument as `write_and_sync` applies and the copy has to be synced before it is renamed into
/// place. Reopening is the cheap way to get a syncable handle without reading the bytes back
/// through this process; *how* to reopen is where the platforms part company.
///
/// On unix a read-only handle is enough, and is what this deliberately uses: `std::fs::copy` gives
/// the temporary file the source's permissions, so asking for write access would fail on a world an
/// operator had marked read-only. `fsync` needs no write access, so it does not have to.
///
/// **Windows is the opposite, and got this wrong for as long as the code existed.** `sync_all` there
/// is `FlushFileBuffers`, which the API documents as failing unless the handle carries
/// `GENERIC_WRITE`. A read-only handle therefore returned `PermissionDenied` every single time, so
/// `copy_atomic` never once succeeded on Windows: `rotate_backups` logs its failures and carries on
/// by design, so every Windows operator was running with **no world backups at all**, silently,
/// with the world itself saving perfectly. Found the first time this project's tests were run on
/// Windows, which had never happened before 2026-09-05.
///
/// So the temporary file (which is ours, made a moment ago, and about to be renamed away) has its
/// read-only attribute cleared if the copy brought one across, and is then opened for write. The
/// operator's own world file is never touched by this: only the copy is.
fn sync_copied_temp(temp: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::fs::File::open(temp)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let permissions = std::fs::metadata(temp)?.permissions();
        if permissions.readonly() {
            let mut writable = permissions;
            #[allow(clippy::permissions_set_readonly_false)]
            writable.set_readonly(false);
            std::fs::set_permissions(temp, writable)?;
        }
        std::fs::OpenOptions::new()
            .write(true)
            .open(temp)?
            .sync_all()
    }
}

pub fn copy_atomic(doing: &str, from: &Path, to: &Path) -> std::io::Result<()> {
    let temp = temp_path(to);
    let copied = std::fs::copy(from, &temp).and_then(|_| sync_copied_temp(&temp));
    if let Err(e) = copied {
        let _ = std::fs::remove_file(&temp);
        return Err(explain(doing, to, &e));
    }
    if let Err(e) = std::fs::rename(&temp, to) {
        let _ = std::fs::remove_file(&temp);
        return Err(explain(doing, to, &e));
    }
    sync_parent_dir(to);
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    /// A directory whose permissions are put back on drop, so a failing test cannot leave a
    /// write-protected directory behind to poison every later run in the same temp tree.
    ///
    /// `Drop` rather than a tidy-up line at the end of each test, deliberately: an assertion that
    /// fires unwinds past any such line, and the very tests that most need a read-only directory
    /// are the ones most likely to fail while it is in place.
    #[cfg(unix)]
    pub(crate) struct ReadOnlyDir {
        dir: PathBuf,
    }

    #[cfg(unix)]
    impl ReadOnlyDir {
        /// Make `dir` unwritable, or return `None` when this environment cannot express that -
        /// running as root, or a filesystem that ignores the mode bits. Skipping loudly beats a
        /// test that silently proves nothing.
        pub(crate) fn new(dir: &Path) -> Option<Self> {
            use std::os::unix::fs::PermissionsExt as _;
            let mut perms = std::fs::metadata(dir).ok()?.permissions();
            perms.set_mode(0o555);
            std::fs::set_permissions(dir, perms).ok()?;
            let guard = Self {
                dir: dir.to_path_buf(),
            };
            // Prove the protection actually took, rather than assuming chmod meant anything here.
            if std::fs::File::create(dir.join(".writability-probe")).is_ok() {
                let _ = std::fs::remove_file(dir.join(".writability-probe"));
                return None;
            }
            Some(guard)
        }
    }

    #[cfg(unix)]
    impl Drop for ReadOnlyDir {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt as _;
            if let Ok(meta) = std::fs::metadata(&self.dir) {
                let mut perms = meta.permissions();
                perms.set_mode(0o755);
                let _ = std::fs::set_permissions(&self.dir, perms);
            }
        }
    }

    /// The same idea for a single file: unwritable while it is held, put back on drop.
    ///
    /// A read-only *directory* is not the same failure as a read-only *file*, and conflating them
    /// hides a real behaviour: POSIX lets an existing, writable file be appended to inside a
    /// directory nobody may create entries in. An append-only log therefore keeps working in a
    /// directory that refuses every rename, which is exactly what the audit log's rotation tests
    /// turn on.
    #[cfg(unix)]
    pub(crate) struct ReadOnlyFile {
        file: PathBuf,
    }

    #[cfg(unix)]
    impl ReadOnlyFile {
        pub(crate) fn new(file: &Path) -> Option<Self> {
            use std::os::unix::fs::PermissionsExt as _;
            let mut perms = std::fs::metadata(file).ok()?.permissions();
            perms.set_mode(0o444);
            std::fs::set_permissions(file, perms).ok()?;
            let guard = Self {
                file: file.to_path_buf(),
            };
            if std::fs::OpenOptions::new().append(true).open(file).is_ok() {
                return None;
            }
            Some(guard)
        }
    }

    #[cfg(unix)]
    impl Drop for ReadOnlyFile {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt as _;
            if let Ok(meta) = std::fs::metadata(&self.file) {
                let mut perms = meta.permissions();
                perms.set_mode(0o644);
                let _ = std::fs::set_permissions(&self.file, perms);
            }
        }
    }

    /// A fresh, empty temp directory named after the caller, cleaned before use.
    pub(crate) fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "terrustia-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        dir
    }

    #[test]
    fn a_full_disk_is_explained_as_a_full_disk() {
        let path = Path::new("/srv/worlds/Terrustia.wld");
        for e in [
            Error::from(ErrorKind::StorageFull),
            Error::from_raw_os_error(28),
        ] {
            let kind = e.kind();
            let explained = explain("saving the world", path, &e);
            assert_eq!(explained.kind(), kind, "the kind must survive explaining");
            let msg = explained.to_string();
            assert!(
                msg.contains("saving the world")
                    && msg.contains("Terrustia.wld")
                    && msg.contains("full"),
                "a full disk must say so, and say where: {msg}"
            );
            assert!(
                msg.contains("previous file is still there"),
                "the operator's first question is whether they lost the world: {msg}"
            );
        }
    }

    #[test]
    fn each_handleable_failure_gets_its_own_advice() {
        let path = Path::new("/srv/worlds/Terrustia.wld");
        for (kind, expected) in [
            (ErrorKind::PermissionDenied, "writable by the account"),
            (ErrorKind::ReadOnlyFilesystem, "read-only filesystem"),
            (ErrorKind::NotFound, "directory no longer exists"),
            (ErrorKind::QuotaExceeded, "quota"),
            (ErrorKind::NotADirectory, "is a file, not a directory"),
            (ErrorKind::IsADirectory, "is a directory, not a file"),
        ] {
            let explained = explain("saving the world", path, &Error::from(kind));
            assert_eq!(explained.kind(), kind);
            let msg = explained.to_string();
            assert!(
                msg.contains(expected),
                "{kind:?} should advise about {expected:?}, got: {msg}"
            );
        }
        // Anything std has no kind for still names the path and the attempt.
        let other = explain(
            "saving the world",
            path,
            &Error::from(ErrorKind::ConnectionReset),
        );
        assert!(other.to_string().contains("could not write"));
    }

    #[test]
    fn a_write_that_succeeds_leaves_no_scratch_file() {
        let dir = temp_dir("safe-write-clean");
        let path = dir.join("thing.toml");
        write_atomic("writing a thing", &path, b"hello").expect("an ordinary write");
        assert_eq!(std::fs::read(&path).expect("read back"), b"hello");
        assert!(
            !temp_path(&path).exists(),
            "the temporary file must not survive a successful write"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The property the whole module exists for: a failed write costs nothing that was there.
    #[cfg(unix)]
    #[test]
    fn a_failed_write_leaves_the_previous_file_byte_identical() {
        let dir = temp_dir("safe-write-readonly");
        let path = dir.join("thing.toml");
        write_atomic("writing a thing", &path, b"the good version").expect("the first write");
        let before = std::fs::read(&path).expect("read back");

        let Some(_guard) = ReadOnlyDir::new(&dir) else {
            eprintln!("skipping: this environment cannot make a directory read-only");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        };
        let e = write_atomic("writing a thing", &path, b"a doomed replacement")
            .expect_err("a read-only directory must refuse the write");
        assert_eq!(e.kind(), ErrorKind::PermissionDenied);
        assert!(e.to_string().contains("writable by the account"));
        assert_eq!(
            std::fs::read(&path).expect("read back"),
            before,
            "a failed write must leave the previous file byte-identical"
        );
        drop(_guard);
        assert!(
            !temp_path(&path).exists(),
            "a failed write must clean up after itself"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The other realistic external failure: the directory is simply gone.
    #[test]
    fn a_vanished_directory_is_reported_rather_than_crashing() {
        let dir = temp_dir("safe-write-vanished");
        let path = dir.join("thing.toml");
        std::fs::remove_dir_all(&dir).expect("remove the directory out from under it");
        let e = write_atomic("writing a thing", &path, b"nowhere to go")
            .expect_err("a missing directory must refuse the write");
        assert_eq!(e.kind(), ErrorKind::NotFound);
        assert!(
            e.to_string().contains("directory no longer exists"),
            "got: {e}"
        );
    }

    /// The reported case: a copy that is refused outright leaves the destination as it was.
    ///
    /// The unreported case - a copy that fails *part-way*, which is what `copy_atomic` exists for -
    /// needs a genuinely full filesystem or a killed process to reach, and is not portably
    /// injectable here. See `wld_save`'s own backup test for the full statement of that window.
    #[cfg(unix)]
    #[test]
    fn a_failed_copy_leaves_the_destination_byte_identical() {
        let dir = temp_dir("safe-copy-readonly");
        let source = dir.join("source.bin");
        let dest = dir.join("dest.bin");
        std::fs::write(&source, b"new contents").expect("write the source");
        std::fs::write(&dest, b"the good version").expect("write the destination");

        let Some(_guard) = ReadOnlyDir::new(&dir) else {
            eprintln!("skipping: this environment cannot make a directory read-only");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        };
        let e = copy_atomic("backing something up", &source, &dest)
            .expect_err("a read-only directory must refuse the copy");
        assert_eq!(e.kind(), ErrorKind::PermissionDenied);
        assert_eq!(
            std::fs::read(&dest).expect("read back"),
            b"the good version",
            "a failed copy must not leave a fragment where the old file was"
        );
        drop(_guard);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A read-only *source* is still backed up.
    ///
    /// `std::fs::copy` gives the temporary file the source's permissions, so the durability sync
    /// has to open it read-only. Opening it for write instead - the obvious way to get a syncable
    /// handle - fails here with `PermissionDenied`, and a world an operator had chmod'd read-only
    /// would silently stop being backed up while its saves carried on working (the rename that
    /// replaces it needs no permission on the file itself, only on the directory).
    #[cfg(unix)]
    #[test]
    fn a_read_only_source_can_still_be_copied() {
        let dir = temp_dir("safe-copy-readonly-source");
        let source = dir.join("source.bin");
        let dest = dir.join("dest.bin");
        std::fs::write(&source, b"a protected original").expect("write the source");

        let Some(_guard) = ReadOnlyFile::new(&source) else {
            eprintln!("skipping: this environment cannot make a file read-only");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        };
        copy_atomic("backing something up", &source, &dest).expect("a read-only source is fine");
        drop(_guard);

        assert_eq!(
            std::fs::read(&dest).expect("read back"),
            b"a protected original"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A real ENOSPC, where the platform offers one. `/dev/full` accepts an open and fails every
    /// write with error 28, which is the genuine article rather than a synthesised `io::Error`.
    /// Linux has it; macOS does not, so this skips there and the mapping above carries the case.
    #[test]
    fn a_real_enospc_write_is_reported_and_not_a_panic() {
        let full = Path::new("/dev/full");
        if !full.exists() {
            eprintln!("skipping: this platform has no /dev/full to produce a real ENOSPC");
            return;
        }
        let e = write_and_sync(full, b"anything at all").expect_err("/dev/full always refuses");
        assert!(
            e.kind() == ErrorKind::StorageFull || e.raw_os_error() == Some(28),
            "expected ENOSPC from /dev/full, got {e:?}"
        );
        let explained = explain("saving the world", full, &e);
        assert!(explained.to_string().contains("full"), "got: {explained}");
    }
}
