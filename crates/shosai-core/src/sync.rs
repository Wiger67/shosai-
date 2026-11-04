//! Nextcloud（WebDAV）雙向同步 — 實驗性功能，需啟用 `nextcloud` feature。
//!
//! 設定檔放在書庫的 `.shosai/nextcloud.toml`：
//!
//! ```toml
//! server = "https://cloud.example.com"   # Nextcloud 網址（可含子路徑）
//! user = "alice"
//! app_password = "xxxxx-xxxxx-xxxxx"      # 建議使用「應用程式密碼」；也可改用環境變數 SHOSAI_NC_PASSWORD
//! remote_dir = "Shosai"                   # 同步到雲端的哪個資料夾
//! insecure = false                        # homelab 自簽憑證可設為 true
//! ```
//!
//! 同步規則（以上次同步狀態 `.shosai/sync-state.toml` 為基準）：
//! - 只有一邊改過 → 以改過的那邊為準
//! - 兩邊都改過 → 保留兩份：雲端版本另存為「檔名 (衝突 時間).副檔名」
//! - 一邊刪除、另一邊沒改 → 刪除另一邊（本機移到 `.shosai-trash`，雲端進 Nextcloud 回收桶）

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use ureq::http;

use crate::export::html::base64;
use crate::export::percent_decode;
use crate::{Error, Result};

const CONFIG_DIR: &str = ".shosai";
const CONFIG_FILE: &str = "nextcloud.toml";
const STATE_FILE: &str = "sync-state.toml";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    pub server: String,
    pub user: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub app_password: String,
    #[serde(default = "default_remote_dir")]
    pub remote_dir: String,
    #[serde(default)]
    pub insecure: bool,
}

fn default_remote_dir() -> String {
    "Shosai".into()
}

impl SyncConfig {
    pub fn path(library: &Path) -> PathBuf {
        library.join(CONFIG_DIR).join(CONFIG_FILE)
    }

    pub fn load(library: &Path) -> Result<SyncConfig> {
        let path = Self::path(library);
        let text = fs::read_to_string(&path)
            .map_err(|_| Error::Sync(format!("找不到設定檔 {}（請參考 README 建立）", path.display())))?;
        let mut cfg: SyncConfig = toml::from_str(&text).map_err(|e| Error::Toml(e.to_string()))?;
        if let Ok(pw) = std::env::var("SHOSAI_NC_PASSWORD") {
            cfg.app_password = pw;
        }
        if cfg.app_password.is_empty() {
            return Err(Error::Sync("沒有設定密碼（app_password 或環境變數 SHOSAI_NC_PASSWORD）".into()));
        }
        Ok(cfg)
    }

    pub fn save(&self, library: &Path) -> Result<()> {
        let path = Self::path(library);
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(path, toml::to_string_pretty(self).map_err(|e| Error::Toml(e.to_string()))?)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// WebDAV 用戶端

#[derive(Debug, Clone, PartialEq)]
pub struct RemoteEntry {
    /// 相對於同步資料夾的路徑（`/` 分隔）
    pub rel: String,
    pub is_dir: bool,
    pub etag: String,
    pub size: u64,
}

pub struct WebDav {
    agent: ureq::Agent,
    /// 例：https://cloud.example.com/remote.php/dav/files/alice/Shosai/
    base_url: String,
    /// base_url 的路徑部分（已解碼），用來把 href 轉回相對路徑
    base_path: String,
    /// 使用者檔案根目錄，例：https://cloud.example.com/remote.php/dav/files/alice/
    user_url: String,
    remote_dir: Vec<String>,
    auth: String,
}

impl WebDav {
    pub fn new(cfg: &SyncConfig) -> WebDav {
        let server = cfg.server.trim_end_matches('/');
        let user_url = format!("{server}/remote.php/dav/files/{}/", encode_segment(&cfg.user));
        let remote_dir: Vec<String> = cfg.remote_dir.split('/').filter(|s| !s.is_empty()).map(String::from).collect();
        let encoded: Vec<String> = remote_dir.iter().map(|s| encode_segment(s)).collect();
        let base_url = if encoded.is_empty() { user_url.clone() } else { format!("{user_url}{}/", encoded.join("/")) };
        let base_path = percent_decode(&url_path(&base_url));

        let mut builder = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(120)))
            .http_status_as_error(false)
            .allow_non_standard_methods(true);
        if cfg.insecure {
            builder = builder.tls_config(ureq::tls::TlsConfig::builder().disable_verification(true).build());
        }
        WebDav {
            agent: builder.build().new_agent(),
            base_url,
            base_path,
            user_url,
            remote_dir,
            auth: format!("Basic {}", base64(format!("{}:{}", cfg.user, cfg.app_password).as_bytes())),
        }
    }

