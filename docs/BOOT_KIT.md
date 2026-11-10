# Boot Kit Essencial PS2 — Pesquisa & Deploy (Mega Prompt 5)

> Módulo que pesquisa, baixa, valida (SHA256) e monta a pasta `/BOOT_MC/`
> com os ELFs mais atualizados, performáticos e estáveis da cena PS2 —
> com um clique na TUI (`[F3]`) ou um comando no CLI (`ps2-manager bootkit`).

## 1. Estrutura gerada

```text
/BOOT_MC/
├── APPS/
│   ├── OPL.ELF                 ← OPL-Mod-PTBR (OPNPS2LD.ELF do Mod-Kit)
│   ├── WLAUNCHELF.ELF          ← wLaunchELF_ISR (exFAT + MX4SIO/BDM)
│   └── MEMORY_ANNIHILATOR.ELF  ← Memory Card Annihilator (sjpacked)
├── BOOT/
│   ├── BOOT.ELF                ← PS2BBL (COMPRESSED_PS2BBL.ELF, sjpacked)
│   └── CONFIG.INI              ← PS2BBL.INI oficial (atalhos/launch keys)
├── SYS-CONF/
│   └── FREMCB.CNF              ← template FreeMcBoot (R1=OPL PT-BR, R2=uLaunchELF)
└── BOOT-KIT-INFO.txt           ← auditoria: tamanho + SHA256 + fonte de cada asset
```

## 2. Fontes pesquisadas (releases oficiais — incluindo builds automáticos)

| Asset | Repo fonte | Asset escolhido | Por quê |
|---|---|---|---|
| `APPS/OPL.ELF` | `Chrispsz/OPL-Mod-PTBR` | `OPL-PTBR-Mod-Kit-*.zip → OPNPS2LD.ELF` | Release oficial PT-BR com BDM/exFAT; `releases/latest` ignora as tags (todas prerelease) → o cliente cai para a lista completa |
| `APPS/WLAUNCHELF.ELF` | `israpps/wLaunchELF_ISR` | `BOOT-EXFAT-MX4SIO.ELF` | Build da matriz oficial com **exFAT + MX4SIO** (regra do kit: nada de FAT32 fatiado); alternativa p/ MemCard PRO2/SD2PSX: `BOOT-EXFAT-MMCE-XFROM.ELF` |
| `APPS/MEMORY_ANNIHILATOR.ELF` | `ffgriever-pl/Memory-Card-Annihilator` | `MemoryCardAnihilator_*.zip → mca_new/mca-packed.elf` | `mca-packed.elf` já vem **sjpacked** (economia em MC 16 MB); idiomas PT incluídos no zip |
| `BOOT/BOOT.ELF` | `israpps/PlayStation2-Basic-BootLoader` | `PS2BBL.7z → PS2/COMPRESSED_PS2BBL.ELF` (tag `latest` pinnada) | O tag `latest` é o build rodante dev; `releases/latest` retornaria a estável `v1.2.0` sem o asset → source aceita `tag = "latest"` |
| `BOOT/CONFIG.INI` | idem | `PS2BBL.7z → SYS-CONF/PS2BBL.INI` | Config oficial com launch keys (mesmo download, cache de release) |
| `SYS-CONF/FREMCB.CNF` | — | template embutido no binário | Layout Controlado Manual Assistido |
| `APPS/GSM.ELF` | ⚠️ pendente | slot `enabled = false` | **Sem release estável no GitHub hoje** (`ps2homebrew/GSM` 404; forks sem releases). Habilite editando o `bootkit.toml` com uma fonte própria |

### Hashes auditados (megaprompt5 rev1 — 2026-11-07)

```text
dadeabf8ddeed5ea256a914d6b1f6ab1dab7f8396b88c283737266eee5c1df41  APPS/OPL.ELF           (1344244 B)
c828b607e9a0ec69e15fa13dac8f832e91cab0a506e8b4ad1558f9fbc6d795ab  APPS/WLAUNCHELF.ELF    (376740 B)
def84b075f9352c2c66aa49de51d7257d878306d9bc838638d35eb8d9fd0fc72  APPS/MEMORY_ANNIHILATOR.ELF (577892 B)
7c047f78ac2e9084e7acbcb25ab0d2e7734c1b0ae649334ce9f40b33f8d4c1ac  BOOT/BOOT.ELF          (100352 B, sjpacked)
dfcabbb5363f4a70292f985d3c6211018510055790521633bab1bcd523761ee8  BOOT/CONFIG.INI        (479 B)
0032ccc7ed1ad1174186780582a342c78c57bde7883fba3f1dca0f8555c4ca3f  SYS-CONF/FREMCB.CNF    (331 B)
```

