//! Low level, platform specific file-system helpers.
//!
//! * Windows: directories are enumerated with `GetFileInformationByHandleEx(FileIdBothDirectoryInfo)`,
//!   which returns name, size, mtime, attributes *and the NTFS file id* for a whole batch of entries in
//!   a single kernel call. The file id lets the engine detect renames/moves exactly.
//! * Linux: `getdents64` via `read_dir` + `lstat`; the inode number is used as the file id.
//! * Worker threads lower their own CPU and I/O priority so syncing never makes the desktop stutter.

use std::borrow::Cow;
use std::io;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct RawEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime_ns: i64,
    pub file_id: u64,
}

/// Prefix used for in-progress downloads. Always ignored by the scanner and watcher.
pub const TEMP_PREFIX: &str = ".~sync-";

pub fn is_temp_name(name: &str) -> bool {
    name.starts_with(TEMP_PREFIX)
}

/// Case folding used for path keys. Windows file systems are case-insensitive, so two names that
/// differ only by case refer to the same file and must map to the same key.
#[inline]
pub fn fold(s: &str) -> Cow<'_, str> {
    if cfg!(windows) && s.chars().any(|c| c.is_uppercase()) {
        Cow::Owned(s.to_lowercase())
    } else {
        Cow::Borrowed(s)
    }
}

/// Whether a remote name can be represented as a local file name on this platform.
pub fn valid_local_name(name: &str) -> bool {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
        return false;
    }
    if cfg!(windows) {
        if name.chars().any(|c| matches!(c, '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*') || (c as u32) < 32) {
            return false;
        }
        if name.ends_with('.') || name.ends_with(' ') {
            return false;
        }
        let stem = name.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
        const RESERVED: [&str; 22] = [
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1",
            "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
        ];
        if RESERVED.contains(&stem.as_str()) {
            return false;
        }
    }
    true
}

#[cfg(test)]
pub fn system_time_to_ns(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i64,
        Err(e) => -(e.duration().as_nanos() as i64),
    }
}

/// Joins a `/` separated relative path onto a base directory.
pub fn join_rel(base: &Path, rel: &str) -> PathBuf {
    let mut p = base.to_path_buf();
    for c in rel.split('/').filter(|c| !c.is_empty()) {
        p.push(c);
    }
    p
}

pub fn parent_key(key: &str) -> &str {
    match key.rfind('/') {
        Some(i) => &key[..i],
        None => "",
    }
}

pub fn last_component(key: &str) -> &str {
    match key.rfind('/') {
        Some(i) => &key[i + 1..],
        None => key,
    }
}

