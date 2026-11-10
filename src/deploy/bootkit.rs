//! Boot Kit Essencial — pesquisa, download, validação (SHA256) e montagem
//! da pasta `/BOOT_MC/` com os ELFs mais atuais da cena PS2 (Mega Prompt 5).
//!
//! Duas camadas de conteúdo:
//!   * KIT ESSENCIAL (`KitKind::Boot`): `/BOOT_MC/` — cadeia de boot para o
//!     memory card (enxuto, manifest `bootkit.toml` embutido).
//!   * BIBLIOTECA (`KitKind::Library`): `/BIBLIOTECA/` — acervo opcional na
//!     mídia; copie ao MC só o que usar (manifest `library.toml` embutido).
//!
//! Fontes (releases oficiais, incluindo builds automáticos):
//!   - OPL:            `Chrispsz/OPL-Mod-PTBR` (Mod-Kit zip → OPNPS2LD.ELF)
//!   - OPL upstream:   `ps2homebrew/Open-PS2-Loader` (build "latest")
//!   - wLaunchELF:     `israpps/wLaunchELF_ISR` (BOOT-EXFAT-MX4SIO.ELF)
//!   - Memory Annih.:  `ffgriever-pl/Memory-Card-Annihilator` (mca-packed.elf)
//!   - PS2BBL:         `israpps/PlayStation2-Basic-BootLoader` (COMPRESSED + INI)
//!   - OpenTuna:       idem @ tag `opentuna` (caminho Fortuna p/ Slims finais)
//!   - GSM:            integrado ao OPL per-game (não é ELF separado hoje)
//!
//! O manifest padrão é embutido no binário (`include_str!`) e pode ser
//! substituído por `--manifest <arquivo>` no CLI.

use crate::deploy::github::{GhAsset, GhRelease, GitHubClient, DEFAULT_REPO};
use crate::error::{Ps2Error, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Manifest padrão do Kit Essencial (memory card).
pub const MANIFEST_TOML: &str = include_str!("../../bootkit.toml");

/// Manifest padrão da Biblioteca da Cena (acervo opcional na mídia).
pub const LIBRARY_TOML: &str = include_str!("../../library.toml");

/// Camada de deploy do kit — controla subdir e arquivo de auditoria.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KitKind {
    /// `/BOOT_MC/` — cadeia de boot transportada para o memory card.
    Boot,
    /// `/BIBLIOTECA/` — acervo opcional que permanece na mídia.
    Library,
}

