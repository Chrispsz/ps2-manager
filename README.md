# PS2 Manager 🎮

**Gerenciador nativo Linux para coleções de PlayStation 2 / Open PS2 Loader (OPL)** — um único binário em Rust, sem Wine, sem Mono, sem dependências legadas do Windows.

> **Uso em 1 clique:** dois cliques no AppImage (ou `./ps2-manager`) abrem a interface direto — sem terminal aberto, o app lança um automaticamente. Sem argumentos = TUI; subcomandos são só para scripts/usos avançados.

Substitui USBUtil, HDL Dump e OPL Manager (via Wine) com uma suíte 100% Rust — crates sem bibliotecas C (LZ4 puro via `lz4_flex`, TLS via `rustls`).

> Integração direta com o [**OPL-Mod-PTBR**](https://github.com/Chrispsz/OPL-Mod-PTBR): o comando `deploy` baixa o release kit mais recente e monta a pasta `BOOT_MC/` pronta para copiar ao memory card via uLaunchELF.

## Recursos

| Módulo | Comando | O que faz |
|---|---|---|
| **ISO Parser** | `info` | Lê o PVD (setor 16) e extrai o **Game ID do SYSTEM.CNF sem montar a imagem** — funciona também em `.zso` (decompressão LZ4 on-demand) |
| **ZSO/ZISO** | `zso` / `unzso` | Conversor LZ4 **multithread (rayon)**, binário-compatível com o driver ZSO do OPL (`modules/isofs/zso.c`): blocos de 2048 B, índice em unidades de `2^align`, bit 31 = bloco plain |
| **Smart Shrink** | `shrink` | Remove setores fantasma (0x00/0xFF) do fim dos arquivos e **realoca os extents reescrevendo PVD, path tables e directory records (LE+BE)** — conteúdo do jogo permanece byte-a-byte idêntico |
| **Biblioteca OPL** | `install` / `scan` | Estrutura oficial `CD DVD ART CFG VMC CHT APPS THM LNG` (idêntica a `sbCreateFolders`), naming `GAMEID.Título.iso`, relatório de fragmentação |
| **Pipeline Staged (MP7)** | `install` | **Anti-desastre + staging em SSD**: conversão ZSO no disco do PC (`~/.cache/ps2-manager`) → validação de integridade (header + Game ID) → `fallocate` + streaming I/O na mídia → rollback automático em `.part` (cancelamento nunca deixa arquivo parcial) |
| **Seletor de Mídias (MP7)** | `devices` / TUI `[F6]` | Lista **apenas** removíveis/USB/SD com montagem (sysfs + `/proc/self/mounts`, **zero libudev**). Discos internos (`/`, `/boot`, `/home`, NVMe, virtio, md/dm/zram) **nunca** aparecem — override de dev: `PS2_MANAGER_ALLOW_INTERNAL=1` |
| **ul.cfg** | `ulcfg` | Formato USBExtreme de 64 bytes exato (`include/supportbase.h`), com o **CRC32 USBAdvance réplica de `USBA_crc32`** (tabela invertida, XOR invertido, NUL incluso) — validado bit a bit contra o C original; partes `ul.<CRC>.<ID>.<NN>` |
| **CFG + GSM** | `cfg` | Gera `CFG/<ID>.cfg` com chaves oficiais (`#Name`, `#Media`, `$GSMVMode`…) + **`$EnableCheat=1` automático** quando `CHT/<ID>.cht` existe (`cheatman.c` lê do CFG do jogo e liga o motor PS2RD sem intervenção) |
| **Capas** | `covers` | Baixa da base [xlenore/ps2-covers](https://github.com/xlenore/ps2-covers) (+ GameTDB fallback) e salva `ART/<ID>_COV.png` **140×200** |
| **Ícones _ICO** | `icons` | Gera `ART/<ID>_ICO.png` **128×128** (sufixo `ICO` do ItemIcon — `themes.c:1064` + `bdmGetImage`): recorte quadrado da capa; sem capa, baixa e grava ambas |
| **Cheats** | `cheats` | Converte PNACH (PCSX2) → `CHT/<ID>.cht` nativo (formato PS2RD do `cheatman.c`: 16 hex digits, tipos byte/word/extended mapeados para raw CB) |
| **VMC** | `vmc list` / `vmc export` / `vmc import` | Leitura **e escrita** com layout `McFsEntry` (ps2sdk) + FAT via `ifc_list` (genvmc): restaura `.psu` do uLE (`import`), faz backup (`export`) e lista saves — import all-or-nothing com rollback de FAT |
| **Deploy PT-BR** | `deploy` | Baixa o `OPL-PTBR-Mod-Kit-*.zip` da release mais recente (incl. prereleases) e gera `BOOT_MC/{OPL,BOOT,APPS,SYS-CONF}` com instruções |
| **Boot Kit Essencial** | `bootkit` | Monta `/BOOT_MC/` com os ELFs mais atuais da cena: OPL-Mod-PTBR + **wLaunchELF_ISR** (exFAT/MX4SIO) + **PS2BBL** + **Memory Card Annihilator**, com SHA256 e fonte de cada asset ([detalhes](docs/BOOT_KIT.md)) |
| **Biblioteca da Cena** | `biblioteca` | Acervo **opcional** em `/BIBLIOTECA/` na mídia: OPL upstream (testes), wLaunchELF p/ cartões 2-em-1 (SD2PSX/MemCard PRO2), OpenTuna (Slims finais/BIOS 2.30) — o MC fica enxuto: copie só o que usar (guia em `LEIA-ME.txt`) |
| **TUI** | `tui` / *sem argumentos* | Dashboard Ratatui com **mouse** (clique em abas/toggles/botões, scroll nas listas): `[F1] Biblioteca · [F2] Instalar (navegador de ISOs + staging) · [F3] Boot Kit · [F4] Capas/Cheats · [F5] Ajuda`, seletor de mídia `[F6]`, wizard de primeiro uso, progresso por fases com ETA/MB/s. **É o padrão ao executar o app** |
| **Diagnóstico** | tecla `d` na TUI / `diag` | **Botão de teste**: exercita TODOS os subsistemas numa sandbox temporária (parser, instalação staging→ZSO, ul.cfg/CFG, capas, GitHub) e grava relatório TXT em `~/.local/share/ps2-manager/reports/`. Com ISO na pasta do AppImage testa a real; sem ISO usa um disco sintético interno |
| **Log persistente** | `~/.local/share/ps2-manager/logs/` | Toda operação, erro e panic é registrado (rotaciona a 2 MB) — a fonte para detectar e reportar problemas |
| **AppImage 1-clique (MP6)** | `dist/` | `AppRun` detecta o emulador de terminal do desktop (Alacritty/Kitty/Foot/Konsole/GNOME…) e abre a TUI automaticamente com 2 cliques — zero comandos, zero flags. CI gera `ps2-manager-x86_64.AppImage` |

## Build

```bash
cargo build --release
# binário: target/release/ps2-manager
```

Requisitos: Rust 1.75+ (testado com 1.99). Zero dependências de sistema além da libc.

## Exemplos rápidos

```bash
# Interface (padrão) — sem argumentos abre a TUI
./ps2-manager

# Diagnóstico completo (teste tudo: parser, install, ZSO, capas, GitHub)
./ps2-manager diag

# Analisar uma ISO (sem mount)
ps2-manager info GranTurismo4.iso

# Instalar convertendo para ZSO — pipeline seguro (staging SSD → fallocate)
ps2-manager install GranTurismo4.iso /media/$USER/PS2_SD --zso

# Listar mídias externas seguras (discos do PC nunca aparecem)
ps2-manager devices

# Smart shrink preservando LBA
ps2-manager shrink sujo.iso limpo.iso

# Converter biblioteca para formato USBExtreme (FAT32 + ul.cfg)
ps2-manager ulcfg /media/$USER/PS2_SD --part-size-mb 1024

# Capas 140x200 + cheats + cfg de uma vez
ps2-manager covers /media/$USER/PS2_SD
ps2-manager cheats /media/$USER/PS2_SD --pnach-dir ~/pnach
ps2-manager cfg /media/$USER/PS2_SD --gsm-vmode 2

# Deploy do OPL PT-BR para o memory card (via uLaunchELF)
GITHUB_TOKEN=ghp_xxx ps2-manager deploy /media/$USER/PS2_SD

# Boot Kit Essencial: OPL + wLaunchELF_ISR + PS2BBL + MCA em /BOOT_MC/
ps2-manager bootkit /media/$USER/PS2_SD
ps2-manager bootkit --list   # fontes/variantes do manifest embutido
```

### Token GitHub

O `deploy` funciona **sem token** para repositórios públicos; com `GITHUB_TOKEN` (env ou `--token`) você ganha rate limit maior. O token **nunca é gravado em disco** — vive só na memória do processo.

## Compatibilidade binária (fonte da verdade)

Toda a camada binária foi implementada contra o código-fonte real do [Open-PS2-Loader](https://github.com/ps2homebrew/Open-PS2-Loader):

| Formato | Referência upstream | Status |
|---|---|---|
| ul.cfg (64 B) | `include/supportbase.h:28-38`, `src/supportbase.c` | ✅ validado |
| CRC32 USBAdvance | `src/system.c:299-325` (`USBA_crc32`) | ✅ bit-exact vs gcc |
| ZSO header + índice | `modules/isofs/zso.h/c`, `pc/ziso.py` | ✅ roundtrip MD5 |
| SYSTEM.CNF / BOOT2 | `src/ps2cnf.c` | ✅ |
| .cht (PS2RD) | `src/cheatman.c`, `include/cheatman.h` | ✅ MAX_CODES 250 · comentários `//`/`#` · código com comentário inline truncado (`term_str`/`is_cmt_str`) |
| Arte `_COV.png` | `src/themes.c`, `src/textures.c` | ✅ |
| Pastas do dispositivo | `sbCreateFolders` (`supportbase.c:842-851`) | ✅ |
| Ícones `_ICO.png` (ItemIcon) | `src/themes.c:1064`, `src/bdmsupport.c bdmGetImage` | ✅ |
| Entradas de diretório VMC (`McFsEntry`) | ps2sdk `libmc-common.h` + genvmc + uLE `vmc_fs/vmc.h` | ✅ list · import · export |
| Formato PSU (`psu_header`/`mcTable`) | uLE `filer.c` (backup/restore) | ✅ roundtrip testado |

## AppImage (execução em 1 clique)

Baixe o artifact `ps2-manager-AppImage_x86_64` do [último build](https://github.com/Chrispsz/ps2-manager/actions), dê permissão e **dois cliques**:

```bash
chmod +x ps2-manager-x86_64.AppImage
./ps2-manager-x86_64.AppImage      # do terminal: abre direto
# ou: 2 cliques no Dolphin/Nautilus/Thunar → abre em uma janela de terminal
```

O `AppRun` interno detecta o emulador de terminal do seu desktop (Alacritty, Kitty, Foot, Konsole, GNOME Terminal…) e abre a TUI interativa automaticamente — sem decorar comandos. Para empacotar manualmente: `./dist/build-appimage.sh`.

## Arquitetura

```
src/
├── core/     storage.rs (anti-desastre: sysfs + mounts, statvfs, fallocate)
│             staging.rs (StagingBuffer RAII no SSD local)  pipeline.rs (3 fases)
├── iso/       parser.rs (ISO9660 mountless, trait SectorSource p/ ISO ou ZSO)
│              zso.rs (codec LZ4 paralelo)  shrink.rs (relocação de extents)
├── opl/       library.rs (pastas/naming/scan)  ulcfg.rs (USBExtreme+CRC32)
│              cfg.rs (CFG/GSM)
├── vmc/       mod.rs (McFsEntry+superblock genvmc, FAT rw via ifc_list,
│              saves, import/export PSU réplica uLE filer.c)
├── scrape/    artwork.rs (ps2-covers/GameTDB)  cheats.rs (PNACH→.cht)
├── deploy/    github.rs (API releases)  fmcb.rs (BOOT_MC + templates)
│              bootkit.rs (Boot Kit Essencial + manifest embutido)
├── tui/       mod.rs (app + abas + mouse)  modals.rs (picker/browser/progresso)
├── dist/      AppRun (launcher 1-clique)  ps2-manager.desktop  build-appimage.sh
└── cli.rs     subcomandos clap
```

## Roadmap

- [x] v0.1 — parser ISO9660, ZSO, shrink, ul.cfg, deploy PT-BR, capas, cheats, TUI
- [x] MP5 — Boot Kit Essencial (`bootkit`, manifest embutido, CI semanal)
- [x] MP5 — Biblioteca da Cena (`biblioteca`, acervo opcional em `/BIBLIOTECA/`)
- [x] MP6/7 — TUI com mouse/abas/modais/wizard, AppImage 1-clique, seletor
      anti-desastre de mídias, pipeline de staging em SSD + fallocate + rollback
- [ ] v0.2 — import PSU, validação de fragmentação real (FIEMAP), scrapping de ícones `_ICO`
- [ ] v0.3 — sincronização incremental com o cartão, cache de metadados SQLite
- [ ] v0.4 — UDPBD server integration (rede para o PS2)

## Licença

MIT. OPL é © ps2homebrew (AFL-3.0) — este projeto não redistribui código do OPL, apenas implementa formatos documentados no seu código-fonte.