pub fn child_key(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

/// True when `key` equals `prefix` or lies inside it.
pub fn is_within(key: &str, prefix: &str) -> bool {
    prefix.is_empty() || key == prefix || (key.len() > prefix.len() && key.starts_with(prefix) && key.as_bytes()[prefix.len()] == b'/')
}

/// The half-open key range containing every strict descendant of `prefix` in a sorted map.
/// All keys starting with "prefix/" sort between "prefix/" and "prefix0" ('0' follows '/').
pub fn descendant_range(prefix: &str) -> (String, String) {
    if prefix.is_empty() {
        (String::new(), "\u{10FFFF}".to_string())
    } else {
        (format!("{prefix}/"), format!("{prefix}0"))
    }
}

// ---------------------------------------------------------------------------------------------
// Directory listing
// ---------------------------------------------------------------------------------------------

#[cfg(windows)]
mod win {
    use super::RawEntry;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_NO_MORE_FILES, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::*;

    const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;
    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
    /// 100ns ticks between 1601-01-01 and 1970-01-01.
    const EPOCH_DIFF: i64 = 116_444_736_000_000_000;

    pub fn filetime_to_ns(ft: i64) -> i64 {
        (ft - EPOCH_DIFF).saturating_mul(100)
    }

    /// Converts a path to a NUL terminated wide string, adding the `\\?\` prefix so paths longer
    /// than MAX_PATH work everywhere.
    pub fn wide(path: &Path) -> Vec<u16> {
        let s = path.as_os_str().to_string_lossy().replace('/', "\\");
        let full = if s.starts_with(r"\\?\") {
            s
        } else if let Some(unc) = s.strip_prefix(r"\\") {
            format!(r"\\?\UNC\{unc}")
        } else if s.len() >= 2 && s.as_bytes()[1] == b':' {
            format!(r"\\?\{s}")
        } else {
            s
        };
        std::ffi::OsStr::new(&full).encode_wide().chain(std::iter::once(0)).collect()
    }

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    fn open(path: &Path, access: u32) -> io::Result<Handle> {
        let w = wide(path);
        let h = unsafe {
            CreateFileW(
                w.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(Handle(h))
    }

    fn is_link(attrs: u32, reparse_tag: u32) -> bool {
        attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0 && (reparse_tag == IO_REPARSE_TAG_SYMLINK || reparse_tag == IO_REPARSE_TAG_MOUNT_POINT)
    }

    pub fn list_dir(path: &Path) -> io::Result<Vec<RawEntry>> {
        let h = open(path, FILE_LIST_DIRECTORY)?;
        // u64 backing store guarantees the 8 byte alignment the structure needs.
        let mut buf = vec![0u64; 64 * 1024 / 8];
        let byte_len = (buf.len() * 8) as u32;
        let mut out = Vec::new();
        let mut class = FileIdBothDirectoryRestartInfo;
        loop {
            let ok = unsafe { GetFileInformationByHandleEx(h.0, class, buf.as_mut_ptr().cast(), byte_len) };
            class = FileIdBothDirectoryInfo;
            if ok == 0 {
                let err = unsafe { GetLastError() };
                if err == ERROR_NO_MORE_FILES {
                    break;
                }
                return Err(io::Error::from_raw_os_error(err as i32));
            }
            let base = buf.as_ptr() as *const u8;
            let mut offset = 0usize;
            loop {
                // SAFETY: the kernel filled the buffer with a chain of FILE_ID_BOTH_DIR_INFO records.
                let info = unsafe { &*(base.add(offset) as *const FILE_ID_BOTH_DIR_INFO) };
                let name_len = info.FileNameLength as usize / 2;
                let name_ptr = std::ptr::addr_of!(info.FileName) as *const u16;
                let name16 = unsafe { std::slice::from_raw_parts(name_ptr, name_len) };
                let attrs = info.FileAttributes;
                if let Ok(name) = String::from_utf16(name16) {
                    if name != "." && name != ".." && !is_link(attrs, info.EaSize) {
                        let is_dir = attrs & FILE_ATTRIBUTE_DIRECTORY != 0;
                        out.push(RawEntry {
                            name,
                            is_dir,
                            size: if is_dir { 0 } else { info.EndOfFile as u64 },
                            mtime_ns: filetime_to_ns(info.LastWriteTime),
                            file_id: info.FileId as u64,
                        });
                    }
                }
                if info.NextEntryOffset == 0 {
                    break;
                }
                offset += info.NextEntryOffset as usize;
            }
        }
        Ok(out)
    }

    pub fn stat(path: &Path) -> io::Result<Option<RawEntry>> {
        let h = match open(path, FILE_READ_ATTRIBUTES) {
            Ok(h) => h,
            Err(e) if matches!(e.raw_os_error(), Some(2) | Some(3)) => return Ok(None),
            Err(e) => return Err(e),
        };
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(h.0, &mut info) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let is_dir = info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
        let ft = ((info.ftLastWriteTime.dwHighDateTime as i64) << 32) | info.ftLastWriteTime.dwLowDateTime as i64;
        Ok(Some(RawEntry {
            name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            is_dir,
            size: if is_dir { 0 } else { ((info.nFileSizeHigh as u64) << 32) | info.nFileSizeLow as u64 },
            mtime_ns: filetime_to_ns(ft),
            file_id: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
        }))
    }
}

#[cfg(unix)]
mod nix {
    use super::RawEntry;
    use std::io;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    fn entry(name: String, md: &std::fs::Metadata) -> RawEntry {
        let is_dir = md.is_dir();
        RawEntry {
            name,
            is_dir,
            size: if is_dir { 0 } else { md.len() },
            mtime_ns: md.mtime().saturating_mul(1_000_000_000).saturating_add(md.mtime_nsec()),
            file_id: md.ino(),
        }
    }

    pub fn list_dir(path: &Path) -> io::Result<Vec<RawEntry>> {
        let mut out = Vec::new();
        for de in std::fs::read_dir(path)? {
            let de = match de {
                Ok(d) => d,
                Err(_) => continue,
            };
            // d_type comes free with getdents64; skip symlinks without an extra syscall.
            match de.file_type() {
                Ok(ft) if ft.is_symlink() => continue,
                Ok(ft) if !(ft.is_file() || ft.is_dir()) => continue,
                Err(_) => continue,
                _ => {}
            }
            let name = match de.file_name().into_string() {
                Ok(n) => n,
                Err(_) => continue, // not valid UTF-8: cannot be represented in Drive
            };
            if let Ok(md) = de.metadata() {
                out.push(entry(name, &md));
            }
        }
        Ok(out)
    }

    pub fn stat(path: &Path) -> io::Result<Option<RawEntry>> {
        match std::fs::symlink_metadata(path) {
            Ok(md) if md.file_type().is_symlink() => Ok(None),
            Ok(md) => Ok(Some(entry(path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), &md))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(windows)]
pub use win::{list_dir, stat};
#[cfg(unix)]
pub use nix::{list_dir, stat};

// ---------------------------------------------------------------------------------------------
// File operations
// ---------------------------------------------------------------------------------------------

/// Clears the read-only attribute on Windows. Git marks every object file read-only, which makes
/// naive replace/delete operations fail with "access denied".
pub fn ensure_writable(path: &Path) {
    #[cfg(windows)]
    {
        if let Ok(md) = std::fs::symlink_metadata(path) {
            let mut perm = md.permissions();
            if perm.readonly() {
                #[allow(clippy::permissions_set_readonly_false)]
                perm.set_readonly(false);
                let _ = std::fs::set_permissions(path, perm);
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
    }
}

fn make_tree_writable(path: &Path) {
    #[cfg(windows)]
    {
        ensure_writable(path);
        if let Ok(rd) = std::fs::read_dir(path) {
            for e in rd.flatten() {
                let p = e.path();
                match e.file_type() {
                    Ok(t) if t.is_dir() => make_tree_writable(&p),
                    _ => ensure_writable(&p),
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
    }
}

/// Removes a file or directory, sending it to the recycle bin / freedesktop trash when requested.
pub fn remove_path(path: &Path, use_trash: bool) -> io::Result<()> {
    let md = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if use_trash {
        match trash::delete(path) {
            Ok(()) => return Ok(()),
            Err(e) => tracing::warn!("could not move {} to trash ({e}); deleting permanently", path.display()),
        }
    }
    if md.is_dir() {
        match std::fs::remove_dir_all(path) {
            Ok(()) => Ok(()),
            Err(_) => {
                make_tree_writable(path);
                std::fs::remove_dir_all(path)
            }
        }
    } else {
        ensure_writable(path);
        std::fs::remove_file(path)
    }
}

/// Atomically replaces `dest` with `tmp` (both on the same volume).
pub fn replace_file(tmp: &Path, dest: &Path) -> io::Result<()> {
    ensure_writable(dest);
    std::fs::rename(tmp, dest)
}

pub fn temp_path_for(dest: &Path) -> PathBuf {
    let r: u64 = rand::random();
    let dir = dest.parent().unwrap_or_else(|| Path::new("."));
    dir.join(format!("{TEMP_PREFIX}{r:016x}.tmp"))
}

/// Builds "name (conflict 2026-01-02 101500 HOST).ext".
pub fn conflict_name(name: &str) -> String {
    let host = gethostname::gethostname().to_string_lossy().into_owned();
    let stamp = chrono::Local::now().format("%Y-%m-%d %H%M%S");
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    format!("{stem} (conflict {stamp} {host}){ext}")
}

// ---------------------------------------------------------------------------------------------
// Priority
// ---------------------------------------------------------------------------------------------

/// Puts the calling thread into background mode: lower CPU priority and (more importantly) low
/// I/O priority so scanning and hashing never compete with interactive applications.
pub fn lower_current_thread_priority() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN};
        SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
    }
    #[cfg(target_os = "linux")]
    unsafe {
        const IOPRIO_WHO_PROCESS: libc::c_long = 1;
        const IOPRIO_CLASS_BE: libc::c_long = 2;
        let tid = libc::syscall(libc::SYS_gettid);
        // Best-effort class, lowest priority (7).
        libc::syscall(libc::SYS_ioprio_set, IOPRIO_WHO_PROCESS, tid, (IOPRIO_CLASS_BE << 13) | 7);
        libc::setpriority(libc::PRIO_PROCESS, tid as libc::id_t, 10);
    }
}

// ---------------------------------------------------------------------------------------------
// Autostart
// ---------------------------------------------------------------------------------------------

pub fn set_autostart(enabled: bool) -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Registry::{RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
        let w = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let key = w(r"Software\Microsoft\Windows\CurrentVersion\Run");
        let name = w("SchmeSync");
        unsafe {
            if enabled {
                let val = w(&format!("\"{}\" --minimized", exe.display()));
                let rc = RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), REG_SZ, val.as_ptr().cast(), (val.len() * 2) as u32);
                if rc != 0 {
                    anyhow::bail!("registry write failed ({rc})");
                }
            } else {
                RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr());
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        // Inside an AppImage the real executable lives on a temporary mount; $APPIMAGE is stable.
        let target = std::env::var("APPIMAGE").map(PathBuf::from).unwrap_or(exe);
        let dir = dirs::config_dir().ok_or_else(|| anyhow::anyhow!("no config dir"))?.join("autostart");
        let file = dir.join("schme16-sync.desktop");
        if enabled {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(
                &file,
                format!(
                    "[Desktop Entry]\nType=Application\nName=Sync\nComment=Google Drive sync\nExec=\"{}\" --minimized\nX-GNOME-Autostart-enabled=true\nTerminal=false\n",
                    target.display()
                ),
            )?;
        } else {
            let _ = std::fs::remove_file(file);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_and_keys() {
        assert!(is_within("a/b/c", "a/b"));
        assert!(!is_within("a/bc", "a/b"));
        assert!(is_within("a/b", "a/b"));
        assert!(is_within("x", ""));
        assert_eq!(parent_key("a/b/c"), "a/b");
        assert_eq!(parent_key("a"), "");
        let (lo, hi) = descendant_range("a/b");
        assert!("a/b/c".to_string() >= lo && "a/b/c".to_string() < hi);
        assert!(!("a/b.txt".to_string() >= lo && "a/b.txt".to_string() < hi));
    }

    #[test]
    fn conflict_names() {
        let n = conflict_name("report.pdf");
        assert!(n.starts_with("report (conflict ") && n.ends_with(").pdf"));
        let n = conflict_name(".bashrc");
        assert!(n.starts_with(".bashrc (conflict "));
    }

    #[test]
    fn list_and_stat() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("f.txt"), b"hello").unwrap();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        let mut l = list_dir(d.path()).unwrap();
        l.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].name, "f.txt");
        assert_eq!(l[0].size, 5);
        assert!(l[1].is_dir);
        let s = stat(&d.path().join("f.txt")).unwrap().unwrap();
        assert_eq!(s.size, 5);
        assert_eq!(s.file_id, l[0].file_id);
        assert_eq!(s.mtime_ns, l[0].mtime_ns);
        let std_mtime = system_time_to_ns(std::fs::metadata(d.path().join("f.txt")).unwrap().modified().unwrap());
        assert_eq!(std_mtime, s.mtime_ns);
        assert!(stat(&d.path().join("missing")).unwrap().is_none());
    }
}
