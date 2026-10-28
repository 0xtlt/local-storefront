//! The write lock of a Function: held by whoever writes its `dist`, so that what a build leaves
//! there is its own when the server reads it.
//!
//! One file per Function, `<record>/locks/<handle>.lock`, shared by the servers of an app. A
//! build takes it before it is spawned, and the server lets it go once it read, checked and
//! published the module, and recorded it. A watch takes it too, without waiting, while it reads
//! a `dist` it may trust at start.
//!
//! On macOS and Linux the lock is an exclusive `flock` of the file, and the build inherits it:
//! the build and what it starts (`cargo`, `rustc`) hold the same lock as the server. A server
//! that is killed lets go of its hold, but its build, which keeps running in a process group of
//! its own, still holds the lock until it is over, and another build of that Function waits.
//!
//! What holds the lock writes in the file who it is:
//!
//! - `held <pid> <token>`: the server, when it takes it;
//! - `group <pgid>`: the build, as it starts, its process group;
//! - `released`: the server, when it lets go.
//!
//! A process that a build starts and that outlives it (a compiler cache's server, such as
//! sccache) inherits the lock too, and would hold it as long as it runs. A lock held this way is
//! not waited for: its server said `released`, or it is gone and so is its build's process group.
//! It is replaced by a new file; the old one stays with what holds it. The servers replace it,
//! and take it, under a second lock that no build inherits (`.guard`), so that two of them never
//! hold two files each.
//!
//! On Windows a build cannot be handed the lock: the server alone holds it. A build that a
//! killed server left running is not waited for there.

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use crate::checkout::encoded_handle;

/// The write locks of the Functions of an app.
#[derive(Clone, Debug)]
pub(super) struct Locks {
    /// `<record>/locks`. `None`: nothing is recorded, nothing is locked.
    dir: Option<PathBuf>,
}

/// The write lock of a Function, held by this server. Let go when dropped.
pub(super) struct Lock {
    file: File,
    token: String,
    /// Let go without saying so, as a server that is killed does: for the tests.
    #[cfg(test)]
    silent: bool,
}

/// Another build of the Function holds its lock.
#[derive(Debug)]
pub(super) struct Busy;

/// The tokens of the locks this process holds: what says that a lock with this process's id in
/// it is still held, when two watchers of one process share an app (the tests).
static HELD: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Makes the tokens of a process its own.
static TOKENS: AtomicU64 = AtomicU64::new(0);

/// How often a build that waits for the lock tries again.
const RETRY: Duration = Duration::from_millis(50);

impl Locks {
    pub(super) fn new(record: Option<&Path>) -> Locks {
        Locks {
            dir: record.map(|record| record.join("locks")),
        }
    }

    /// Takes the lock of `handle`, if no build of it holds it. `Ok(None)`: there is no lock to
    /// take (nothing is recorded, or the folder of the locks cannot be written).
    pub(super) fn try_take(&self, handle: &str) -> Result<Option<Lock>, Busy> {
        let Some(dir) = &self.dir else {
            return Ok(None);
        };
        if std::fs::create_dir_all(dir).is_err() {
            return Ok(None);
        }
        let path = dir.join(format!("{}.lock", encoded_handle(handle)));
        #[cfg(unix)]
        let _guard = guard(dir);
        // A second time once a lock no build holds was replaced.
        for _ in 0..2 {
            let Ok(file) = OpenOptions::new()
                .read(true)
                .append(true)
                .create(true)
                .open(&path)
            else {
                return Ok(None);
            };
            match file.try_lock() {
                Ok(()) => return Ok(Some(Lock::new(file))),
                Err(TryLockError::WouldBlock) => {
                    if !stale(&path) {
                        return Err(Busy);
                    }
                    // What holds it outlived its build: it keeps that file.
                    if std::fs::remove_file(&path).is_err() {
                        return Err(Busy);
                    }
                }
                // Locks are not to be had there.
                Err(TryLockError::Error(_)) => return Ok(None),
            }
        }
        Err(Busy)
    }

    /// Takes the lock of `handle`, waiting for the build of it that holds it to be over.
    /// `waiting` runs once if it has to wait. `None`: `stop` was set meanwhile.
    pub(super) fn take(
        &self,
        handle: &str,
        stop: &AtomicBool,
        waiting: impl FnOnce(),
    ) -> Option<Option<Lock>> {
        let mut waiting = Some(waiting);
        loop {
            if stop.load(Ordering::Relaxed) {
                return None;
            }
            match self.try_take(handle) {
                Ok(lock) => return Some(lock),
                Err(Busy) => {
                    if let Some(waiting) = waiting.take() {
                        waiting();
                    }
                    std::thread::sleep(RETRY);
                }
            }
        }
    }
}

