//! Minimal, fast Google Drive v3 client: partial-field responses, HTTP/2 connection reuse,
//! exponential backoff, resumable uploads and checksum-verified downloads.

use super::auth::Auth;
use crate::remote_tree::{RKind, RNode, FOLDER_MIME};
use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use md5::{Digest, Md5};
use reqwest::{Method, RequestBuilder, Response, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::{Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

const API: &str = "https://www.googleapis.com/drive/v3";
const UPLOAD: &str = "https://www.googleapis.com/upload/drive/v3/files";
pub const FIELDS: &str = "id,name,mimeType,md5Checksum,size,modifiedTime,parents,trashed,driveId";
const MULTIPART_LIMIT: u64 = 5 * 1024 * 1024;

#[derive(Debug)]
pub struct ApiError {
    pub status: u16,
    pub body: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = serde_json::from_str::<serde_json::Value>(&self.body)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| self.body.chars().take(200).collect());
        write!(f, "Drive API error {}: {}", self.status, msg)
    }
}
impl std::error::Error for ApiError {}

pub fn is_not_found(e: &anyhow::Error) -> bool {
    e.downcast_ref::<ApiError>().is_some_and(|a| a.status == 404)
}

pub fn is_network(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.downcast_ref::<reqwest::Error>().is_some_and(|r| r.is_connect() || r.is_timeout() || r.is_request()))
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct DriveFile {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mime_type: String,
    pub md5_checksum: Option<String>,
    pub size: Option<String>,
    pub modified_time: Option<String>,
    #[serde(default)]
    pub parents: Vec<String>,
    #[serde(default)]
    pub trashed: bool,
    pub drive_id: Option<String>,
}