impl KitKind {
    pub fn subdir(self) -> &'static str {
        match self {
            Self::Boot => "BOOT_MC",
            Self::Library => "BIBLIOTECA",
        }
    }

    pub fn info_file(self) -> &'static str {
        match self {
            Self::Boot => "BOOT-KIT-INFO.txt",
            Self::Library => "LEIA-ME.txt",
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct KitManifest {
    pub meta: KitMeta,
    #[serde(default)]
    pub assets: Vec<KitAsset>,
}

#[derive(Debug, Deserialize)]
pub struct KitMeta {
    pub kit_name: String,
    #[serde(default)]
    pub spec: String,
}

#[derive(Debug, Deserialize)]
pub struct KitAsset {
    pub id: String,
    /// Caminho relativo dentro de `/BOOT_MC/`.
    pub target: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub source: AssetSource,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind")]
pub enum AssetSource {
    #[serde(rename = "github_release")]
    GithubRelease {
        repo: String,
        #[serde(default)]
        tag: Option<String>,
        asset: String,
    },
    #[serde(rename = "github_zip")]
    GithubZip {
        repo: String,
        #[serde(default)]
        tag: Option<String>,
        asset: String,
        inner: String,
    },
    #[serde(rename = "github_7z")]
    Github7z {
        repo: String,
        #[serde(default)]
        tag: Option<String>,
        asset: String,
        inner: String,
    },
    #[serde(rename = "opl_ptbr_kit")]
    OplPtbrKit { inner: String },
    #[serde(rename = "builtin")]
    Builtin { template: String },
    /// Baixa o asset da release **sem extrair** (arquivo cru — usado pela
    /// Biblioteca para preservar instaladores multi-arquivo, ex.: OpenTuna).
    #[serde(rename = "github_raw")]
    GithubRaw {
        repo: String,
        #[serde(default)]
        tag: Option<String>,
        asset: String,
    },
}

#[derive(Debug)]
pub struct AssetReport {
    pub target: String,
    pub status: AssetStatus,
}

#[derive(Debug)]
pub enum AssetStatus {
    Ok { bytes: u64, sha256: String, source: String },
    Skipped(String),
    Failed(String),
}

#[derive(Debug)]
pub struct KitReport {
    pub kit_name: String,
    pub dest: PathBuf,
    pub assets: Vec<AssetReport>,
    pub total_bytes: u64,
}

fn default_true() -> bool {
    true
}

pub fn parse_manifest(s: &str) -> Result<KitManifest> {
    toml::from_str(s).map_err(|e| Ps2Error::Other(format!("manifest inválido: {e}")))
}

pub fn default_manifest() -> Result<KitManifest> {
    parse_manifest(MANIFEST_TOML)
}

pub fn default_library() -> Result<KitManifest> {
    parse_manifest(LIBRARY_TOML)
}

pub fn load_manifest(path: &Path) -> Result<KitManifest> {
    let s = std::fs::read_to_string(path).map_err(Ps2Error::Io)?;
    parse_manifest(&s)
}

/// Ajusta os toggles padrão do kit (usado pelo CLI `--no-*` e pela TUI [F3]).
pub fn apply_flags(m: &mut KitManifest, include_opl: bool, include_wle: bool, include_configs: bool) {
    for a in &mut m.assets {
        match a.id.as_str() {
            "opl_ptbr" => a.enabled = include_opl,
            "wlaunchelf" => a.enabled = include_wle,
            "boot_config" | "fremcb" => a.enabled = include_configs,
            _ => {}
        }
    }
}

pub fn source_desc(s: &AssetSource) -> String {
    match s {
        AssetSource::GithubRelease { repo, tag, asset } => match tag {
            Some(t) => format!("{repo}@{t} :: {asset}"),
            None => format!("{repo} :: {asset}"),
        },
        AssetSource::GithubZip { repo, tag, asset, inner } => match tag {
            Some(t) => format!("{repo}@{t} :: {asset} → {inner}"),
            None => format!("{repo} :: {asset} → {inner}"),
        },
        AssetSource::Github7z { repo, tag, asset, inner } => match tag {
            Some(t) => format!("{repo}@{t} :: {asset} → {inner}"),
            None => format!("{repo} :: {asset} → {inner}"),
        },
        AssetSource::OplPtbrKit { inner } => format!("{DEFAULT_REPO} :: Mod-Kit.zip → {inner}"),
        AssetSource::Builtin { .. } => "template embutido do PS2 Manager".into(),
        AssetSource::GithubRaw { repo, tag, asset } => match tag {
            Some(t) => format!("{repo}@{t} :: {asset} (cru)") ,
            None => format!("{repo} :: {asset} (cru)"),
        },
    }
}

fn asset_matches(name: &str, pattern: &str) -> bool {
    if let Some((pre, suf)) = pattern.split_once('*') {
        let n = name.to_ascii_lowercase();
        n.starts_with(&pre.to_ascii_lowercase()) && n.ends_with(&suf.to_ascii_lowercase())
    } else {
        name.eq_ignore_ascii_case(pattern)
    }
}

fn find_asset<'a>(rel: &'a GhRelease, pattern: &str) -> Option<&'a GhAsset> {
    rel.assets.iter().find(|a| asset_matches(&a.name, pattern))
}

async fn fetch_release(
    client: &GitHubClient,
    cache: &mut HashMap<String, GhRelease>,
    repo: &str,
    tag: Option<&str>,
) -> Result<GhRelease> {
    let key = match tag {
        Some(t) => format!("{repo}@{t}"),
        None => repo.to_string(),
    };
    if let Some(r) = cache.get(&key) {
        return Ok(r.clone());
    }
    let rel = match tag {
        Some(t) => client.tag_release(repo, t).await?,
        None => client.latest_release(repo).await?,
    };
    cache.insert(key, rel.clone());
    Ok(rel)
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect()
}

