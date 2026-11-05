//! PS2 Manager — gerenciador nativo Linux para coleções PS2/OPL.
//!
//! Módulos:
//!   iso/     parser ISO9660 mountless + ZSO (LZ4) + Smart Shrink (LBA)
//!   opl/     estrutura de pastas, naming, ul.cfg (USBExtreme), CFG/GSM
//!   vmc/     memória virtual .bin (superblock genvmc) + export PSU
//!   scrape/  capas GameTDB (ART/_COV.png) + PNACH → .cht
//!   deploy/  GitHub API (OPL-Mod-PTBR) + gerador /BOOT_MC/
//!   tui/     dashboard Ratatui

mod cli;
mod deploy;
mod error;
mod iso;
mod opl;
mod scrape;
mod tui;
mod vmc;

use clap::Parser;

fn main() {
    let cli = cli::Cli::parse();
    let token = cli.token.or_else(|| std::env::var("GITHUB_TOKEN").ok());
    let command = cli.command;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let res = rt.block_on(async move { cli::run(command, token).await });
    rt.shutdown_timeout(std::time::Duration::from_millis(600));

    if let Err(e) = res {
        eprintln!("✗ erro: {e}");
        std::process::exit(1);
    }
}
