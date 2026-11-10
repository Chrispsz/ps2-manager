//! Download automático de capas → `ART/<GAMEID>_COV.png` (140×200 PNG).
//! Ícones por jogo → `ART/<GAMEID>_ICO.png` (128×128, v0.3).
//!
//! Fontes (em ordem):
//!   1. `xlenore/ps2-covers` (base da comunidade, ~16k capas):
//!      `covers/default/{ID}.jpg` e `covers/3d/{ID}.png`
//!      — ID convertido `SLUS_214.44` → `SLUS-21444`
//!   2. GameTDB por região (fallback)
//!      O OPL só faz probe de `.png` para arte descoberta (`textures.c:552-571`),
//!      e valida no máximo 1024×1024 — 140×200 (convenção OPL) cabe de sobra.
//!      Ícones: o elemento ItemIcon do tema usa o sufixo "ICO" com o MESMO
//!      mecanismo (`themes.c:1064` → `initGameImage(..., "ICO", 20, ...)`;
//!      `bdmsupport.c bdmGetImage` monta `ART/<ID>_ICO` + probe `.png`).

use crate::error::Result;
use crate::opl::library::GameEntry;
use image::imageops::FilterType;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;

/// `SLUS_214.44` → `SLUS-21444` (formato usado pelo ps2-covers)
fn dashes_id(game_id: &str) -> String {
    game_id.replace('_', "-").replace('.', "")
}

fn cover_urls(game_id: &str, extra_regions: &[String]) -> Vec<String> {
    let d = dashes_id(game_id);
    let mut urls = vec![
        format!("https://raw.githubusercontent.com/xlenore/ps2-covers/main/covers/default/{d}.jpg"),
        format!("https://raw.githubusercontent.com/xlenore/ps2-covers/main/covers/3d/{d}.png"),
    ];
    let regions = if extra_regions.is_empty() {
        vec!["US".to_string(), "EU".to_string(), "JP".to_string()]
    } else {
        extra_regions.to_vec()
    };
    for reg in regions {
        urls.push(format!("https://art.gametdb.com/ps2/coverS/{reg}/{game_id}.png"));
    }
    urls
}

/// Baixa capas para todos os jogos da biblioteca que ainda não as possuem.
/// Retorna (baixadas, falhas).
pub async fn fetch_covers(
    device_root: &Path,
    entries: &[GameEntry],
    concurrency: usize,
    extra_regions: &[String],
    token: Option<String>,
) -> Result<(usize, usize)> {
    let art_dir = device_root.join("ART");
    tokio::fs::create_dir_all(&art_dir).await.map_err(crate::error::Ps2Error::Io)?;

    let mut http = reqwest::Client::builder().user_agent("ps2-manager/0.1");
    if let Some(t) = &token {
        use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
        let mut headers = HeaderMap::new();
        if let Ok(v) = HeaderValue::from_str(&format!("Bearer {t}")) {
            headers.insert(AUTHORIZATION, v);
        }
        http = http.default_headers(headers);
    }
    let client = Arc::new(http.build().map_err(crate::error::Ps2Error::Http)?);
    let sem = Arc::new(Semaphore::new(concurrency.max(1)));

    let mut handles = Vec::new();
    for e in entries {
        let dest: PathBuf = art_dir.join(format!("{}_COV.png", e.game_id));
        if tokio::fs::try_exists(&dest).await.unwrap_or(false) {
            continue;
        }
        let permit = sem.clone().acquire_owned().await.unwrap();
        let client = client.clone();
        let dest = dest.clone();
        let urls = cover_urls(&e.game_id, extra_regions);
        handles.push(tokio::spawn(async move {
            let _permit = permit;
            for url in urls {
                let Ok(resp) = client.get(&url).send().await else { continue };
                if !resp.status().is_success() {
                    continue;
                }
                let Ok(bytes) = resp.bytes().await else { continue };
                if let Ok(img) = image::load_from_memory(&bytes) {
                    let cov = img.resize_exact(140, 200, FilterType::Lanczos3);
                    if cov.save(&dest).is_ok() {
                        return true;
                    }
                }
            }
            false
        }));
    }

    let (mut ok, mut fail) = (0usize, 0usize);
    for h in handles {
        match h.await {
            Ok(true) => ok += 1,
            _ => fail += 1,
        }
    }
    Ok((ok, fail))
}

