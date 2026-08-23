//! Settings → Account: optional cloud sign-in (ADR 0013).
//!
//! Local SQLite and the loopback API never need this. The session file is what
//! provider `platform` reads on the daemon; this module is the writer.

use std::path::{Path, PathBuf};
use std::time::Duration;

use iced::Task;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::shell;

pub const FILE: &str = "cloud.session.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Session {
    pub url: String,
    pub refresh_token: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub access_expires_at: i64,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub entitlement: String,
    #[serde(default)]
    pub is_admin: bool,
}

pub struct State {
    pub url: String,
    pub email: String,
    pub paste: String,
    pub session: Option<Session>,
    pub busy: bool,
    pub waiting: bool,
    pub error: Option<String>,
    pub notice: Option<String>,
    dir: PathBuf,
}

impl State {
    pub fn load(dir: &Path, cloud_url: &str) -> Self {
        let session = read_session(dir);
        let url = if !cloud_url.trim().is_empty() {
            cloud_url.trim().to_string()
        } else {
            session.as_ref().map(|s| s.url.clone()).unwrap_or_default()
        };
        Self {
            url,
            email: session.as_ref().map(|s| s.email.clone()).unwrap_or_default(),
            paste: String::new(),
            session,
            busy: false,
            waiting: false,
            error: None,
            notice: None,
            dir: dir.to_path_buf(),
        }
    }
}

