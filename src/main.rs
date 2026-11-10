//! PS2 Manager — gerenciador nativo Linux para coleções PS2/OPL.
//!
//! Módulos:
//!   logging/  log persistente (~/.local/share/ps2-manager/logs/)
//!   diag/     diagnóstico completo (botão de teste)
//!   core/     seletor seguro de mídias (anti-desastre) + staging SSD + pipeline
//!   iso/      parser ISO9660 mountless + ZSO (LZ4) + Smart Shrink (LBA)
//!   opl/      estrutura de pastas, naming, ul.cfg (USBExtreme), CFG/GSM
//!   vmc/      memória virtual .bin (superblock genvmc) + export PSU
//!   scrape/   capas GameTDB (ART/_COV.png) + PNACH → .cht
//!   deploy/   GitHub API (OPL-Mod-PTBR) + boot kit /BOOT_MC/
//!   tui/      dashboard Ratatui (mouse + abas + modais)
//!
//! Executar sem argumentos = abrir a interface (TUI) direto.

mod cli;
mod core;
mod deploy;
mod diag;
mod error;
mod iso;
mod logging;
mod opl;
mod scrape;
mod tui;
mod vmc;

use clap::Parser;

fn main() {
    logging::init();
    let cli = cli::Cli::parse();
    let token = cli.token.or_else(|| std::env::var("GITHUB_TOKEN").ok());
    let command = cli.command;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let res = rt.block_on(async move {
        match command {
            // Padrão: abrir a interface. Nada de parede de comandos.
            None => tui::run_tui(None).await,
            Some(cmd) => cli::run(cmd, token)
                .await
                .map_err(|e| crate::error::Ps2Error::Other(e.to_string())),
        }
    });
    rt.shutdown_timeout(std::time::Duration::from_millis(600));

    if let Err(e) = res {
        logging::error(&format!("saída com erro: {e}"));
        eprintln!("✗ erro: {e}");
        std::process::exit(1);
    }
    logging::info("encerrado normalmente");
}