async fn download_to(
    client: &GitHubClient,
    url: &str,
    tmp: &Path,
    name: &str,
    progress: &Arc<AtomicU64>,
) -> Result<PathBuf> {
    let p = tmp.join(sanitize(name));
    client.download(url, &p, Some(progress.clone())).await?;
    Ok(p)
}

fn zip_inner(zip_path: &Path, inner: &str) -> Result<Vec<u8>> {
    let f = std::fs::File::open(zip_path).map_err(Ps2Error::Io)?;
    let mut ar = zip::ZipArchive::new(f)
        .map_err(|e| Ps2Error::Other(format!("ZIP inválido: {e}")))?;
    let want = inner.to_ascii_lowercase();
    for i in 0..ar.len() {
        let mut zf = ar
            .by_index(i)
            .map_err(|e| Ps2Error::Other(format!("ZIP: {e}")))?;
        if zf.is_dir() {
            continue;
        }
        if zf.name().to_ascii_lowercase().ends_with(&want) {
            let mut buf = Vec::with_capacity(zf.size() as usize);
            use std::io::Read;
            zf.read_to_end(&mut buf).map_err(Ps2Error::Io)?;
            return Ok(buf);
        }
    }
    Err(Ps2Error::Other(format!(
        "ZIP: entrada '{inner}' não encontrada"
    )))
}

fn find_by_suffix(dir: &Path, want: &str) -> Result<PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).map_err(Ps2Error::Io)? {
            let e = e.map_err(Ps2Error::Io)?;
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.to_string_lossy().to_ascii_lowercase().ends_with(want) {
                return Ok(p);
            }
        }
    }
    Err(Ps2Error::Other(format!(
        "7z: entrada '{want}' não encontrada"
    )))
}

fn sevenz_inner(src: &Path, out_dir: &Path, inner: &str) -> Result<Vec<u8>> {
    std::fs::create_dir_all(out_dir).map_err(Ps2Error::Io)?;
    sevenz_rust::decompress_file(src, out_dir)
        .map_err(|e| Ps2Error::Other(format!("7z: {e}")))?;
    let found = find_by_suffix(out_dir, &inner.to_ascii_lowercase())?;
    std::fs::read(&found).map_err(Ps2Error::Io)
}

