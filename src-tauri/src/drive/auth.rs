//! OAuth 2.0 for installed apps: loopback redirect + PKCE, as recommended by Google.

use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub const SCOPE: &str = "https://www.googleapis.com/auth/drive";
const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: Option<u64>,
    refresh_token: Option<String>,
}

pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
}

fn random_b64(n: usize) -> String {
    let mut b = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut b);
    URL_SAFE_NO_PAD.encode(b)
}

const DONE_PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Sync</title>
<style>body{font-family:system-ui,sans-serif;display:grid;place-items:center;height:100vh;margin:0;background:#0f1115;color:#e7e9ee}
div{text-align:center}h1{font-weight:600;font-size:22px}p{opacity:.7}</style></head>
<body><div><h1>%TITLE%</h1><p>You can close this tab and return to Sync.</p></div></body></html>"#;

/// Runs the full browser consent flow. `open` is called with the URL the user must visit.
pub async fn authorize(http: &reqwest::Client, client_id: &str, client_secret: &str, open: impl FnOnce(&str)) -> Result<Tokens> {
    let listener = TcpListener::bind("127.0.0.1:0").await.context("binding loopback listener")?;
    let port = listener.local_addr()?.port();
    let redirect = format!("http://127.0.0.1:{port}");
    let verifier = random_b64(48);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_b64(16);

    let mut url = url::Url::parse(AUTH_URL)?;
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", &redirect)
        .append_pair("response_type", "code")
        .append_pair("scope", &format!("{SCOPE} openid email profile"))
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent")
        .append_pair("state", &state);
    open(url.as_str());

    let code = tokio::time::timeout(Duration::from_secs(300), async {
        loop {
            let (mut sock, _) = listener.accept().await?;
            let mut buf = vec![0u8; 8192];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
            let parsed = url::Url::parse(&format!("http://localhost{path}"))?;
            let q: std::collections::HashMap<String, String> = parsed.query_pairs().into_owned().collect();
            if q.is_empty() {
                let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                continue;
            }
            let ok = q.get("state") == Some(&state) && q.contains_key("code");
            let page = DONE_PAGE.replace("%TITLE%", if ok { "Account connected" } else { "Sign-in failed" });
            let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}", page.len());
            let _ = sock.write_all(resp.as_bytes()).await;
            if let Some(err) = q.get("error") {
                bail!("Google returned an error: {err}");
            }
            if q.get("state") != Some(&state) {
                bail!("OAuth state mismatch");
            }
            return q.get("code").cloned().ok_or_else(|| anyhow!("no authorization code"));
        }
    })
    .await
    .map_err(|_| anyhow!("timed out waiting for the browser sign-in"))??;

    let resp = http
        .post(TOKEN_URL)
        .form(&[
            ("code", code.as_str()),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("redirect_uri", redirect.as_str()),
            ("grant_type", "authorization_code"),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await?;
    if !resp.status().is_success() {
        bail!("token exchange failed: {}", resp.text().await.unwrap_or_default());
    }
    let t: TokenResponse = resp.json().await?;
    Ok(Tokens {
        access_token: t.access_token,
        refresh_token: t.refresh_token.ok_or_else(|| anyhow!("Google did not return a refresh token"))?,
        expires_in: t.expires_in.unwrap_or(3600),
    })
}

/// Holds an account's refresh token and hands out cached access tokens.
pub struct Auth {
    http: reqwest::Client,
    client_id: String,
    client_secret: String,
    refresh_token: String,
    cached: tokio::sync::Mutex<Option<(String, Instant)>>,
}

impl Auth {
    pub fn new(http: reqwest::Client, client_id: String, client_secret: String, refresh_token: String) -> Auth {
        Auth { http, client_id, client_secret, refresh_token, cached: tokio::sync::Mutex::new(None) }
    }

    pub fn with_access(self, access: String, expires_in: u64) -> Auth {
        *self.cached.try_lock().expect("fresh mutex") = Some((access, Instant::now() + Duration::from_secs(expires_in.saturating_sub(60))));
        self
    }

    pub async fn token(&self) -> Result<String> {
        let mut c = self.cached.lock().await;
        if let Some((tok, exp)) = c.as_ref() {
            if Instant::now() < *exp {
                return Ok(tok.clone());
            }
        }
        let resp = self
            .http
            .post(TOKEN_URL)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("refresh_token", self.refresh_token.as_str()),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await?;
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            if body.contains("invalid_grant") {
                bail!("the Google sign-in for this account has expired or was revoked; remove and re-add the account");
            }
            bail!("token refresh failed: {body}");
        }
        let t: TokenResponse = resp.json().await?;
        let exp = Instant::now() + Duration::from_secs(t.expires_in.unwrap_or(3600).saturating_sub(60));
        *c = Some((t.access_token.clone(), exp));
        Ok(t.access_token)
    }

    pub async fn invalidate(&self) {
        *self.cached.lock().await = None;
    }
}
