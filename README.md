# Sync

A fast, low-resource Google Drive sync client for **Windows** and **Linux** (AppImage), built with Rust + Tauri.

It was built as a replacement for Insync. It keeps the parts that matter: `.gitignore`-style exclusions, one-way and two-way sync, and folders from outside the base directory. It also fixes the things that hurt:

| Problem | How Sync handles it |
| --- | --- |
| Base directory gets "lost" | The local root is checked before every pass. If it is missing (unplugged drive, renamed folder), the pair goes **on hold** and nothing is treated as deleted. Mass deletions above a threshold always wait for your confirmation. |
| Trouble with `.git` folders | Read-only git objects are handled (the read-only flag is cleared before replace/delete). Transient `*.lock` files are excluded by default. Bursts like `git checkout` are debounced into a single pass. Many small files transfer in parallel using single-request multipart uploads. |
| Folder replaced by a file with the same name (or vice versa) | Handled explicitly: the old item is trashed first, then the new one is created. No manual cleanup in Drive. |
| Slow, clunky | Incremental: native change notifications locally plus the Drive Changes feed remotely, so a pass with nothing changed costs one HTTP request. Files are hashed only when size and mtime are inconclusive. The UI is about 40 KB gzipped, and closing the window destroys the web view, so the background process stays tiny. |

## Features

- **Two-way, upload-only and download-only** sync per pair, with optional delete propagation
- **`.gitignore`-compatible exclusions**: global rules plus per-pair rules (pair rules win, `!` re-includes), a live "test a path" checker, and quick-add presets. Matching is case-insensitive on Windows.
- **Selective sync** tree: untick folders to exclude them (stored as anchored rules)
- **External folders**: graft any local folder (even on another drive) into a pair at a virtual path, and it syncs as if it lived there. Nothing is moved, linked or junctioned on disk.
- **Exact rename/move detection** on both sides: NTFS file IDs and inodes locally, Drive file IDs remotely. A moved folder is one API call, not a re-upload.
- **Conflict handling**: if both sides changed, the local copy is kept as `name (conflict <date> <host>).ext` and both versions survive
- **Safe deletes**: local deletions go to the Recycle Bin / freedesktop trash, and Drive deletions go to the Drive trash
- **Integrity**: downloads are MD5-verified against Drive's checksum, written to a temp file, then atomically renamed into place. Files that change mid-transfer are retried rather than recorded.
- Resumable uploads for large files, exponential backoff for rate limits, automatic token refresh
- Shared drives, multiple Google accounts, light/dark/auto themes, tray icon, launch at sign-in

## OS-specific tricks

**Windows**
- One recursive `ReadDirectoryChangesW` watch per root
- Directories are enumerated with `GetFileInformationByHandleEx(FileIdBothDirectoryInfo)`. Each kernel call returns a batch of entries with size, mtime, attributes and the 64-bit file ID, so no per-file `stat` is needed.
- Long paths (`\\?\`), reserved names (`CON`, `aux.txt`…), invalid characters and trailing dots are handled. Drive items that can't exist on Windows are skipped with a warning instead of failing the sync.
- Symlinks and junctions are skipped (no loops); cloud placeholder reparse points are not
- Worker threads use `THREAD_MODE_BACKGROUND_BEGIN` (low CPU *and* I/O priority)

**Linux**
- inotify watches are placed per directory, and **only on synced directories**. Excluded trees like `node_modules` are never watched, which keeps you far below `max_user_watches`. If the limit is ever hit, Sync falls back to shorter periodic rescans.
- `getdents64` `d_type` is used to skip symlinks without extra syscalls, and inodes are used for move detection
- Scanner and hasher threads use the best-effort/lowest `ioprio` class and `nice 10`
- Autostart uses `$APPIMAGE`, so it survives AppImage remounts

## Setup

1. Install from the [releases](https://github.com/schme16/sync/releases): the `.exe` (NSIS) or `.msi` on Windows, or the `.AppImage` on Linux (`chmod +x` it).
2. **Google API credentials.** Google requires desktop sync apps to use an OAuth client:
   1. Create a project in the [Google Cloud console](https://console.cloud.google.com/projectcreate).
   2. Enable the **Google Drive API**.
   3. Configure the consent screen (External) and add yourself as a test user, or publish it.
   4. Create an OAuth client of type **Desktop app**.
   5. In Sync, go to **Accounts → Google API credentials** and paste the client ID and secret.

   Release builds can bake default credentials in with the `SYNC_GOOGLE_CLIENT_ID` / `SYNC_GOOGLE_CLIENT_SECRET` repository secrets.
3. **Accounts → Add Google account**. This signs in through your browser (loopback + PKCE). The refresh token goes to Windows Credential Manager / Secret Service, with a private `0600` file as fallback.
4. **Sync pairs → New sync pair**: pick a local folder, a Drive folder and a direction.

### Migrating from Insync

Point a new pair at your existing Insync folder and the same Drive folder. On the first pass, files that exist on both sides are matched by MD5 and **adopted without transferring anything**; only real differences move. Stop Insync first.

## How it works

```
 local FS ──(native events, debounced)──▶ re-list dirty dirs ─┐
                                                              ├─▶ 3-way planner ─▶ executor ─▶ base snapshot (SQLite)
 Drive  ──(changes feed every N s)──────▶ remote tree mirror ─┘      (L vs B vs R)    (parallel)
```

For every path the planner compares **L** (local now), **R** (Drive now) and **B** (both sides at the last successful sync). That tells it which side changed, so deletions, renames, kind changes and conflicts are unambiguous. The planner is a pure function with unit tests covering the tricky cases: folder↔file swaps, edit-vs-delete, partial folder deletes, and one-way modes. State lives per pair in SQLite (WAL), outside the synced folder.

Google Docs/Sheets/Slides (native Google formats) can't be downloaded as files and are skipped.

## Building

Requirements: Rust (stable), Node 20+. On Linux also `libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libdbus-1-dev`.

```sh
npm install
npm run tauri dev                       # run in development
npm run tauri build -- --bundles nsis,msi          # Windows installers
npm run tauri build -- --bundles appimage,deb      # Linux
cargo test --manifest-path src-tauri/Cargo.toml    # engine tests
npm run dev                             # UI only, in a browser, with mock data
```

Pushing a `v*` tag builds Windows and Linux artifacts in GitHub Actions and attaches them to a draft release.

## Files

| | Windows | Linux |
| --- | --- | --- |
| Config | `%APPDATA%\schme16-sync\config.json` | `~/.config/schme16-sync/config.json` |
| Sync state | `%LOCALAPPDATA%\schme16-sync\pairs\*.db` | `~/.local/share/schme16-sync/pairs/*.db` |
| Logs (7 days) | `%LOCALAPPDATA%\schme16-sync\logs` | `~/.local/share/schme16-sync/logs` |

## License

MIT
