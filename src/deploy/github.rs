//! Cliente da API do GitHub — releases do `Chrispsz/OPL-Mod-PTBR`.
//!
//! O token NUNCA é gravado em arquivo: vem da env `GITHUB_TOKEN` ou
//! `--token` e vive só em memória do processo.

use crate::error::{Ps2Error, Result};
use serde::Deserialize;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub const DEFAULT_REPO: &str = "Chrispsz/OPL-Mod-PTBR";

#[derive(Debug, Clone, Deserialize)]
pub struct GhAsset {
    pub name: String,
    pub size: u64,
    pub browser_download_url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct GhRelease {
    pub tag_name: String,
    pub name: Option<String>,
    pub prerelease: bool,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub assets: Vec<GhAsset>,
}

pub struct GitHubClient {
    http: reqwest::Client,
    #[allow(dead_code)]
    token: Option<String>,
}

impl GitHubClient {
    pub fn new(token: Option<String>) -> Result<Self> {
        let mut builder = reqwest::Client::builder()
            .user_agent("ps2-manager/0.1")
            .timeout(Duration::from_secs(300));
        if let Some(t) = &token {
            use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
            let mut headers = HeaderMap::new();
            if let Ok(v) = HeaderValue::from_str(&format!("Bearer {t}")) {
                headers.insert(AUTHORIZATION, v);
            }
            builder = builder.default_headers(headers);
        }
        let http = builder.build()?;
        Ok(Self { http, token })
    }

    fn api(&self, path: &str) -> String {
        format!("https://api.github.com{path}")
    }

    pub async fn latest_release(&self, repo: &str) -> Result<GhRelease> {
        let url = self.api(&format!("/repos/{repo}/releases/latest"));
        let resp = self.http.get(&url).send().await?;
        // /releases/latest ignora prereleases — o OPL-Mod-PTBR marca todas
        // como prerelease, então caímos para a lista completa.
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            let list = self.list_releases(repo, 1).await?;
            return list
                .into_iter()
                .next()
                .ok_or_else(|| Ps2Error::Other(format!("{repo}: nenhuma release publicada")));
        }
        resp.error_for_status()?.json().await.map_err(Ps2Error::Http)
    }

    pub async fn list_releases(&self, repo: &str, per_page: u8) -> Result<Vec<GhRelease>> {
        let url = self.api(&format!("/repos/{repo}/releases?per_page={per_page}"));
        let resp = self.http.get(&url).send().await?;
        resp.error_for_status()?.json().await.map_err(Ps2Error::Http)
    }

    /// Download com streaming + progresso em bytes.
    pub async fn download(
        &self,
        url: &str,
        dest: &Path,
        progress: Option<Arc<AtomicU64>>,
    ) -> Result<u64> {
        let mut resp = self.http.get(url).send().await?.error_for_status()?;
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::File::create(dest).await.map_err(Ps2Error::Io)?;
        let mut total = 0u64;
        while let Some(chunk) = resp.chunk().await? {
            file.write_all(&chunk).await.map_err(Ps2Error::Io)?;
            total += chunk.len() as u64;
            if let Some(p) = &progress {
                p.fetch_add(chunk.len() as u64, Ordering::Relaxed);
            }
        }
        file.flush().await.map_err(Ps2Error::Io)?;
        Ok(total)
    }
}