    fn url(&self, rel: &str) -> String {
        let enc: Vec<String> = rel.split('/').filter(|s| !s.is_empty()).map(encode_segment).collect();
        format!("{}{}", self.base_url, enc.join("/"))
    }

    fn request(
        &self,
        method: &str,
        rel: &str,
        depth: Option<&str>,
        body: Vec<u8>,
    ) -> Result<(u16, http::HeaderMap, Vec<u8>)> {
        self.request_url(method, &self.url(rel), rel, depth, body)
    }

    fn request_url(
        &self,
        method: &str,
        url: &str,
        rel: &str,
        depth: Option<&str>,
        body: Vec<u8>,
    ) -> Result<(u16, http::HeaderMap, Vec<u8>)> {
        let mut req = http::Request::builder()
            .method(http::Method::from_bytes(method.as_bytes()).map_err(|e| Error::Sync(e.to_string()))?)
            .uri(url)
            .header("Authorization", &self.auth);
        if let Some(d) = depth {
            req = req.header("Depth", d).header("Content-Type", "application/xml; charset=utf-8");
        }
        let req = req.body(body).map_err(|e| Error::Sync(e.to_string()))?;
        let mut resp = self.agent.run(req).map_err(|e| Error::Sync(format!("{method} {rel}: {e}")))?;
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let bytes = resp
            .body_mut()
            .with_config()
            .limit(512 * 1024 * 1024)
            .read_to_vec()
            .map_err(|e| Error::Sync(e.to_string()))?;
        if status == 401 {
            return Err(Error::Sync("帳號或密碼錯誤（401）".into()));
        }
        Ok((status, headers, bytes))
    }

    /// 列出某資料夾（Depth: 1）。資料夾不存在時回傳 `None`。
    pub fn list_dir(&self, rel: &str) -> Result<Option<Vec<RemoteEntry>>> {
        const BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:"><d:prop><d:getetag/><d:getcontentlength/><d:resourcetype/></d:prop></d:propfind>"#;
        let (status, _, body) = self.request("PROPFIND", rel, Some("1"), BODY.as_bytes().to_vec())?;
        if status == 404 {
            return Ok(None);
        }
        if status != 207 {
            return Err(Error::Sync(format!("PROPFIND {rel} 失敗：HTTP {status}")));
        }
        let xml = String::from_utf8_lossy(&body);
        let self_rel = rel.trim_matches('/');
        let mut out = Vec::new();
        for resp in elements(&xml, "response") {
            let Some(href) = elements(resp, "href").first().map(|h| percent_decode(h.trim())) else { continue };
            let href_path = url_path(&href);
            let Some(rel_path) = href_path.strip_prefix(&self.base_path) else { continue };
            let rel_path = rel_path.trim_matches('/').to_string();
            if rel_path == self_rel {
                continue;
            }
            let is_dir = elements(resp, "resourcetype").first().is_some_and(|r| !elements(r, "collection").is_empty());
            let etag = elements(resp, "getetag")
                .first()
                .map(|e| xml_unescape(e).trim().trim_matches('"').to_string())
                .unwrap_or_default();
            let size = elements(resp, "getcontentlength").first().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
            out.push(RemoteEntry { rel: rel_path, is_dir, etag, size });
        }
        Ok(Some(out))
    }

    /// 遞迴列出所有檔案。
    pub fn list_all(&self) -> Result<Option<Vec<RemoteEntry>>> {
        let Some(top) = self.list_dir("")? else { return Ok(None) };
        let mut files = Vec::new();
        let mut queue = top;
        while let Some(e) = queue.pop() {
            if e.is_dir {
                if !is_ignored(&e.rel) {
                    queue.extend(self.list_dir(&e.rel)?.unwrap_or_default());
                }
            } else if !is_ignored(&e.rel) {
                files.push(e);
            }
        }
        Ok(Some(files))
    }

    pub fn get(&self, rel: &str) -> Result<Vec<u8>> {
        let (status, _, body) = self.request("GET", rel, None, Vec::new())?;
        if !(200..300).contains(&status) {
            return Err(Error::Sync(format!("下載 {rel} 失敗：HTTP {status}")));
        }
        Ok(body)
    }