impl Lock {
    fn new(mut file: File) -> Lock {
        let token = format!(
            "{}-{}",
            std::process::id(),
            TOKENS.fetch_add(1, Ordering::Relaxed)
        );
        HELD.lock().expect("locks poisoned").insert(token.clone());
        // What the last holder wrote is not this one's.
        let _ = file.set_len(0);
        let _ = writeln!(file, "held {} {token}", std::process::id());
        Lock {
            file,
            token,
            #[cfg(test)]
            silent: false,
        }
    }

    /// The descriptor the build inherits.
    #[cfg(unix)]
    pub(super) fn fd(&self) -> std::os::fd::RawFd {
        std::os::fd::AsRawFd::as_raw_fd(&self.file)
    }

    /// The lock's open file, shared: what a process the build started holds.
    #[cfg(test)]
    pub(super) fn share(&self) -> File {
        self.file.try_clone().expect("the lock's file")
    }

    /// Lets go of the lock as a server that is killed does: nothing said, the build (if it runs)
    /// still holding it.
    #[cfg(test)]
    pub(super) fn abandon(mut self) {
        self.silent = true;
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        #[cfg(test)]
        let silent = self.silent;
        #[cfg(not(test))]
        let silent = false;
        if !silent {
            let _ = writeln!(self.file, "released");
        }
        HELD.lock().expect("locks poisoned").remove(&self.token);
        // Closing it lets go, once what the build started is over too. Never `unlock`: that
        // would let go for the build as well.
    }
}

/// Run in the build's process, before its command: keeps the lock's descriptor open in it (and
/// in what it starts), and says its process group in the lock. Only what is safe between `fork`
/// and `exec`: no allocation, no lock.
#[cfg(unix)]
pub(super) fn inherit(fd: std::os::fd::RawFd) -> std::io::Result<()> {
    // SAFETY: `fcntl`, `getpgrp` and `write` are async-signal-safe; `fd` is open in the child,
    // as it was in the server when it forked.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut line = [0u8; 32];
        let prefix = b"group ";
        line[..prefix.len()].copy_from_slice(prefix);
        let mut len = prefix.len();
        let mut digits = [0u8; 20];
        let mut n = libc::getpgrp() as u64;
        let mut count = 0;
        loop {
            digits[count] = b'0' + (n % 10) as u8;
            count += 1;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        for digit in digits[..count].iter().rev() {
            line[len] = *digit;
            len += 1;
        }
        line[len] = b'\n';
        len += 1;
        // The lock was opened to append: this goes after `held`.
        libc::write(fd, line.as_ptr().cast(), len);
    }
    Ok(())
}

/// The lock that the servers of the app take while they take or replace a write lock. No build
/// inherits it: one that is killed lets go of it.
#[cfg(unix)]
fn guard(dir: &Path) -> Option<File> {
    let file = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(dir.join(".guard"))
        .ok()?;
    file.lock().ok()?;
    Some(file)
}

/// Whether what holds the lock at `path` is not a build of the Function: its server let go of
/// it, or the server and the build's process group are gone. What is left is something the
/// build started and that outlived it.
#[cfg(unix)]
fn stale(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let mut holder = None;
    let mut group = None;
    let mut released = false;
    for line in text.lines() {
        let mut words = line.split(' ');
        match words.next() {
            Some("held") => {
                holder = words
                    .next()
                    .and_then(|pid| pid.parse::<i32>().ok())
                    .zip(words.next().map(str::to_string));
            }
            Some("group") => group = words.next().and_then(|group| group.parse::<i32>().ok()),
            Some("released") => released = true,
            _ => {}
        }
    }
    if released {
        return true;
    }
    // No group said: the build is not started yet, or is starting (it says it first thing).
    let Some(group) = group else {
        return false;
    };
    let server = holder.is_some_and(|(pid, token)| {
        if pid == std::process::id() as i32 {
            HELD.lock().expect("locks poisoned").contains(&token)
        } else {
            alive(pid)
        }
    });
    !server && !alive(-group)
}

/// Whether the process `pid` (or, negative, a process of the group `-pid`) is there.
#[cfg(unix)]
fn alive(pid: i32) -> bool {
    if pid == 0 || pid == -1 || pid == i32::MIN {
        return false;
    }
    // SAFETY: signal 0 only asks whether the process exists.
    let found = unsafe { libc::kill(pid, 0) } == 0;
    found || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn stale(_: &Path) -> bool {
    false
}
