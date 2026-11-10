//! MD5 hashing (the checksum Google Drive exposes) on a dedicated low-priority thread pool.

use md5::{Digest, Md5};
use std::io::Read;
use std::path::Path;
use std::sync::OnceLock;

pub fn md5_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Md5::new();
    let mut buf = vec![0u8; 512 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

/// Shared worker pool for scanning and hashing. Threads run in background I/O mode.
pub fn pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8);
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .thread_name(|i| format!("sync-io-{i}"))
            .start_handler(|_| crate::fsutil::lower_current_thread_priority())
            .build()
            .expect("thread pool")
    })
}