pub fn session_path(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

pub fn read_session(dir: &Path) -> Option<Session> {
    let raw = std::fs::read_to_string(session_path(dir)).ok()?;
    let session: Session = serde_json::from_str(&raw).ok()?;
    if session.refresh_token.trim().is_empty() || session.url.trim().is_empty() {
        return None;
    }
    Some(session)
}

fn save_session(dir: &Path, session: &Session) -> Result<(), String> {
    let body = serde_json::to_string_pretty(session).map_err(|e| e.to_string())?;
    shell::write_atomic(&session_path(dir), &body).map_err(|e| e.to_string())
}

fn clear_session(dir: &Path) {
    let _ = std::fs::remove_file(session_path(dir));
}

/// Raw token, or a magic-link URL (`?token=` / `#/verify?token=`).
pub fn extract_magic_token(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(url) = reqwest::Url::parse(trimmed) {
        for (k, v) in url.query_pairs() {
            if k == "token" && !v.is_empty() {
                return Some(v.into_owned());
            }
        }
        if let Some(frag) = url.fragment() {
            let q = frag.split('?').nth(1).unwrap_or(frag);
            let dummy = format!("http://127.0.0.1/?{q}");
            if let Ok(u) = reqwest::Url::parse(&dummy) {
                for (k, v) in u.query_pairs() {
                    if k == "token" && !v.is_empty() {
                        return Some(v.into_owned());
                    }
                }
            }
        }
    }
    if trimmed.matches('.').count() == 2 {
        return None;
    }
    Some(trimmed.to_string())
}

#[derive(Debug, Clone)]
pub enum Message {
    UrlChanged(String),
    EmailChanged(String),
    PasteChanged(String),
    SendLink,
    SignedIn(Result<Session, String>),
    PasteVerify,
    SignOut,
    SignedOut(Result<(), String>),
    Dismiss,
}

pub fn update(state: &mut State, msg: Message) -> Task<Message> {
    match msg {
        Message::UrlChanged(v) => {
            state.url = v;
            Task::none()
        }
        Message::EmailChanged(v) => {
            state.email = v;
            Task::none()
        }
        Message::PasteChanged(v) => {
            state.paste = v;
            Task::none()
        }
        Message::Dismiss => {
            state.error = None;
            state.notice = None;
            Task::none()
        }
        Message::SendLink => {
            let url = state.url.trim().trim_end_matches('/').to_string();
            let email = state.email.trim().to_string();
            if url.is_empty() {
                state.error = Some("Enter the cloud URL first.".into());
                return Task::none();
            }
            if !email.contains('@') {
                state.error = Some("Enter the email the magic link should reach.".into());
                return Task::none();
            }
            state.busy = true;
            state.waiting = true;
            state.error = None;
            state.notice = Some(
                "Check your email and open the link. This window will sign in when it lands."
                    .into(),
            );
            Task::perform(sign_in_with_callback(url, email), Message::SignedIn)
        }
        Message::SignedIn(Ok(session)) => {
            state.busy = false;
            state.waiting = false;
            state.paste.clear();
            if let Err(e) = save_session(&state.dir, &session) {
                state.error = Some(e);
                return Task::none();
            }
            state.session = Some(session.clone());
            state.url = session.url;
            state.email = session.email;
            state.notice = Some("Signed in. Pick Platform AI as the provider to use hosted models.".into());
            Task::none()
        }
        Message::SignedIn(Err(e)) => {
            state.busy = false;
            state.waiting = false;
            state.error = Some(e);
            Task::none()
        }
        Message::PasteVerify => {
            let url = state.url.trim().trim_end_matches('/').to_string();
            let Some(token) = extract_magic_token(&state.paste) else {
                state.error = Some("Paste the magic link, or the token from it.".into());
                return Task::none();
            };
            if url.is_empty() {
                state.error = Some("Enter the cloud URL first.".into());
                return Task::none();
            }
            state.busy = true;
            state.error = None;
            Task::perform(verify_token(url, token), Message::SignedIn)
        }
        Message::SignOut => {
            let url = state
                .session
                .as_ref()
                .map(|s| s.url.clone())
                .unwrap_or_else(|| state.url.trim().trim_end_matches('/').to_string());
            let refresh = state
                .session
                .as_ref()
                .map(|s| s.refresh_token.clone())
                .unwrap_or_default();
            state.busy = true;
            let dir = state.dir.clone();
            Task::perform(
                async move {
                    let _ = logout_remote(&url, &refresh).await;
                    clear_session(&dir);
                    Ok(())
                },
                Message::SignedOut,
            )
        }
        Message::SignedOut(result) => {
            state.busy = false;
            state.waiting = false;
            state.session = None;
            state.paste.clear();
            match result {
                Ok(()) => state.notice = Some("Signed out. Local work is unchanged.".into()),
                Err(e) => state.error = Some(e),
            }
            Task::none()
        }
    }
}

async fn sign_in_with_callback(cloud: String, email: String) -> Result<Session, String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("Could not listen for the sign-in callback: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| e.to_string())?
        .port();
    let redirect = format!("http://127.0.0.1:{port}/callback");
    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{}/accounts/api/v1/auth/magic-link", cloud.trim_end_matches('/')))
        .json(&json!({ "email": email, "redirect_uri": redirect }))
        .send()
        .await
        .map_err(|e| format!("Could not reach the cloud: {e}"))?;
    if !resp.status().is_success() {
        let body: Value = resp.json().await.unwrap_or(json!({}));
        let msg = body
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("The cloud refused the magic-link request.");
        return Err(msg.to_string());
    }
    let waited = tokio::time::timeout(Duration::from_secs(15 * 60), accept_callback(listener)).await;
    let (access, refresh) = match waited {
        Ok(Ok(pair)) => pair,
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err("Timed out waiting for the email link. Paste it below.".into()),
    };
    session_from_tokens(&cloud, access, refresh).await
}

async fn accept_callback(listener: tokio::net::TcpListener) -> Result<(String, String), String> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|e| format!("Callback accept failed: {e}"))?;
        let mut buf = vec![0u8; 4096];
        let n = stream.read(&mut buf).await.unwrap_or(0);
        let head = String::from_utf8_lossy(&buf[..n]);
        let line = head.lines().next().unwrap_or("");
        let path = line.split_whitespace().nth(1).unwrap_or("");
        let dummy = format!("http://127.0.0.1{path}");
        let parsed = reqwest::Url::parse(&dummy).ok();
        let mut access = String::new();
        let mut refresh = String::new();
        if let Some(url) = parsed {
            for (k, v) in url.query_pairs() {
                match k.as_ref() {
                    "access_token" => access = v.into_owned(),
                    "refresh_token" => refresh = v.into_owned(),
                    _ => {}
                }
            }
        }
        let ok = !access.is_empty() && !refresh.is_empty();
        let body = if ok {
            "<!doctype html><title>Signed in</title><p>You can close this tab and return to the app.</p>"
        } else {
            "<!doctype html><title>Sign-in failed</title><p>Missing tokens. Return to the app and paste the link.</p>"
        };
        let status = if ok { "200 OK" } else { "400 Bad Request" };
        let resp = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(resp.as_bytes()).await;
        if ok {
            return Ok((access, refresh));
        }
        // Browser prefetch or a probe — keep listening.
    }
}

