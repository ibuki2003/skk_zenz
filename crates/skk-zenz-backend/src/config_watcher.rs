use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use anyhow::{Context, Result};

pub(crate) struct ConfigWatcher {
    fd: OwnedFd,
    filename: Vec<u8>,
}

impl ConfigWatcher {
    pub(crate) fn new(path: &Path) -> Result<Self> {
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let parent = CString::new(parent.as_os_str().as_bytes())
            .context("設定ディレクトリのパスにNULが含まれています")?;
        // SAFETY: This call has no pointer arguments.
        let raw_fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if raw_fd < 0 {
            return Err(io::Error::last_os_error()).context("inotifyを初期化できません");
        }
        // SAFETY: raw_fd is uniquely owned after a successful inotify_init1.
        let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
        let mask = libc::IN_CLOSE_WRITE | libc::IN_MOVED_TO | libc::IN_CREATE | libc::IN_Q_OVERFLOW;
        // SAFETY: parent is NUL-terminated and fd is a live inotify descriptor.
        if unsafe { libc::inotify_add_watch(fd.as_raw_fd(), parent.as_ptr(), mask) } < 0 {
            return Err(io::Error::last_os_error())
                .with_context(|| format!("設定ディレクトリを監視できません: {}", path.display()));
        }
        Ok(Self {
            fd,
            filename: path
                .file_name()
                .context("設定ファイル名がありません")?
                .as_bytes()
                .to_vec(),
        })
    }

    pub(crate) fn fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    pub(crate) fn take_changed(&self) -> Result<bool> {
        let mut buffer = [0_u8; 4096];
        let mut changed = false;
        loop {
            // SAFETY: buffer is writable for its full length and fd is nonblocking.
            let length = unsafe {
                libc::read(
                    self.fd.as_raw_fd(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                )
            };
            if length < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::WouldBlock {
                    return Ok(changed);
                }
                return Err(error).context("inotifyイベントを読めません");
            }
            if length == 0 {
                return Ok(changed);
            }
            let mut offset = 0;
            let length = length as usize;
            while offset + size_of::<libc::inotify_event>() <= length {
                // SAFETY: Bounds were checked; inotify records need not be aligned.
                let event = unsafe {
                    buffer
                        .as_ptr()
                        .add(offset)
                        .cast::<libc::inotify_event>()
                        .read_unaligned()
                };
                let name_start = offset + size_of::<libc::inotify_event>();
                let name_end = (name_start + event.len as usize).min(length);
                let name = buffer[name_start..name_end]
                    .split(|byte| *byte == 0)
                    .next()
                    .unwrap_or_default();
                if event.mask & libc::IN_Q_OVERFLOW != 0 || name == self.filename {
                    changed = true;
                }
                offset = name_end;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn detects_only_the_config_filename() {
        let directory =
            std::env::temp_dir().join(format!("skk-zenz-config-watcher-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let config = directory.join("config.toml");
        fs::write(&config, "").unwrap();
        let watcher = ConfigWatcher::new(&config).unwrap();

        fs::write(directory.join("other.toml"), "ignored").unwrap();
        assert!(!watcher.take_changed().unwrap());
        fs::write(&config, "threads = 1").unwrap();
        assert!(watcher.take_changed().unwrap());

        fs::remove_dir_all(directory).unwrap();
    }
}