    /// 上傳，回傳新的 ETag（若伺服器有提供）。
    pub fn put(&self, rel: &str, data: Vec<u8>) -> Result<Option<String>> {
        let (status, headers, _) = self.request("PUT", rel, None, data)?;
        if !(200..300).contains(&status) {
            return Err(Error::Sync(format!("上傳 {rel} 失敗：HTTP {status}")));
        }
        let etag = headers
            .get("oc-etag")
            .or_else(|| headers.get("etag"))
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim_matches('"').to_string());
        Ok(etag)
    }

    pub fn mkcol(&self, rel: &str) -> Result<()> {
        let (status, _, _) = self.request("MKCOL", rel, None, Vec::new())?;
        // 201 建立成功；405 已存在
        if status == 201 || status == 405 {
            Ok(())
        } else {
            Err(Error::Sync(format!("建立資料夾 {rel} 失敗：HTTP {status}")))
        }
    }

    pub fn delete(&self, rel: &str) -> Result<()> {
        let (status, _, _) = self.request("DELETE", rel, None, Vec::new())?;
        if (200..300).contains(&status) || status == 404 {
            Ok(())
        } else {
            Err(Error::Sync(format!("刪除 {rel} 失敗：HTTP {status}")))
        }
    }

    /// 確保同步根目錄存在（逐層建立）。
    fn ensure_root(&self) -> Result<()> {
        if self.list_dir("")?.is_some() {
            return Ok(());
        }
        let mut url = self.user_url.clone();
        for part in &self.remote_dir {
            url.push_str(&encode_segment(part));
            url.push('/');
            let (status, _, _) = self.request_url("MKCOL", &url, part, None, Vec::new())?;
            if status != 201 && status != 405 {
                return Err(Error::Sync(format!("無法建立雲端資料夾 {part}：HTTP {status}")));
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 同步

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct SyncState {
    #[serde(default)]
    files: BTreeMap<String, FileState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct FileState {
    etag: String,
    mtime: u64,
    size: u64,
}

#[derive(Debug, Default)]
pub struct SyncReport {
    pub uploaded: Vec<String>,
    pub downloaded: Vec<String>,
    pub deleted_local: Vec<String>,
    pub deleted_remote: Vec<String>,
    pub conflicts: Vec<String>,
    pub errors: Vec<String>,
}

impl fmt::Display for SyncReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let lines = [
            ("↑ 上傳", &self.uploaded),
            ("↓ 下載", &self.downloaded),
            ("✕ 本機刪除", &self.deleted_local),
            ("✕ 雲端刪除", &self.deleted_remote),
            ("⚠ 衝突（已保留兩份）", &self.conflicts),
            ("✗ 錯誤", &self.errors),
        ];
        let mut any = false;
        for (label, items) in lines {
            if !items.is_empty() {
                any = true;
                writeln!(f, "{label}（{}）", items.len())?;
                for i in items {
                    writeln!(f, "    {i}")?;
                }
            }
        }
        if !any {
            writeln!(f, "已是最新，沒有需要同步的檔案")?;
        }
        Ok(())
    }
}

struct LocalFile {
    path: PathBuf,
    mtime: u64,
    size: u64,
}

pub fn sync(library: &Path, cfg: &SyncConfig) -> Result<SyncReport> {
    let dav = WebDav::new(cfg);
    dav.ensure_root()?;

    let state_path = library.join(CONFIG_DIR).join(STATE_FILE);
    let mut state: SyncState =
        fs::read_to_string(&state_path).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default();

    let mut local = BTreeMap::new();
    scan_local(library, library, &mut local)?;
    let remote: BTreeMap<String, RemoteEntry> =
        dav.list_all()?.unwrap_or_default().into_iter().map(|e| (e.rel.clone(), e)).collect();

    let all: BTreeSet<String> = local.keys().chain(remote.keys()).chain(state.files.keys()).cloned().collect();
    let mut report = SyncReport::default();
    let mut made_dirs = HashSet::new();

    for rel in all {
        let l = local.get(&rel);
        let r = remote.get(&rel);
        let s = state.files.get(&rel).cloned();
        let result = (|| -> Result<()> {
            let local_changed = l.is_some_and(|l| s.as_ref().is_none_or(|s| s.mtime != l.mtime || s.size != l.size));
            let remote_changed = r.is_some_and(|r| s.as_ref().is_none_or(|s| s.etag != r.etag));
            match (l, r) {
                (None, None) => {
                    state.files.remove(&rel);
                }
                (Some(l), None) => {
                    if s.is_some() && !local_changed {
                        // 雲端已刪除，本機沒改 → 本機也移到回收桶
                        crate::library::trash(library, &l.path)?;
                        state.files.remove(&rel);
                        report.deleted_local.push(rel.clone());
                    } else {
                        let st = upload(&dav, &rel, l, &mut made_dirs)?;
                        state.files.insert(rel.clone(), st);
                        report.uploaded.push(rel.clone());
                    }
                }
                (None, Some(r)) => {
                    if s.is_some() && !remote_changed {
                        // 本機已刪除，雲端沒改 → 刪除雲端（Nextcloud 有回收桶）
                        dav.delete(&rel)?;
                        state.files.remove(&rel);
                        report.deleted_remote.push(rel.clone());
                    } else {
                        let st = download(&dav, library, &rel, r)?;
                        state.files.insert(rel.clone(), st);
                        report.downloaded.push(rel.clone());
                    }
                }
                (Some(l), Some(r)) => match (local_changed, remote_changed) {
                    (false, false) => {}
                    (true, false) => {
                        let st = upload(&dav, &rel, l, &mut made_dirs)?;
                        state.files.insert(rel.clone(), st);
                        report.uploaded.push(rel.clone());
                    }
                    (false, true) => {
                        let st = download(&dav, library, &rel, r)?;
                        state.files.insert(rel.clone(), st);
                        report.downloaded.push(rel.clone());
                    }
                    (true, true) => {
                        let remote_bytes = dav.get(&rel)?;
                        let local_bytes = fs::read(&l.path)?;
                        if remote_bytes != local_bytes {
                            // 衝突：雲端版本另存一份，再把本機版本上傳
                            let copy = conflict_name(&rel);
                            write_local(library, &copy, &remote_bytes)?;
                            let st = upload(&dav, &rel, l, &mut made_dirs)?;
                            state.files.insert(rel.clone(), st);
                            report.conflicts.push(format!("{rel} → 雲端版本存為 {copy}"));
                        } else {
                            state
                                .files
                                .insert(rel.clone(), FileState { etag: r.etag.clone(), mtime: l.mtime, size: l.size });
                        }
                    }
                },
            }
            Ok(())
        })();
        if let Err(e) = result {
            report.errors.push(format!("{rel}：{e}"));
        }
    }

    fs::create_dir_all(state_path.parent().unwrap())?;
    fs::write(&state_path, toml::to_string(&state).map_err(|e| Error::Toml(e.to_string()))?)?;
    Ok(report)
}

fn upload(dav: &WebDav, rel: &str, l: &LocalFile, made: &mut HashSet<String>) -> Result<FileState> {
    // 逐層建立雲端資料夾
    let parts: Vec<&str> = rel.split('/').collect();
    let mut acc = String::new();
    for p in &parts[..parts.len() - 1] {
        if !acc.is_empty() {
            acc.push('/');
        }
        acc.push_str(p);
        if made.insert(acc.clone()) {
            dav.mkcol(&acc)?;
        }
    }
    let data = fs::read(&l.path)?;
    let etag = match dav.put(rel, data)? {
        Some(e) => e,
        None => {
            let parent = rel.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
            dav.list_dir(parent)?
                .unwrap_or_default()
                .into_iter()
                .find(|e| e.rel == rel)
                .map(|e| e.etag)
                .unwrap_or_default()
        }
    };
    Ok(FileState { etag, mtime: l.mtime, size: l.size })
}

fn download(dav: &WebDav, library: &Path, rel: &str, r: &RemoteEntry) -> Result<FileState> {
    let data = dav.get(rel)?;
    let path = write_local(library, rel, &data)?;
    let meta = fs::metadata(&path)?;
    Ok(FileState { etag: r.etag.clone(), mtime: mtime_secs(&meta), size: meta.len() })
}

fn write_local(library: &Path, rel: &str, data: &[u8]) -> Result<PathBuf> {
    if rel.split('/').any(|p| p == ".." || p.is_empty()) {
        return Err(Error::Sync(format!("不安全的路徑：{rel}")));
    }
    let path = library.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // 先寫暫存檔再改名，避免寫到一半損毀
    let tmp = path.with_extension("shosai-tmp");
    fs::write(&tmp, data)?;
    fs::rename(&tmp, &path)?;
    Ok(path)
}

fn conflict_name(rel: &str) -> String {
    let stamp = crate::timestamp();
    let (stem, ext) = match rel.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && !e.contains('/') => (s, format!(".{e}")),
        _ => (rel, String::new()),
    };
    format!("{stem} (衝突 {stamp}){ext}")
}

fn is_ignored(rel: &str) -> bool {
    rel.split('/').any(|p| p.starts_with('.') || p == "target" || p == "node_modules") || rel.ends_with(".shosai-tmp")
}

fn mtime_secs(meta: &fs::Metadata) -> u64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)
}