impl DriveFile {
    pub fn mtime_ms(&self) -> i64 {
        self.modified_time
            .as_deref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|d| d.timestamp_millis())
            .unwrap_or(0)
    }
    pub fn size_u64(&self) -> u64 {
        self.size.as_deref().and_then(|s| s.parse().ok()).unwrap_or(0)
    }
    pub fn is_folder(&self) -> bool {
        self.mime_type == FOLDER_MIME
    }
    pub fn to_node(&self, parent: &str) -> RNode {
        RNode {
            id: self.id.clone(),
            parent: parent.to_string(),
            name: self.name.clone(),
            kind: RKind::from_mime(&self.mime_type),
            size: self.size_u64(),
            md5: self.md5_checksum.clone(),
            mtime_ms: self.mtime_ms(),
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub file_id: Option<String>,
    #[serde(default)]
    pub removed: bool,
    pub file: Option<DriveFile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileList {
    next_page_token: Option<String>,
    #[serde(default)]
    files: Vec<DriveFile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChangeList {
    next_page_token: Option<String>,
    new_start_page_token: Option<String>,
    #[serde(default)]
    changes: Vec<Change>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AboutUser {
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub email_address: String,
    #[serde(default)]
    pub photo_link: String,
    #[serde(default)]
    pub permission_id: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SharedDrive {
    pub id: String,
    pub name: String,
}

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("schme16-sync/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(90))
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(60))
        .gzip(true)
        .build()
        .expect("http client")
}

pub fn escape_q(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

#[derive(Clone)]
pub struct Drive {
    http: reqwest::Client,
    auth: Arc<Auth>,
}

impl Drive {
    pub fn new(http: reqwest::Client, auth: Arc<Auth>) -> Drive {
        Drive { http, auth }
    }

    /// Sends a request built by `make`, retrying with backoff on rate limits, 5xx and transient
    /// network errors, and transparently refreshing an expired access token once.
    async fn send(&self, make: impl Fn(&reqwest::Client) -> Result<RequestBuilder>) -> Result<Response> {
        let mut attempt = 0u32;
        let mut refreshed = false;
        loop {
            let token = self.auth.token().await?;
            match make(&self.http)?.bearer_auth(&token).send().await {
                Ok(resp) if resp.status().is_success() || resp.status() == StatusCode::PERMANENT_REDIRECT => return Ok(resp),
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let body = resp.text().await.unwrap_or_default();
                    if status == 401 && !refreshed {
                        self.auth.invalidate().await;
                        refreshed = true;
                        continue;
                    }
                    let rate = status == 429 || (status == 403 && (body.contains("ateLimitExceeded") || body.contains("quotaExceeded")));
                    if (rate || status >= 500) && attempt < 7 {
                        backoff(attempt).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(ApiError { status, body }.into());
                }
                Err(e) if (e.is_timeout() || e.is_connect() || e.is_request()) && attempt < 4 => {
                    backoff(attempt).await;
                    attempt += 1;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Like `send`, but also reads the body, re-sending the request when the connection drops
    /// mid-body. HTTP/2 connections are long-lived and can be torn down while a response is
    /// streaming (e.g. h2's GOAWAY "too_many_data_frames" guard tripping on Google's framing);
    /// a retry gets a fresh connection. Malformed JSON is not retried.
    async fn json<T: serde::de::DeserializeOwned>(&self, make: impl Fn(&reqwest::Client) -> Result<RequestBuilder>) -> Result<T> {
        let mut attempt = 0u32;
        loop {
            match self.send(&make).await?.bytes().await {
                Ok(body) => return Ok(serde_json::from_slice(&body)?),
                Err(e) if attempt < 4 => {
                    tracing::warn!("response body interrupted, retrying: {e}");
                    backoff(attempt).await;
                    attempt += 1;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    pub async fn about(&self) -> Result<AboutUser> {
        #[derive(Deserialize)]
        struct About {
            user: AboutUser,
        }
        let a: About = self.json(|h| Ok(h.get(format!("{API}/about")).query(&[("fields", "user(displayName,emailAddress,photoLink,permissionId)")]))).await?;
        Ok(a.user)
    }

    pub async fn get(&self, id: &str) -> Result<DriveFile> {
        self.json(|h| Ok(h.get(format!("{API}/files/{id}")).query(&[("fields", FIELDS), ("supportsAllDrives", "true")]))).await
    }

    async fn list(&self, q: &str, extra: &[(&str, &str)]) -> Result<Vec<DriveFile>> {
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        let fields = format!("nextPageToken,files({FIELDS})");
        loop {
            let page: FileList = self
                .json(|h| {
                    let mut r = h.get(format!("{API}/files")).query(&[
                        ("q", q),
                        ("fields", fields.as_str()),
                        ("pageSize", "1000"),
                        ("supportsAllDrives", "true"),
                        ("includeItemsFromAllDrives", "true"),
                    ]);
                    r = r.query(extra);
                    if let Some(t) = &token {
                        r = r.query(&[("pageToken", t.as_str())]);
                    }
                    Ok(r)
                })
                .await?;
            out.extend(page.files);
            match page.next_page_token {
                Some(t) => token = Some(t),
                None => break,
            }
        }
        Ok(out)
    }

    pub async fn list_children(&self, folder_id: &str) -> Result<Vec<DriveFile>> {
        self.list(&format!("'{}' in parents and trashed=false", escape_q(folder_id)), &[]).await
    }

    /// Lists every non-trashed file in My Drive or a shared drive (1000 per request), which is far
    /// cheaper than walking folders when syncing a whole drive.
    pub async fn list_all(&self, drive_id: Option<&str>) -> Result<Vec<DriveFile>> {
        match drive_id {
            Some(d) => self.list("trashed=false", &[("corpora", "drive"), ("driveId", d)]).await,
            // Items outside the sync root (e.g. "Shared with me") are pruned by the caller.
            None => self.list("trashed=false", &[("corpora", "user"), ("spaces", "drive")]).await,
        }
    }

    pub async fn list_folders(&self, parent: &str) -> Result<Vec<DriveFile>> {
        let mut v = self
            .list(&format!("'{}' in parents and mimeType='{FOLDER_MIME}' and trashed=false", escape_q(parent)), &[("orderBy", "name")])
            .await?;
        v.sort_by_key(|a| a.name.to_lowercase());
        Ok(v)
    }

    pub async fn shared_drives(&self) -> Result<Vec<SharedDrive>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct L {
            #[serde(default)]
            drives: Vec<SharedDrive>,
            next_page_token: Option<String>,
        }
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let l: L = self
                .json(|h| {
                    let mut r = h.get(format!("{API}/drives")).query(&[("pageSize", "100"), ("fields", "nextPageToken,drives(id,name)")]);
                    if let Some(t) = &token {
                        r = r.query(&[("pageToken", t.as_str())]);
                    }
                    Ok(r)
                })
                .await?;
            out.extend(l.drives);
            match l.next_page_token {
                Some(t) => token = Some(t),
                None => break,
            }
        }
        Ok(out)
    }

    pub async fn start_page_token(&self, drive_id: Option<&str>) -> Result<String> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct T {
            start_page_token: String,
        }
        let t: T = self
            .json(|h| {
                let mut r = h.get(format!("{API}/changes/startPageToken")).query(&[("supportsAllDrives", "true")]);
                if let Some(d) = drive_id {
                    r = r.query(&[("driveId", d)]);
                }
                Ok(r)
            })
            .await?;
        Ok(t.start_page_token)
    }

    /// Fetches all changes since `token`. Returns the changes and the token to use next time.
    pub async fn changes(&self, token: &str, drive_id: Option<&str>) -> Result<(Vec<Change>, String)> {
        let mut out = Vec::new();
        let mut tok = token.to_string();
        let fields = format!("nextPageToken,newStartPageToken,changes(fileId,removed,file({FIELDS}))");
        loop {
            let page: ChangeList = self
                .json(|h| {
                    let mut r = h.get(format!("{API}/changes")).query(&[
                        ("pageToken", tok.as_str()),
                        ("pageSize", "1000"),
                        ("includeRemoved", "true"),
                        ("supportsAllDrives", "true"),
                        ("includeItemsFromAllDrives", "true"),
                        ("spaces", "drive"),
                        ("fields", fields.as_str()),
                    ]);
                    if let Some(d) = drive_id {
                        r = r.query(&[("driveId", d)]);
                    }
                    Ok(r)
                })
                .await?;
            out.extend(page.changes);
            if let Some(next) = page.next_page_token {
                tok = next;
                continue;
            }
            return Ok((out, page.new_start_page_token.ok_or_else(|| anyhow!("changes response without a new start token"))?));
        }
    }

    pub async fn create_folder(&self, name: &str, parent: &str) -> Result<DriveFile> {
        let body = json!({ "name": name, "mimeType": FOLDER_MIME, "parents": [parent] });
        self.json(|h| Ok(h.post(format!("{API}/files")).query(&[("fields", FIELDS), ("supportsAllDrives", "true")]).json(&body))).await
    }

    /// Moves a file to the Drive trash (recoverable for 30 days). Missing files count as success.
    pub async fn trash(&self, id: &str) -> Result<()> {
        let body = json!({ "trashed": true });
        match self.send(|h| Ok(h.patch(format!("{API}/files/{id}")).query(&[("supportsAllDrives", "true"), ("fields", "id")]).json(&body))).await {
            Ok(_) => Ok(()),
            Err(e) if is_not_found(&e) => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub async fn move_rename(&self, id: &str, name: &str, add_parent: &str, remove_parent: &str) -> Result<DriveFile> {
        let body = json!({ "name": name });
        self.json(|h| {
            let mut r = h.patch(format!("{API}/files/{id}")).query(&[("fields", FIELDS), ("supportsAllDrives", "true")]).json(&body);
            if add_parent != remove_parent {
                r = r.query(&[("addParents", add_parent), ("removeParents", remove_parent)]);
            }
            Ok(r)
        })
        .await
    }

    /// Uploads `src`, creating a new file under `parent` or adding a new revision to `existing`
    /// (which keeps sharing settings and version history intact).
    pub async fn upload(
        &self,
        src: &Path,
        name: &str,
        parent: &str,
        existing: Option<&str>,
        mtime_ms: i64,
        size: u64,
        progress: &Arc<AtomicU64>,
    ) -> Result<DriveFile> {
        let mtime = chrono::DateTime::from_timestamp_millis(mtime_ms).unwrap_or_default().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let meta = match existing {
            Some(_) => json!({ "modifiedTime": mtime }),
            None => json!({ "name": name, "parents": [parent], "modifiedTime": mtime }),
        };
        let (method, url) = match existing {
            Some(id) => (Method::PATCH, format!("{UPLOAD}/{id}")),
            None => (Method::POST, UPLOAD.to_string()),
        };
        if size <= MULTIPART_LIMIT {
            let data = tokio::fs::read(src).await?;
            let boundary = format!("sync{:016x}", rand::random::<u64>());
            let mut body = Vec::with_capacity(data.len() + 512);
            body.extend_from_slice(format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{meta}\r\n--{boundary}\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
            body.extend_from_slice(&data);
            body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
            let body = bytes::Bytes::from(body);
            let f: DriveFile = self
                .json(|h| {
                    Ok(h.request(method.clone(), &url)
                        .query(&[("uploadType", "multipart"), ("fields", FIELDS), ("supportsAllDrives", "true")])
                        .header("Content-Type", format!("multipart/related; boundary={boundary}"))
                        .body(body.clone()))
                })
                .await?;
            progress.store(size, Ordering::Relaxed);
            return Ok(f);
        }

        // Resumable session for large files.
        let resp = self
            .send(|h| {
                Ok(h.request(method.clone(), &url)
                    .query(&[("uploadType", "resumable"), ("fields", FIELDS), ("supportsAllDrives", "true")])
                    .header("X-Upload-Content-Type", "application/octet-stream")
                    .header("X-Upload-Content-Length", size.to_string())
                    .json(&meta))
            })
            .await?;
        let session = resp
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| anyhow!("resumable upload did not return a session URL"))?
            .to_string();

        let mut offset = 0u64;
        for attempt in 0..8u32 {
            progress.store(offset, Ordering::Relaxed);
            let token = self.auth.token().await?;
            let mut file = std::fs::File::open(src)?;
            file.seek(SeekFrom::Start(offset))?;
            let file = tokio::fs::File::from_std(file);
            let counter = progress.clone();
            let stream = tokio_util::io::ReaderStream::with_capacity(file, 256 * 1024).map(move |chunk| {
                if let Ok(c) = &chunk {
                    counter.fetch_add(c.len() as u64, Ordering::Relaxed);
                }
                chunk
            });
            let res = self
                .http
                .put(&session)
                .bearer_auth(&token)
                .header("Content-Length", (size - offset).to_string())
                .header("Content-Range", format!("bytes {}-{}/{}", offset, size - 1, size))
                .body(reqwest::Body::wrap_stream(stream))
                .send()
                .await;
            match res {
                Ok(r) if r.status().is_success() => return Ok(r.json().await?),
                Ok(r) if r.status().as_u16() == 404 || r.status().as_u16() == 410 => {
                    return Err(anyhow!("upload session expired; it will be retried"));
                }
                Ok(r) if r.status().is_client_error() && r.status().as_u16() != 429 && r.status().as_u16() != 408 => {
                    let status = r.status().as_u16();
                    return Err(ApiError { status, body: r.text().await.unwrap_or_default() }.into());
                }
                _ => {
                    backoff(attempt).await;
                    offset = self.query_upload_offset(&session, size).await.unwrap_or(0);
                }
            }
        }
        Err(anyhow!("upload kept failing; it will be retried later"))
    }

    async fn query_upload_offset(&self, session: &str, size: u64) -> Result<u64> {
        let token = self.auth.token().await?;
        let r = self
            .http
            .put(session)
            .bearer_auth(token)
            .header("Content-Length", "0")
            .header("Content-Range", format!("bytes */{size}"))
            .send()
            .await?;
        if r.status().as_u16() == 308 {
            let next = r
                .headers()
                .get("range")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.rsplit('-').next())
                .and_then(|n| n.parse::<u64>().ok())
                .map(|n| n + 1)
                .unwrap_or(0);
            return Ok(next);
        }
        Ok(0)
    }

    /// Streams a file's content to `dest`, returning its MD5 so the caller can verify integrity.
    pub async fn download(&self, id: &str, dest: &Path, progress: &AtomicU64) -> Result<String> {
        let mut last_err = None;
        for attempt in 0..4u32 {
            progress.store(0, Ordering::Relaxed);
            match self.download_once(id, dest, progress).await {
                Ok(h) => return Ok(h),
                Err(e) if e.downcast_ref::<ApiError>().is_some_and(|a| a.status < 500 && a.status != 429) => return Err(e),
                Err(e) => {
                    last_err = Some(e);
                    backoff(attempt).await;
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow!("download failed")))
    }

    async fn download_once(&self, id: &str, dest: &Path, progress: &AtomicU64) -> Result<String> {
        let resp = self
            .send(|h| Ok(h.get(format!("{API}/files/{id}")).query(&[("alt", "media"), ("supportsAllDrives", "true"), ("acknowledgeAbuse", "true")])))
            .await?;
        let file = tokio::fs::File::create(dest).await?;
        let mut w = tokio::io::BufWriter::with_capacity(256 * 1024, file);
        let mut hasher = Md5::new();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            hasher.update(&chunk);
            w.write_all(&chunk).await?;
            progress.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        }
        w.flush().await?;
        w.into_inner().sync_all().await?;
        Ok(hex::encode(hasher.finalize()))
    }
}

async fn backoff(attempt: u32) {
    let base = 500u64 * (1 << attempt.min(6));
    let jitter = rand::random::<u64>() % 500;
    tokio::time::sleep(Duration::from_millis(base + jitter)).await;
}
