# PS2 Manager 🎮

**Gerenciador nativo Linux para coleções de PlayStation 2 / Open PS2 Loader (OPL)** — um único binário em Rust, sem Wine, sem Mono, sem dependências legadas do Windows.

Substitui USBUtil, HDL Dump e OPL Manager (via Wine) com uma suíte 100% Rust — crates sem bibliotecas C (LZ4 puro via `lz4_flex`, TLS via `rustls`).

> Integração direta com o [**OPL-Mod-PTBR**](https://github.com/Chrispsz/OPL-Mod-PTBR): o comando `deploy` baixa o release kit mais recente e monta a pasta `BOOT_MC/` pronta para copiar ao memory card via uLaunchELF.

## Recursos

| Módulo | Comando | O que faz |
|---|---|---|
| **ISO Parser** | `info` | Lê o PVD (setor 16) e extrai o **Game ID do SYSTEM.CNF sem montar a imagem** — funciona também em `.zso` (decompressão LZ4 on-demand) |
| **ZSO/ZISO** | `zso` / `unzso` | Conversor LZ4 **multithread (rayon)**, binário-compatível com o driver ZSO do OPL (`modules/isofs/zso.c`): blocos de 2048 B, índice em unidades de `2^align`, bit 31 = bloco plain |
| **Smart Shrink** | `shrink` | Remove setores fantasma (0x00/0xFF) do fim dos arquivos e **realoca os extents reescrevendo PVD, path tables e directory records (LE+BE)** — conteúdo do jogo permanece byte-a-byte idêntico |
| **Biblioteca OPL** | `install` / `scan` | Estrutura oficial `CD DVD ART CFG VMC CHT APPS THM LNG` (idêntica a `sbCreateFolders`), naming `GAMEID.Título.iso`, relatório de fragmentação |
| **ul.cfg** | `ulcfg` | Formato USBExtreme de 64 bytes exato (`include/supportbase.h`), com o **CRC32 USBAdvance réplica de `USBA_crc32`** (tabela invertida, XOR invertido, NUL incluso) — validado bit a bit contra o C original; partes `ul.<CRC>.<ID>.<NN>` |
| **CFG + GSM** | `cfg` | Gera `CFG/<ID>.cfg` com chaves oficiais (`#Name`, `#Media`, `$GSMVMode`…) |
| **Capas** | `covers` | Baixa da base [xlenore/ps2-covers](https://github.com/xlenore/ps2-covers) (+ GameTDB fallback) e salva `ART/<ID>_COV.png` **140×200** |
| **Cheats** | `cheats` | Converte PNACH (PCSX2) → `CHT/<ID>.cht` nativo (formato PS2RD do `cheatman.c`: 16 hex digits, tipos byte/word/extended mapeados para raw CB) |
| **VMC** | `vmc list` / `vmc export` | Lê superblock genvmc (384 B), caminha FAT e lista saves; exportação `.psu` (experimental) |
| **Deploy PT-BR** | `deploy` | Baixa o `OPL-PTBR-Mod-Kit-*.zip` da release mais recente (incl. prereleases) e gera `BOOT_MC/{OPL,BOOT,APPS,SYS-CONF}` com instruções |
| **Boot Kit Essencial** | `bootkit` | Monta `/BOOT_MC/` com os ELFs mais atuais da cena: OPL-Mod-PTBR + **wLaunchELF_ISR** (exFAT/MX4SIO) + **PS2BBL** + **Memory Card Annihilator**, com SHA256 e fonte de cada asset ([detalhes](docs/BOOT_KIT.md)) |
| **TUI** | `tui` | Dashboard Ratatui: biblioteca, ações rápidas `[C]ZSO [A]capas [D]deploy [F3]boot kit`, barra de progresso e log |

## Build

```bash
cargo build --release
# binário: target/release/ps2-manager
```

Requisitos: Rust 1.75+ (testado com 1.99). Zero dependências de sistema além da libc.

## Exemplos rápidos

```bash
# Analisar uma ISO (sem mount)
ps2-manager info GranTurismo4.iso

# Instalar convertendo para ZSO (comprime ~40-60%)
ps2-manager install GranTurismo4.iso /media/$USER/PS2_SD --zso

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
| .cht (PS2RD) | `src/cheatman.c`, `include/cheatman.h` | ✅ |
| Arte `_COV.png` | `src/themes.c`, `src/textures.c` | ✅ |
| Pastas do dispositivo | `sbCreateFolders` (`supportbase.c:842-851`) | ✅ |
| VMC superblock | `modules/vmc/genvmc/genvmc.c:82-118` | ✅ list · ⚠ PSU experimental |

## Arquitetura

```
src/
├── iso/       parser.rs (ISO9660 mountless, trait SectorSource p/ ISO ou ZSO)
│              zso.rs (codec LZ4 paralelo)  shrink.rs (relocação de extents)
├── opl/       library.rs (pastas/naming/scan)  ulcfg.rs (USBExtreme+CRC32)
│              cfg.rs (CFG/GSM)
├── vmc/       mod.rs (superblock genvmc, FAT chain, saves, export PSU)
├── scrape/    artwork.rs (ps2-covers/GameTDB)  cheats.rs (PNACH→.cht)
├── deploy/    github.rs (API releases)  fmcb.rs (BOOT_MC + templates)
├── tui/       mod.rs (dashboard Ratatui + tokio select loop)
├── cli.rs     subcomandos clap
└── main.rs    runtime tokio
```

## Roadmap

- [x] v0.1 — parser ISO9660, ZSO, shrink, ul.cfg, deploy PT-BR, capas, cheats, TUI
- [ ] v0.2 — import PSU, validação de fragmentação real (FIEMAP), scrapping de ícones `_ICO`
- [ ] v0.3 — sincronização incremental com o cartão, cache de metadados SQLite
- [ ] v0.4 — UDPBD server integration (rede para o PS2)

## Licença

MIT. OPL é © ps2homebrew (AFL-3.0) — este projeto não redistribui código do OPL, apenas implementa formatos documentados no seu código-fonte.