async fn verify_token(cloud: String, token: String) -> Result<Session, String> {
    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{}/accounts/api/v1/auth/verify", cloud.trim_end_matches('/')))
        .json(&json!({ "token": token }))
        .send()
        .await
        .map_err(|e| format!("Could not reach the cloud: {e}"))?;
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(json!({}));
    if !status.is_success() {
        let msg = body
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("This link is invalid or has expired.");
        return Err(msg.to_string());
    }
    let access = body["access_token"]
        .as_str()
        .ok_or_else(|| "Cloud verify returned no access_token.".to_string())?
        .to_string();
    let refresh = body["refresh_token"]
        .as_str()
        .ok_or_else(|| "Cloud verify returned no refresh_token.".to_string())?
        .to_string();
    session_from_body(&cloud, body, access, refresh)
}

async fn session_from_tokens(cloud: &str, access: String, refresh: String) -> Result<Session, String> {
    let http = reqwest::Client::new();
    let resp = http
        .get(format!("{}/accounts/api/v1/me", cloud.trim_end_matches('/')))
        .bearer_auth(&access)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let body: Value = resp.json().await.unwrap_or(json!({}));
    session_from_body(cloud, json!({ "user": body, "expires_in": 15 * 60 }), access, refresh)
}

fn session_from_body(cloud: &str, body: Value, access: String, refresh: String) -> Result<Session, String> {
    let user = body.get("user").cloned().unwrap_or(body.clone());
    let expires_in = body["expires_in"].as_i64().unwrap_or(15 * 60);
    Ok(Session {
        url: cloud.trim_end_matches('/').to_string(),
        refresh_token: refresh,
        access_token: access,
        access_expires_at: chrono::Utc::now().timestamp() + expires_in,
        email: user["email"].as_str().unwrap_or_default().to_string(),
        entitlement: user["entitlement"].as_str().unwrap_or_default().to_string(),
        is_admin: user["is_admin"].as_bool().unwrap_or(false),
    })
}

async fn logout_remote(cloud: &str, refresh: &str) -> Result<(), String> {
    if cloud.is_empty() || refresh.is_empty() {
        return Ok(());
    }
    let http = reqwest::Client::new();
    let _ = http
        .post(format!("{}/accounts/api/v1/auth/logout", cloud.trim_end_matches('/')))
        .json(&json!({ "refresh_token": refresh }))
        .send()
        .await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_accepts_a_raw_hex_token() {
        let t = "aabbccddeeff00112233445566778899";
        assert_eq!(extract_magic_token(t).as_deref(), Some(t));
    }

    #[test]
    fn extract_reads_query_and_hash_links() {
        assert_eq!(
            extract_magic_token("https://api.example.com/accounts/api/v1/auth/verify?token=abc123")
                .as_deref(),
            Some("abc123")
        );
        assert_eq!(
            extract_magic_token("https://api.example.com/accounts/#/verify?token=abc123").as_deref(),
            Some("abc123")
        );
    }

    #[test]
    fn extract_rejects_a_jwt() {
        assert!(extract_magic_token("aaa.bbb.ccc").is_none());
        assert!(extract_magic_token("").is_none());
    }
}