fn sha256_hex(data: &[u8]) -> String {
    let d = Sha256::digest(data);
    let mut out = String::with_capacity(64);
    for b in d.iter() {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

async fn resolve_asset(
    client: &GitHubClient,
    cache: &mut HashMap<String, GhRelease>,
    tmp: &Path,
    asset: &KitAsset,
    progress: &Arc<AtomicU64>,
) -> Result<(Vec<u8>, String)> {
    match &asset.source {
        AssetSource::GithubRelease { repo, tag, asset: pattern } => {
            let rel = fetch_release(client, cache, repo, tag.as_deref()).await?;
            let a = find_asset(&rel, pattern).ok_or_else(|| {
                Ps2Error::Other(format!("{repo}@{}: asset '{pattern}' não encontrado", rel.tag_name))
            })?;
            let path = download_to(client, &a.browser_download_url, tmp, &a.name, progress).await?;
            let bytes = std::fs::read(&path).map_err(Ps2Error::Io)?;
            Ok((bytes, format!("{repo}@{} :: {}", rel.tag_name, a.name)))
        }
        AssetSource::GithubZip { repo, tag, asset: pattern, inner } => {
            let rel = fetch_release(client, cache, repo, tag.as_deref()).await?;
            let a = find_asset(&rel, pattern).ok_or_else(|| {
                Ps2Error::Other(format!("{repo}@{}: asset '{pattern}' não encontrado", rel.tag_name))
            })?;
            let path = download_to(client, &a.browser_download_url, tmp, &a.name, progress).await?;
            let inner_c = inner.clone();
            let bytes = tokio::task::spawn_blocking(move || zip_inner(&path, &inner_c))
                .await
                .map_err(|e| Ps2Error::Other(format!("task: {e}")))??;
            Ok((bytes, format!("{repo}@{} :: {} → {inner}", rel.tag_name, a.name)))
        }
        AssetSource::Github7z { repo, tag, asset: pattern, inner } => {
            let rel = fetch_release(client, cache, repo, tag.as_deref()).await?;
            let a = find_asset(&rel, pattern).ok_or_else(|| {
                Ps2Error::Other(format!("{repo}@{}: asset '{pattern}' não encontrado", rel.tag_name))
            })?;
            let path = download_to(client, &a.browser_download_url, tmp, &a.name, progress).await?;
            let inner_c = inner.clone();
            let out_dir = tmp.join(format!("{}-7z", asset.id));
            let bytes = tokio::task::spawn_blocking(move || sevenz_inner(&path, &out_dir, &inner_c))
                .await
                .map_err(|e| Ps2Error::Other(format!("task: {e}")))??;
            Ok((bytes, format!("{repo}@{} :: {} → {inner}", rel.tag_name, a.name)))
        }
        AssetSource::OplPtbrKit { inner } => {
            let rel = fetch_release(client, cache, DEFAULT_REPO, None).await?;
            let a = find_asset(&rel, "OPL-PTBR-Mod-Kit-*.zip").ok_or_else(|| {
                Ps2Error::Other(format!(
                    "{DEFAULT_REPO}@{}: Mod-Kit zip não encontrado",
                    rel.tag_name
                ))
            })?;
            let path = download_to(client, &a.browser_download_url, tmp, &a.name, progress).await?;
            let inner_c = inner.clone();
            let bytes = tokio::task::spawn_blocking(move || zip_inner(&path, &inner_c))
                .await
                .map_err(|e| Ps2Error::Other(format!("task: {e}")))??;
            Ok((bytes, format!("{DEFAULT_REPO}@{} :: {} → {inner}", rel.tag_name, a.name)))
        }
        AssetSource::Builtin { template } => Ok((
            template.clone().into_bytes(),
            "template embutido do PS2 Manager".into(),
        )),
        AssetSource::GithubRaw { repo, tag, asset: pattern } => {
            let rel = fetch_release(client, cache, repo, tag.as_deref()).await?;
            let a = find_asset(&rel, pattern).ok_or_else(|| {
                Ps2Error::Other(format!("{repo}@{}: asset '{pattern}' não encontrado", rel.tag_name))
            })?;
            let path = download_to(client, &a.browser_download_url, tmp, &a.name, progress).await?;
            let bytes = std::fs::read(&path).map_err(Ps2Error::Io)?;
            Ok((bytes, format!("{repo}@{} :: {} (cru)", rel.tag_name, a.name)))
        }
    }
}

fn write_kit_info(
    dest_root: &Path,
    kind: KitKind,
    manifest: &KitManifest,
    assets: &[AssetReport],
    total: u64,
) -> std::io::Result<()> {
    let dir = dest_root.join(kind.subdir());
    std::fs::create_dir_all(&dir)?;
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let mut s = String::new();
    if kind == KitKind::Library {
        s.push_str(
            "BIBLIOTECA DA CENA PS2 — como usar\n\
             ===================================\n\
             Este acervo OPCIONAL fica na mídia (não vai para o MC).\n\
             O memory card tem poucos MB: copie para mc0:/APPS/ apenas o\n\
             que você usar de verdade (wLaunchELF: mass:/BIBLIOTECA/…).\n\n\
             Pastas:\n\
             • OPL-UPSTREAM/  OPL oficial (inglês) — testar recursos novos.\n\
             • WLAUNCHELF-CARTOES-2EM1/  variante p/ SD2PSX e MemCard PRO2.\n\
             • OPENTUNA/      boot p/ Slims finais (BIOS 2.30, via Fortuna).\n\n\
             Picks manuais (fonte sem release automatizável):\n\
             • FreeMcTuna    github.com/israpps/FreeMcTuna\n\
             • FreeDVDBoot   github.com/cturt/FreeDVDBoot (PREBUILT_ISO_FILES)\n\
             • GSM/cheats/widescreen: já integrados ao OPL (CFG per-game).\n\n\n",
        );
    }
    s.push_str(&format!(
        "{} — gerado pelo PS2 Manager (epoch {secs})\n",
        manifest.meta.kit_name
    ));
    s.push_str(&format!(
        "Spec: {}\nTotal gravado: {total} bytes\n\n",
        manifest.meta.spec
    ));
    for a in assets {
        match &a.status {
            AssetStatus::Ok { bytes, sha256, source } => s.push_str(&format!(
                "{:<30} {:>10}  sha256:{sha256}  {source}\n",
                a.target, bytes
            )),
            AssetStatus::Skipped(w) => {
                s.push_str(&format!("{:<30} SKIP ({w})\n", a.target))
            }
            AssetStatus::Failed(m) => {
                s.push_str(&format!("{:<30} FAIL ({m})\n", a.target))
            }
        }
    }
    std::fs::write(dir.join(kind.info_file()), s)
}

/// Monta o Boot Kit Essencial em `dest_root/BOOT_MC/` baixando os ELFs
/// mais recentes de cada fonte, validando SHA256 e gravando em mídia.
pub async fn assemble(
    dest_root: &Path,
    manifest: &KitManifest,
    client: &GitHubClient,
    progress: Arc<AtomicU64>,
    overwrite: bool,
) -> Result<KitReport> {
    assemble_kind(dest_root, KitKind::Boot, manifest, client, progress, overwrite).await
}

/// Monta qualquer camada de kit (`KitKind`) em `dest_root/<subdir>/`.
pub async fn assemble_kind(
    dest_root: &Path,
    kind: KitKind,
    manifest: &KitManifest,
    client: &GitHubClient,
    progress: Arc<AtomicU64>,
    overwrite: bool,
) -> Result<KitReport> {
    let tmp = std::env::temp_dir().join(format!(
        "ps2mgr-{}-{}",
        kind.subdir().to_ascii_lowercase(),
        std::process::id()
    ));
    tokio::fs::create_dir_all(&tmp).await.map_err(Ps2Error::Io)?;
    let mut cache: HashMap<String, GhRelease> = HashMap::new();
    let mut out: Vec<AssetReport> = Vec::new();

    for a in &manifest.assets {
        if !a.enabled {
            out.push(AssetReport {
                target: a.target.clone(),
                status: AssetStatus::Skipped("desabilitado".into()),
            });
            continue;
        }
        let target_path = dest_root.join(kind.subdir()).join(&a.target);
        if target_path.exists() && !overwrite {
            out.push(AssetReport {
                target: a.target.clone(),
                status: AssetStatus::Skipped("já existe (sem sobrescrever)".into()),
            });
            continue;
        }
        match resolve_asset(client, &mut cache, &tmp, a, &progress).await {
            Ok((bytes, prov)) => {
                let hash = sha256_hex(&bytes);
                if let Some(parent) = target_path.parent() {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .map_err(Ps2Error::Io)?;
                }
                match tokio::fs::write(&target_path, &bytes).await {
                    Ok(()) => out.push(AssetReport {
                        target: a.target.clone(),
                        status: AssetStatus::Ok {
                            bytes: bytes.len() as u64,
                            sha256: hash,
                            source: prov,
                        },
                    }),
                    Err(e) => out.push(AssetReport {
                        target: a.target.clone(),
                        status: AssetStatus::Failed(format!("gravar: {e}")),
                    }),
                }
            }
            Err(e) => out.push(AssetReport {
                target: a.target.clone(),
                status: AssetStatus::Failed(e.to_string()),
            }),
        }
    }

    let total: u64 = out
        .iter()
        .filter_map(|r| match &r.status {
            AssetStatus::Ok { bytes, .. } => Some(*bytes),
            _ => None,
        })
        .sum();

    // Só regrava a auditoria quando houver arquivo novo — preserva a
    // trilha SHA256 da última montagem real.
    let written = out
        .iter()
        .filter(|r| matches!(r.status, AssetStatus::Ok { .. }))
        .count();
    if written > 0 {
        let _ = write_kit_info(dest_root, kind, manifest, &out, total);
    }
    let _ = tokio::fs::remove_dir_all(&tmp).await;
    progress.store(0, Ordering::Relaxed);

    Ok(KitReport {
        kit_name: manifest.meta.kit_name.clone(),
        dest: dest_root.join(kind.subdir()),
        assets: out,
        total_bytes: total,
    })
}
