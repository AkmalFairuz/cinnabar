//! The skin library's files: the install's own directories on a desktop and, in a browser, which
//! has no file system, the page's storage (`launcher::browser_files`), binary files base64-encoded.
//! Each function matches the `std::fs` call it stands in for. Files a browser player picks to import
//! are staged in memory while the importer reads them, so an archive never fills page storage.

use std::io;
use std::path::Path;

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use std::io::{self, Read, Write};
    use std::path::Path;

    pub fn read_bounded(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(limit + 1)
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    pub fn len(path: &Path) -> io::Result<u64> {
        Ok(std::fs::metadata(path)?.len())
    }

    pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
        std::fs::write(path, bytes)
    }

    pub fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?
            .write_all(bytes)
    }

    pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }

    pub fn remove_file(path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }

    pub fn create_dir_all(path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)
    }

    pub fn exists(path: &Path) -> bool {
        path.exists()
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use launcher::browser_files;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::io;
    use std::path::{Path, PathBuf};

    thread_local! {
        /// Picked files waiting for the importer, by the path it reads them at.
        static STAGED: RefCell<HashMap<PathBuf, Vec<u8>>> = RefCell::new(HashMap::new());
    }

    fn staged<T>(path: &Path, read: impl FnOnce(&[u8]) -> T) -> Option<T> {
        STAGED.with(|staged| staged.borrow().get(path).map(|bytes| read(bytes)))
    }

    pub fn stage(path: PathBuf, bytes: Vec<u8>) {
        STAGED.with(|staged| staged.borrow_mut().insert(path, bytes));
    }

    pub fn clear_staged() {
        STAGED.with(|staged| staged.borrow_mut().clear());
    }

    /// Base64 inflates a file by a third; the stored text may be that much longer.
    fn encoded_limit(limit: u64) -> u64 {
        limit.saturating_add(2).saturating_mul(4) / 3 + 4
    }

    fn decode(text: &[u8]) -> io::Result<Vec<u8>> {
        STANDARD
            .decode(text)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "a browser skin file is damaged"))
    }

    pub fn read_bounded(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
        let limit_usize = usize::try_from(limit + 1).unwrap_or(usize::MAX);
        if let Some(bytes) = staged(path, |bytes| bytes[..bytes.len().min(limit_usize)].to_vec()) {
            return Ok(bytes);
        }
        let mut bytes = decode(&browser_files::read(path, encoded_limit(limit))?)?;
        bytes.truncate(usize::try_from(limit + 1).unwrap_or(usize::MAX));
        Ok(bytes)
    }

    pub fn len(path: &Path) -> io::Result<u64> {
        if let Some(len) = staged(path, |bytes| bytes.len() as u64) {
            return Ok(len);
        }
        Ok(decode(&browser_files::read(path, u64::MAX)?)?.len() as u64)
    }

    pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
        browser_files::write(path, STANDARD.encode(bytes).as_bytes())
    }

    pub fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
        if exists(path) {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "the browser file exists"));
        }
        write(path, bytes)
    }

    pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
        let text = browser_files::read(from, u64::MAX)?;
        browser_files::write(to, &text)?;
        browser_files::remove(from)
    }

    pub fn remove_file(path: &Path) -> io::Result<()> {
        if !exists(path) {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no such browser file"));
        }
        browser_files::remove(path)
    }

    /// Browser storage has no directories.
    pub fn create_dir_all(_path: &Path) -> io::Result<()> {
        Ok(())
    }

    pub fn exists(path: &Path) -> bool {
        staged(path, |_| ()).is_some() || browser_files::read(path, u64::MAX).is_ok()
    }
}

/// Holds a picked file at `path` for the importer until [`clear_staged`].
#[cfg(target_arch = "wasm32")]
pub fn stage(path: std::path::PathBuf, bytes: Vec<u8>) {
    imp::stage(path, bytes);
}

/// Forgets every staged picked file.
#[cfg(target_arch = "wasm32")]
pub fn clear_staged() {
    imp::clear_staged();
}

/// Reads at most `limit + 1` bytes of `path`, so a caller bounded by `limit` sees an oversized file.
pub fn read_bounded(path: impl AsRef<Path>, limit: u64) -> io::Result<Vec<u8>> {
    imp::read_bounded(path.as_ref(), limit)
}

/// The byte length of `path`.
pub fn len(path: impl AsRef<Path>) -> io::Result<u64> {
    imp::len(path.as_ref())
}

/// Replaces `path` with `bytes`.
pub fn write(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    imp::write(path.as_ref(), bytes.as_ref())
}

/// Creates `path` with `bytes`, failing with [`io::ErrorKind::AlreadyExists`] when it exists.
pub fn write_new(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    imp::write_new(path.as_ref(), bytes.as_ref())
}

/// Moves `from` to `to`, replacing it.
pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    imp::rename(from.as_ref(), to.as_ref())
}

/// Removes `path`, failing with [`io::ErrorKind::NotFound`] when it is absent.
pub fn remove_file(path: impl AsRef<Path>) -> io::Result<()> {
    imp::remove_file(path.as_ref())
}

/// Creates the directory `path` and its parents; a browser needs none.
pub fn create_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    imp::create_dir_all(path.as_ref())
}

/// Whether `path` holds a file.
pub fn exists(path: impl AsRef<Path>) -> bool {
    imp::exists(path.as_ref())
}