## 3. Regras de seleção (filtro de ELFs)

1. **exFAT + BDM nativo** — nenhum utilitário legado que exija FAT32 fatiado.
2. **SHA256 obrigatório** — calculado no download e gravado em `BOOT-KIT-INFO.txt`.
3. **Tamanho otimizado** — variantes `COMPRESSED_*` / `mca-packed.elf` (sjpacked) sempre preferidas.
4. **Releases oficiais** — incluindo builds automáticos `latest` da cena (wLaunchELF_ISR, PS2BBL).
5. **Idempotente** — re-run sem `--overwrite` nunca regrava mídia existente.

## 4. Protocolo de pesquisa contínua

- **CI:** `.github/workflows/weekly-bootkit.yml` monta o kit toda segunda 06:00 UTC
  (e sob demanda via `workflow_dispatch`), publica zip + sha256 como artifact e
  cola o relatório no job summary — se uma fonte quebrar (asset renomeado,
  release removida), o CI fica vermelho e sinaliza a troca de fonte.
- **Manifest:** `bootkit.toml` na raiz do repo é a fonte da verdade; é embutido
  no binário (`include_str!`) e pode ser substituído via `--manifest <arquivo>`.
- **Observação de upstream:** watch em `ps2homebrew/Open-PS2-Loader`,
  `israpps/wLaunchELF_ISR`, `israpps/PlayStation2-Basic-BootLoader`,
  `ffgriever-pl/Memory-Card-Annihilator` para mudanças de driver BDM
  (MX4SIO/SD2PSX/MemCard PRO2), formato ZSO e tradução PT-BR.

## 5. Uso

```bash
# CLI — monta kit na raiz do dispositivo
ps2-manager bootkit /media/$USER/PS2_SD
ps2-manager bootkit --dest /media/$USER/PS2_SD --overwrite --no-configs
ps2-manager bootkit --list              # mostra fontes/variantes do manifest

# TUI — painel [F3] "Deploy de Boot Essencial"
#   ↑/↓ navega · ESPAÇO alterna · ENTER gera · F3 fecha
```

## 6. Biblioteca da Cena — camada EXTRAS (`library.toml`)

Duas camadas de conteúdo, para o memory card **nunca ficar sujo**:

| Camada | Destino | Manifest | Filosofia |
|---|---|---|---|
| **Kit Essencial** | `/BOOT_MC/` (→ MC) | `bootkit.toml` | Enxuto: só a cadeia de boot |
| **Biblioteca** | `/BIBLIOTECA/` (fica na mídia) | `library.toml` | Acervo opcional — copie ao MC só o que usar |

```bash
# CLI
ps2-manager biblioteca /media/$USER/PS2_SD
ps2-manager biblioteca --list     # fontes do acervo
```

Conteúdo curado (zero redundância — cheats/GSM/widescreen já vivem no OPL;
FHDB é só-Fat; MCA já está no Kit):

| Pasta | Asset | Fonte | Por quê |
|---|---|---|---|
| `OPL-UPSTREAM/` | `OPNPS2LD.ELF` | `ps2homebrew/Open-PS2-Loader@latest` | OPL oficial (inglês) p/ testar recursos novos antes do Mod PT-BR |
| `WLAUNCHELF-CARTOES-2EM1/` | `WLAUNCHELF-MMCE-XFROM.ELF` | `israpps/wLaunchELF_ISR@latest` | Variante MMCE/XFROM — SD2PSX e MemCard PRO2 |
| `OPENTUNA/` | `PS2BBL_OpenTuna_Installer.7z` | `israpps/PlayStation2-Basic-BootLoader@opentuna` (asset cru) | Boot p/ Slims finais (BIOS 2.30, via Fortuna) |

**Picks manuais** (sem release automatizável — documentados no `LEIA-ME.txt`):
[FreeMcTuna](https://github.com/israpps/FreeMcTuna) e
[FreeDVDBoot](https://github.com/cturt/FreeDVDBoot) (`PREBUILT_ISO_FILES/`).

- **TUI:** painel [F3] — toggle "Baixar Biblioteca da Cena" (ligado por padrão).
- **CI:** o pipeline semanal também monta e audita `/BIBLIOTECA/` — se uma
  fonte do acervo quebrar, o CI fica vermelho junto com o kit.
