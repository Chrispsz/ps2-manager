//! Deploy FMCB/uLaunchELF — baixa o release kit do `Chrispsz/OPL-Mod-PTBR`
//! e monta a pasta `/BOOT_MC/` pronta para cópia via uLaunchELF no PS2.
//!
//! Mapeamento segue o próprio release.yml do repositório:
//!   OPNPS2LD.ELF         → BOOT_MC/OPL/OPNPS2LD.ELF   (mc0:/OPL/)
//!   BOOT.ELF (uLaunchELF)→ BOOT_MC/BOOT/BOOT.ELF      (mc0:/BOOT/)
//!   MCA.ELF / HDLGameInstaller.ELF / MFU.ELF → BOOT_MC/APPS/ (mc0:/APPS/)
//!   INFO.txt             → BOOT_MC/INFO.txt

use crate::deploy::github::{GitHubClient, DEFAULT_REPO};
use crate::error::{Ps2Error, Result};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub struct DeployReport {
    pub tag: String,
    pub files: Vec<(String, u64)>,
}

const MAPPING: [(&str, &str); 5] = [
    ("OPNPS2LD.ELF", "BOOT_MC/OPL/OPNPS2LD.ELF"),
    ("BOOT.ELF", "BOOT_MC/BOOT/BOOT.ELF"),
    ("MCA.ELF", "BOOT_MC/APPS/MCA.ELF"),
    ("HDLGAMEINSTALLER.ELF", "BOOT_MC/APPS/HDLGameInstaller.ELF"),
    ("MFU.ELF", "BOOT_MC/APPS/MFU.ELF"),
];

fn extract_and_map(zip_path: &Path, device_root: &Path) -> Result<Vec<(String, u64)>> {
    let file = std::fs::File::open(zip_path).map_err(Ps2Error::Io)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| Ps2Error::Other(format!("ZIP inválido: {e}")))?;
    let mut out = Vec::new();
    for i in 0..archive.len() {
        let mut zf = archive
            .by_index(i)
            .map_err(|e| Ps2Error::Other(format!("ZIP: {e}")))?;
        if zf.is_dir() {
            continue;
        }
        let base = std::path::Path::new(zf.name())
            .file_name()
            .map(|s| s.to_string_lossy().to_ascii_uppercase())
            .unwrap_or_default();
        if base == "INFO.TXT" {
            let dest = device_root.join("BOOT_MC/INFO.txt");
            let mut w = std::fs::File::create(&dest).map_err(Ps2Error::Io)?;
            std::io::copy(&mut zf, &mut w).map_err(Ps2Error::Io)?;
            out.push(("BOOT_MC/INFO.txt".into(), zf.size()));
            continue;
        }
        if let Some((_, rel)) = MAPPING.iter().find(|(src, _)| *src == base) {
            let dest = device_root.join(rel);
            let mut w = std::fs::File::create(&dest).map_err(Ps2Error::Io)?;
            std::io::copy(&mut zf, &mut w).map_err(Ps2Error::Io)?;
            out.push((rel.to_string(), zf.size()));
        }
    }
    Ok(out)
}

fn write_templates(device_root: &Path, tag: &str) -> Result<()> {
    let dir = device_root.join("BOOT_MC/SYS-CONF");
    std::fs::create_dir_all(&dir).map_err(Ps2Error::Io)?;

    let cnf = format!(
        "# FREMCB.CNF — template gerado pelo PS2 Manager (release {tag})\n\
         # Ajuste os nomes de chave conforme a versão do seu FreeMcBoot.\n\
         # Objetivo: R1 abre o OPL PT-BR, R2 abre o uLaunchELF.\n\
         name_pad1_R1=OPL PT-BR\n\
         path_pad1_R1=mc0:/OPL/OPNPS2LD.ELF\n\
         name_pad1_R2=uLaunchELF\n\
         path_pad1_R2=mc0:/BOOT/BOOT.ELF\n"
    );
    std::fs::write(dir.join("FREMCB.CNF"), cnf).map_err(Ps2Error::Io)?;

    let readme = format!(
        "BOOT_MC gerado pelo PS2 Manager — release {tag} (OPL-Mod-PTBR)\n\
         ================================================================\n\n\
         Copie para o PS2 via uLaunchELF:\n\
         1. OPL/OPNPS2LD.ELF        → mc0:/OPL/OPNPS2LD.ELF\n\
         2. BOOT/BOOT.ELF           → mc0:/BOOT/BOOT.ELF\n\
         3. APPS/*                  → mc0:/APPS/\n\
         4. SYS-CONF/FREMCB.CNF     → mc0:/SYS-CONF/ (ajuste as teclas)\n\n\
         Dica: no uLaunchELF use FileBrowser → mass:/BOOT_MC → copie para mc0:.\n\
         Fonte: https://github.com/{DEFAULT_REPO}\n"
    );
    std::fs::write(
        device_root.join("BOOT_MC/README-INSTALACAO.txt"),
        readme,
    )
    .map_err(Ps2Error::Io)?;
    Ok(())
}

/// Executa o deploy completo: download do Mod-Kit + extração + templates.
pub async fn deploy_boot_mc(
    device_root: &Path,
    repo: &str,
    client: &GitHubClient,
    progress: Arc<AtomicU64>,
) -> Result<DeployReport> {
    let rel = client.latest_release(repo).await?;
    let kit = rel
        .assets
        .iter()
        .find(|a| a.name.starts_with("OPL-PTBR-Mod-Kit-") && a.name.ends_with(".zip"))
        .ok_or_else(|| {
            Ps2Error::Other(format!(
                "release {} não contém OPL-PTBR-Mod-Kit-*.zip",
                rel.tag_name
            ))
        })?
        .clone();

    for d in ["BOOT_MC/OPL", "BOOT_MC/BOOT", "BOOT_MC/APPS", "BOOT_MC/SYS-CONF"] {
        tokio::fs::create_dir_all(device_root.join(d))
            .await
            .map_err(Ps2Error::Io)?;
    }

    let tmp = std::env::temp_dir().join(format!("ps2mgr-kit-{}", std::process::id()));
    tokio::fs::create_dir_all(&tmp).await.map_err(Ps2Error::Io)?;
    let zip_path = tmp.join(&kit.name);
    client
        .download(&kit.browser_download_url, &zip_path, Some(progress.clone()))
        .await?;

    let dr = device_root.to_path_buf();
    let files = tokio::task::spawn_blocking(move || extract_and_map(&zip_path, &dr))
        .await
        .map_err(|e| Ps2Error::Other(format!("task: {e}")))??;
    let _ = std::fs::remove_dir_all(&tmp);
    write_templates(device_root, &rel.tag_name)?;
    let _ = progress.load(Ordering::Relaxed);
    Ok(DeployReport { tag: rel.tag_name, files })
}