/// Recorte quadrado central → 128×128 (potência de 2, seguro para textura
/// GS do OPL).
fn icon_from_cover(img: &image::DynamicImage) -> image::DynamicImage {
    let (w, h) = (img.width(), img.height());
    let side = w.min(h).max(1);
    let x = (w - side) / 2;
    let y = (h - side) / 2;
    let crop = image::imageops::crop_imm(img, x, y, side, side).to_image();
    image::DynamicImage::ImageRgba8(image::imageops::resize(
        &crop,
        128,
        128,
        FilterType::Lanczos3,
    ))
}

/// Gera ícones `ART/<GAMEID>_ICO.png` (128×128) — o ícone por jogo que os
/// temas OPL mostram no elemento ItemIcon (sufixo "ICO" + probe `.png`).
///
/// Estratégia: derivar da capa existente (`<ID>_COV.png`); quando a capa
/// não existe, baixa da mesma base e grava também a capa (uma rede a menos
/// na rotina de capas). Ícones já existentes (ex.: customizados à mão) são
/// preservados. Retorna (ícones gerados, falhas).
pub async fn fetch_icons(
    device_root: &Path,
    entries: &[GameEntry],
    concurrency: usize,
    extra_regions: &[String],
    token: Option<String>,
) -> Result<(usize, usize)> {
    let art_dir = device_root.join("ART");
    tokio::fs::create_dir_all(&art_dir).await.map_err(crate::error::Ps2Error::Io)?;

    let mut http = reqwest::Client::builder().user_agent("ps2-manager/0.1");
    if let Some(t) = &token {
        use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
        let mut headers = HeaderMap::new();
        if let Ok(v) = HeaderValue::from_str(&format!("Bearer {t}")) {
            headers.insert(AUTHORIZATION, v);
        }
        http = http.default_headers(headers);
    }
    let client = Arc::new(http.build().map_err(crate::error::Ps2Error::Http)?);
    let sem = Arc::new(Semaphore::new(concurrency.max(1)));

    let mut handles = Vec::new();
    for e in entries {
        let ico: PathBuf = art_dir.join(format!("{}_ICO.png", e.game_id));
        if tokio::fs::try_exists(&ico).await.unwrap_or(false) {
            continue;
        }
        let cov: PathBuf = art_dir.join(format!("{}_COV.png", e.game_id));
        let has_cov = tokio::fs::try_exists(&cov).await.unwrap_or(false);
        let permit = sem.clone().acquire_owned().await.unwrap();
        let client = client.clone();
        let urls = cover_urls(&e.game_id, extra_regions);
        handles.push(tokio::spawn(async move {
            let _permit = permit;
            if has_cov {
                // 1) capa local → ícone
                if let Ok(img) = image::open(&cov) {
                    return icon_from_cover(&img).save(&ico).is_ok();
                }
                return false;
            }
            // 2) sem capa: baixa uma vez e escreve capa + ícone
            for url in urls {
                let Ok(resp) = client.get(&url).send().await else { continue };
                if !resp.status().is_success() {
                    continue;
                }
                let Ok(bytes) = resp.bytes().await else { continue };
                if let Ok(img) = image::load_from_memory(&bytes) {
                    let cov_ok = img
                        .resize_exact(140, 200, FilterType::Lanczos3)
                        .save(&cov)
                        .is_ok();
                    let ico_ok = icon_from_cover(&img).save(&ico).is_ok();
                    return ico_ok && cov_ok;
                }
            }
            false
        }));
    }

    let (mut ok, mut fail) = (0usize, 0usize);
    for h in handles {
        match h.await {
            Ok(true) => ok += 1,
            _ => fail += 1,
        }
    }
    Ok((ok, fail))
}
