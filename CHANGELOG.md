# CHANGELOG

## 0.2.0 (2026-11-11)
- feat(ux): **executar sem argumentos abre a interface (TUI) direto** — acabou a
  parede de comandos; subcomandos ficam para uso avançado/scripts
- feat(ux): binário cru sem terminal (2 cliques no gerenciador de arquivos)
  lança um emulador de terminal automaticamente (x-terminal-emulator, GNOME,
  Konsole, Alacritty, Kitty, Foot, Xfce, xterm…) com fallback zenity/kdialog
- feat(diag): **Diagnóstico completo** (tecla `d` na TUI · item 5 na F4 ·
  `ps2-manager diag`) — testa ambiente, mídias, parser ISO9660, instalação
  staging→ZSO, integração OPL (scan/ul.cfg/CFG), capas e GitHub numa sandbox
  temporária; relatório TXT em `~/.local/share/ps2-manager/reports/`
- feat(diag): disco sintético interno (mini-ISO9660 válida com SYSTEM.CNF) —
  o diagnóstico funciona mesmo sem nenhuma ISO à mão; com ISO na pasta do
  AppImage, testa o arquivo real do usuário
- feat(log): **log persistente** em `~/.local/share/ps2-manager/logs/`
  (rotaciona a 2 MB, mantém 5) — todo evento da TUI/CLI/erros/panics é
  registrado para investigar problemas depois de usar
- feat(tui): painel de Ajuda reescrito (início rápido, diagnóstico, logs);
  hook de panic restaura o terminal e registra no log
- test: gerador de ISO sintética virou teste de regressão do parser (4 testes)

## 0.1.1 (2026-11-10)
- fix(gov): release commit sem `[skip ci]` — skip no head commit suprime
  também o push da tag (job release nunca dispararia)
- fix(gov): weekly-release só adiciona Cargo.lock se ele for rastreado
- feat(gov): weekly-release.sh — ciclo semanal com dry-run e abort-limpo
- feat(gov): bootstrap.sh + daily-audit.sh — governança no caminho persistente
- feat: Biblioteca da Cena (MP5) — acervo opcional em /BIBLIOTECA/

## 0.1.0 (2026-11-10)
- Primeira release: ISO Parser mountless, ZSO LZ4 multithread, Smart Shrink,
  pipeline staged anti-desastre, seletor de mídias seguro, ul.cfg USBExtreme,
  CFG/GSM, capas GameTDB, cheats PNACH→CHT, VMC, deploy OPL-Mod-PTBR,
  Boot Kit Essencial e TUI Ratatui com mouse.