fn scan_local(root: &Path, dir: &Path, out: &mut BTreeMap<String, LocalFile>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(root) else { continue };
        let rel = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
        if is_ignored(&rel) {
            continue;
        }
        let meta = entry.metadata()?;
        if meta.is_dir() {
            scan_local(root, &path, out)?;
        } else if meta.is_file() {
            out.insert(rel, LocalFile { path, mtime: mtime_secs(&meta), size: meta.len() });
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 小工具：URL 與 XML

fn encode_segment(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// 取出 URL 的路徑部分（若本來就是路徑則原樣回傳）。
fn url_path(url: &str) -> String {
    match url.find("://") {
        Some(i) => {
            let rest = &url[i + 3..];
            rest.find('/').map(|j| rest[j..].to_string()).unwrap_or_else(|| "/".into())
        }
        None => url.to_string(),
    }
}

fn xml_unescape(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// 找出所有「本地名稱」為 `name` 的元素內容（忽略命名空間前綴）。
fn elements<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(off) = xml[pos..].find('<') {
        let start = pos + off;
        let after = &xml[start + 1..];
        let tag_end = after.find(|c: char| c.is_whitespace() || c == '>' || c == '/').unwrap_or(after.len());
        let full = &after[..tag_end];
        pos = start + 1;
        if full.starts_with('/') || full.starts_with('?') || full.starts_with('!') {
            continue;
        }
        let local = full.rsplit(':').next().unwrap_or(full);
        if local != name {
            continue;
        }
        let Some(gt) = after.find('>') else { break };
        if after[..gt].ends_with('/') {
            out.push("");
            pos = start + 1 + gt + 1;
            continue;
        }
        let content_start = start + 1 + gt + 1;
        let close = format!("</{full}>");
        if let Some(end) = xml[content_start..].find(&close) {
            out.push(&xml[content_start..content_start + end]);
            pos = content_start + end + close.len();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns">
 <d:response><d:href>/nc/remote.php/dav/files/alice/Shosai/</d:href>
  <d:propstat><d:prop><d:getetag>&quot;abc&quot;</d:getetag><d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat></d:response>
 <d:response><d:href>/nc/remote.php/dav/files/alice/Shosai/%E6%9B%B8/</d:href>
  <d:propstat><d:prop><d:getetag>"d1"</d:getetag><d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat></d:response>
 <d:response><d:href>/nc/remote.php/dav/files/alice/Shosai/a%20b.md</d:href>
  <d:propstat><d:prop><d:getetag>"e1"</d:getetag><d:getcontentlength>42</d:getcontentlength><d:resourcetype/></d:prop></d:propstat></d:response>
</d:multistatus>"#;

    #[test]
    fn parse_multistatus() {
        let responses = elements(SAMPLE, "response");
        assert_eq!(responses.len(), 3);
        assert_eq!(elements(responses[2], "getcontentlength"), vec!["42"]);
        assert!(elements(responses[2], "collection").is_empty());
        assert_eq!(elements(responses[1], "collection").len(), 1);
    }

    #[test]
    fn urls() {
        let cfg = SyncConfig {
            server: "https://cloud.lan/nc/".into(),
            user: "alice".into(),
            app_password: "pw".into(),
            remote_dir: "Shosai".into(),
            insecure: false,
        };
        let dav = WebDav::new(&cfg);
        assert_eq!(dav.base_url, "https://cloud.lan/nc/remote.php/dav/files/alice/Shosai/");
        assert_eq!(dav.base_path, "/nc/remote.php/dav/files/alice/Shosai/");
        assert_eq!(
            dav.url("書/第 1 章.md"),
            "https://cloud.lan/nc/remote.php/dav/files/alice/Shosai/%E6%9B%B8/%E7%AC%AC%201%20%E7%AB%A0.md"
        );
    }

    #[test]
    fn conflict_names() {
        assert!(conflict_name("書/a.md").starts_with("書/a (衝突 "));
        assert!(conflict_name("書/a.md").ends_with(").md"));
        assert!(is_ignored(".shosai/sync-state.toml"));
        assert!(!is_ignored("書/a.md"));
    }
}
