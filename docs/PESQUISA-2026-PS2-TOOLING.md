# Pesquisa 2026 — Ecossistema de Tooling PS2 (foco Slim)

> **Catálogo curado da cena homebrew de PlayStation 2** — compilado em novembro/2026
> para orientar o desenvolvimento e o workflow do
> [ps2-manager](https://github.com/Chrispsz/ps2-manager) (gerenciador Rust/ratatui
> para PS2/OPL no Linux).
>
> **Convenção de honestidade:** itens marcados **(verificar)** não puderam ser
> confirmados em fonte primária — trate como pista, não como fato. Onde não há
> link, a URL não é citada por falta de confiança. A palavra final é sempre o
> teste no próprio console.

**Público-alvo:** dono de um **PS2 Slim** (nunca teve Fat) — tudo organizado em
"o que funciona no MEU console" primeiro, com o Fat como contexto secundário.

---

## 1. Visão geral — a cena PS2 em 2026

O PS2 encerrou a produção em 2013 (a última unidade foi um Slim SCPH-90000
vendido no Japão), mas a cena homebrew nunca esteve tão saudável — 2026 encontra
um ecossistema **maduro e em movimento**:

- **OPL em desenvolvimento ativo**: stable 1.2.0, beta 1.3.x e builds automáticos
  frequentes ("daily") na org
  [ps2homebrew](https://github.com/ps2homebrew/Open-PS2-Loader). Camada universal:
  USB, MX4SIO, cartões virtuais, SMB, HDD (Fat) e UDPBD — cheats, widescreen
  patches, GSM e PADEMU **integrados** (§3).
- **A revolução do armazenamento chama-se MX4SIO**: microSD no slot de memory
  card, a preço de adaptador, aposentando o USB 1.1 no Slim (§3.2).
- **Cartões "2-em-1"** — SD2PSX (open-source, RP2040) e MemCard PRO2 (8BitMods,
  comercial) — unificam VMC + armazenamento de jogos no mesmo cartão (§3.3/§3.4).
- **Os últimos Slims foram "destravados"**: o FMCB clássico nunca bootou nos
  consoles finais (BIOS 2.30), mas **Fortuna** (exploit via Browser) e
  **FreeMcTuna** (fork mantido do FMCB) cobrem essa lacuna (§2).
- **A tooling de PC migrou do Windows para multiplataforma** — o
  [ps2-manager](https://github.com/Chrispsz/ps2-manager) é a peça nativa Linux:
  ISO/ZSO, staging em SSD, anti-desastre e boot kit em um binário Rust (§8.4).
- **IA entrou na periferia, não no núcleo**: traduções fan-made assistidas por
  IA, capas restauradas e documentação — sem substituir ferramentas core (§8.3).

**Panorama por geração de hardware** (detalhes na matriz do §2.9):

| Geração | Modelos | Situação em 2026 |
|---|---|---|
| Fat (bay de HDD) | SCPH-10000–5000x | Melhor I/O nativo (HDD interno), mas requer hardware de expansão raro/caro |
| Slim (top-load) | SCPH-70000–79000 | FMCB funciona direto; Ethernet integrado; sem HDD |
| Slim final | SCPH-90000+ | Depende da BIOS: 2.20 = FMCB ok · 2.30 = Fortuna/FreeMcTuna |

**Nota Brasil:** o PS2 foi montado localmente pela Sony em Manaus; para homebrew
o que importa é a **BIOS**, não a origem da carcaça.

---

## 2. Pontos de entrada / Exploits

O padrão moderno da cena é **softmod via memory card**: um arquivo no MC que o
OSDSYS (o "Browser" do console) executa no boot. A escolha depende do modelo/BIOS.

### 2.1 FreeMcBoot (FMCB) — o clássico

- **O que é:** installer que grava no MC um hook de boot via OSDSYS. Com o MC
  inserido, o console oferece um menu de ELFs; sem ele, fica 100% original.
- **Estado:** desenvolvimento clássico parado na versão **1.966** (mantida por
  sp193) — ainda "o auge" em estabilidade, mas sem novidades.
- **Compatibilidade:** Fat e Slim **até BIOS ~2.20**. No SCPH-90000/BIOS 2.20
  funciona via "cross-flashing" (grava parte do exploit na flash interna do
  console — o MC fica **amarrado àquele console**) **(verificar detalhes do
  fluxo 90000/2.20 antes de documentar ao usuário final)**.
- **Multi-install:** o mesmo MC boota em **qualquer** console suportado (exceto
  90000) — útil para preparar cartões num PS2 e usar noutro.
- **Limite duro:** BIOS **2.30** (produção final dos 90000) fecha o vetor — aí
  entram Fortuna e FreeMcTuna.

### 2.2 FreeMcTuna — o FMCB vivo

- **O que é:** fork **mantido** do FreeMcBoot:
  [github.com/israpps/FreeMcTuna](https://github.com/israpps/FreeMcTuna).
- **Diferencial:** base FMCB modernizada que **também funciona nos Slims
  tardios** (o terreno da BIOS 2.30, onde o FMCB clássico não boota)
  **(verificar cenário exato por modelo — em 2.30 o installer normalmente é
  lançado via Fortuna ou FreeDVDBoot)**.
- **Por que usar:** o caminho "estilo FMCB" recomendado em 2026 — mesmo menu e
  conceitos, código vivo; o `bootkit` gera o `SYS-CONF/FREMCB.CNF` compatível.
- **Parente próximo:** FunTuna (israpps) — instalação automática combinada com
  Fortuna para consoles finais **(verificar estado atual antes de recomendar)**.

### 2.3 Fortuna — o exploit dos últimos Slims

- **O que é:** exploit para os consoles **finais** (SCPH-900xx+, BIOS 2.30) onde
  o FMCB não boota — vetor independente do OSDSYS do FMCB: **salva arquivos no
  MC que são processados pelo Browser**; ao acessar o cartão, injeta um menu
  que executa ELFs.
- **Uso típico:** 90000/2.30 sem nenhum mod → grava-se o Fortuna no MC (via
  FreeDVDBoot, outro console modado, ou **via PC** com SD2PSX/MemCard PRO2,
  que expõem o microSD no computador) → Browser → menu.
- **Limitações:** o menu executa ELFs preparados para o formato dele — nem todo
  ELF solto roda direto **(verificar restrições exatas)**. Na prática é o
  *primeiro degrau* para o que interessa (OPL, wLaunchELF, PS2BBL).

### 2.4 FreeDVDBoot — boot por DVD gravado

- **O que é:** exploit do **player de DVD** (decoder MPEG), por cturt:
  [github.com/cturt/FreeDVDBoot](https://github.com/cturt/FreeDVDBoot). Um DVD-R
  com arquivos especialmente montados transborda o player e executa homebrew —
  **sem chip, sem MC, sem nada instalado**.
- **Compatibilidade:** todos os modelos testados pela cena, incluindo 90000/BIOS
  2.30; a imagem gravada varia conforme a versão do DVD player (região/fábrica).
- **Papel em 2026:** o **kit de primeiros socorros** — forma universal de rodar
  um ELF em qualquer PS2 limpo: instalar o FreeMcTuna, gravar o Fortuna no MC,
  rodar o Memory Card Annihilator.

### 2.5 FHDB — FreeMcBoot no HDD (SÓ Fat)

- **O que é:** FreeHardDiskBoot — o conceito do FMCB instalado no **HDD interno**
  (expansion bay); boota pelo HDD sem memory card.
- **Por que o Slim fica de fora:** o Slim **não tem expansion bay** — não existe
  "HDD interno" nele, ponto. Todo o universo HDD (HDL, HDL Dump, pfsshell,
  partições APA) é exclusividade Fat e irrelevante para o workflow Slim (muita
  doc antiga assume HDD; não se aplique).

### 2.6 PS2BBL — bootloader configurável

- **O que é:** [PlayStation2 Basic BootLoader](https://github.com/israpps/PlayStation2-Basic-BootLoader)
  (israpps) — bootloader de memory card **configurável via `CONFIG.INI`**:
  auto-boot com timer (ex.: entra direto no OPL após 5 s), atalhos por botão
  para outros ELFs (OPL / wLaunchELF / Annihilator), logo, "modo criança" etc.
- **Por que importa:** boot de console moderno — liga → OPL. É o `BOOT/BOOT.ELF`
  do boot kit do ps2-manager (com `CONFIG.INI` oficial).
- **Compatibilidade:** roda onde um entry point já existe (FMCB, FreeMcTuna,
  Fortuna) — em 90000/2.30 é lançado pelo Fortuna **(verificar combinação exata
  recomendada para 2.30)**.

### 2.7 wLaunchELF_ISR — o canivete suíço, mantido

- **O que é:** fork mantido do clássico uLaunchELF/wLaunchELF (gerenciador de
  arquivos + launcher de ELFs) com foco em **BDM/exFAT**:
  [github.com/israpps/wLaunchELF_ISR](https://github.com/israpps/wLaunchELF_ISR).
- **Diferenciais:** builds com drivers modernos (exFAT, MX4SIO, MMCE para
  SD2PSX/MemCard PRO2, XFROM) — copia arquivos de/para o microSD do MX4SIO e
  cartões virtuais, coisa que o wLaunchELF antigo (FAT32 só) não fazia.
- **Papel no workflow:** usado **uma vez** para copiar `BOOT_MC/` do SD para o
  MC; depois fica como utilitário de emergência. O `bootkit` usa o build
  `BOOT-EXFAT-MX4SIO.ELF` (ou `BOOT-EXFAT-MMCE-XFROM.ELF` para SD2PSX/MCP2).

### 2.8 Matriz de compatibilidade — ferramenta × console

| Ferramenta (entry point) | Fat (1x000–5000x) | Slim 7x000–79000 | Slim 90000 (BIOS 2.20) | Slim 90000+ (BIOS 2.30) |
|---|:---:|:---:|:---:|:---:|
| FreeMcBoot 1.966 (clássico) | ✅ | ✅ | ✅ (install por console) | ❌ |
| [FreeMcTuna](https://github.com/israpps/FreeMcTuna) | ✅ | ✅ | ✅ | ✅ **(verificar)** |
| Fortuna | ✅ (desnecessário) | ✅ (desnecessário) | ✅ (desnecessário) | ✅ **alvo principal** |
| [FreeDVDBoot](https://github.com/cturt/FreeDVDBoot) | ✅ | ✅ | ✅ | ✅ |
| FHDB (HDD interno) | ✅ | ❌ sem bay | ❌ | ❌ |
| [PS2BBL](https://github.com/israpps/PlayStation2-Basic-BootLoader) | ✅ (via entry point) | ✅ | ✅ | ✅ via Fortuna/FreeMcTuna |
| [wLaunchELF_ISR](https://github.com/israpps/wLaunchELF_ISR) | ✅ | ✅ | ✅ | ✅ via entry point |

### 2.9 Matriz por modelo — qual caminho seguir

| Modelo | Período aprox. | Entry point recomendado | Observações |
|---|---|---|---|
| SCPH-10000–39000 (Fat) | 2000–2003 | FMCB ou FreeMcTuna | Os primeiros (Japão) usavam PCMCIA para rede **(verificar)** |
| SCPH-50000 (Fat) | 2003–2004 | FMCB ou FreeMcTuna | Última Fat; ainda tem expansion bay |
| SCPH-70000–79000 (Slim) | 2004–2007 | **FMCB/FreeMcTuna direto** | Fonte externa (tijolo), Ethernet integrado, sem HDD |
| SCPH-90000 (Slim, BIOS **2.20**) | ~2006–2008 **(verificar)** | **FMCB funciona** | Fonte interna; install "cross-flash" amarra o MC ao console |
| SCPH-90000+ (Slim, BIOS **2.30**) | final da produção | **Fortuna + FreeMcTuna** (ou FreeDVDBoot p/ instalar) | FMCB clássico bloqueado |

**Como saber a sua situação sem abrir nada:** se o FMCB/FreeMcTuna de um cartão
pronto bootar no seu 90000, é BIOS 2.20; se não bootar, é 2.30 → caminho Fortuna
**(verificar — heurística de produção: fim da linha tende a 2.30; nada substitui o teste)**.

---

## 3. Carregadores & armazenamento

### 3.1 OPL — Open PS2 Loader (o coração de tudo)

Repo: [github.com/ps2homebrew/Open-PS2-Loader](https://github.com/ps2homebrew/Open-PS2-Loader)

| Aspecto | Estado |
|---|---|
| Stable | **1.2.0** |
| Beta | linha **1.3.x** **(verificar número exato da corrente beta)** |
| Builds automáticos | frequentes ("daily"), sempre com drivers/patches atualizados **(verificar canal/cadência atual)** |

**Fontes de jogos (camada BDM — Block Device Manager):**

| Fonte | Slim? | Formato | Notas |
|---|:---:|---|---|
| **MX4SIO** (microSD no slot de MC) | ✅ | exFAT, ISO/ZSO | O melhor custo-benefício no Slim (§3.2) |
| **MMCE** (SD2PSX / MemCard PRO2) | ✅ | exFAT, ISO/ZSO | Armazenamento via cartão de memória virtual (§3.3/§3.4) |
| **USB (mass:)** | ✅ | exFAT/FAT32, ISO/ZSO | Universal mas **USB 1.1** = lento (§3.5) |
| **SMB (rede)** | ✅ | ISO/ZSO no PC/NAS | Coleções grandes sem mídia removível (§7) |
| **UDPBD (rede)** | ✅ | ISO/ZSO servidas via UDP | Servidor simples no PC; no roadmap do ps2-manager |
| **HDD interno (APA/HDL)** | ❌ só Fat | HDD no expansion bay | O mais rápido no Fat; inexistente no Slim |

**Recursos integrados ao OPL (sem ELFs externos):**

- **ISO e ZSO** nativos (ZSO = ISO comprimida em LZ4, §3.7);
- **exFAT** nos dispositivos BDM nos builds modernos (1.2.0+): arquivos >4 GB
  sem fatiamento `ul.` (o formato ul.cfg/USBExtreme segue suportado para FAT32
  legado; o ps2-manager gera os dois);
- **Cheats PS2RD** por jogo (`CHT/<ID>.cht`) — §6.1;
- **Widescreen patches por jogo** — banco compilado no OPL — §5.1;
- **GSM** (modo de vídeo por jogo, via opção ou `CFG/$GSMVMode` — §5.3) e
  **PADEMU** (controles USB/DS3/DS4 — §6.2);
- **VMC** (memory cards virtuais por jogo, `VMC/<nome>.bin`) — §4.3;
- **Themes/capas** (`THM/`, `ART/<ID>_COV.png` 140×200) e translations — o
  [OPL-Mod-PTBR](https://github.com/Chrispsz/OPL-Mod-PTBR) é a mod PT-BR
  mantida pelo autor deste gerenciador;
- **APPS**: aba que lança ELFs de `APPS/` (o boot kit expõe wLaunchELF e o
  Annihilator dentro do próprio OPL).

### 3.2 MX4SIO — o storage definitivo do Slim

- **O que é:** "Memory card eXpansion for SIO2" — **adaptador PCB que encaixa no
  slot de memory card** e aceita um microSD, falando o protocolo do slot (SIO2)
  a alta velocidade. Driver integrado ao OPL/ps2sdk pela cena; adaptadores são
  produzidos pela comunidade e vendidos por lojas como a
  [8BitMods](https://8bitmods.com) **(verificar disponibilidade e revisão do PCB)**.
- **Por que é o melhor custo-benefício para Slim:**
  - **Rápido**: o barramento do slot de MC é muito mais veloz que o USB 1.1;
    ganho grande e perceptível em loading (a cena relata desempenho próximo ao
    HDD interno de Fat — **verificar benchmarks por título antes de prometer números**);
  - **Barato**: adaptador + microSD por uma fração de qualquer outra opção;
  - **Simples**: exFAT direto, sem fatiamento, sem fonte extra, sem fiação.
- **No OPL:** drivers inclusos nos builds atuais (o `bootkit` usa OPL-Mod-PTBR e
  wLaunchELF_ISR com suporte MX4SIO prontos).
- **Cuidados:** use microSD de marca (A1/A2/classe alta) — falsificados de
  "grande capacidade" são o mesmo problema dos MCs falsos (§4.1). Com o
  adaptador montado o slot deixa de ser MC (saves vão para VMC no microSD).

### 3.3 SD2PSX — o cartão open-source (RP2040)

- **O que é:** cartão de memória virtual **open-source** (hardware + firmware)
  baseado no **RP2040** (chip do Raspberry Pi Pico) com slot microSD; projeto na
  org [github.com/sd2psx](https://github.com/sd2psx).
- **Recursos:** emula o memory card com **múltiplos cartões virtuais**
  comutáveis no próprio cartão (troca por botão, sem PC); funciona também como
  **armazenamento BDM (modo MMCE)** para o OPL — o mesmo microSD carrega saves E
  jogos —; e tem **firmware atualizável**.
- **Público:** o "cartão definitivo" open-source — opcional neste workflow:
  MX4SIO + um MC 8 MB oficial cobrem 90% do uso por menos. Como o microSD é
  acessível no PC, instalar Fortuna/FreeMcTuna em consoles finais vira cópia
  trivial de arquivos.

### 3.4 MemCard PRO2 — o cartão comercial (8BitMods)

- **O que é:** produto comercial da [8BitMods](https://8bitmods.com): cartão de
  memória virtual com **microSD**, **até 8 cartões virtuais** comutáveis por
  botão no próprio cartão e **USB-C** para gerenciamento pelo PC.
- **No OPL:** suportado como BDM (modo MMCE) — carrega jogos além dos saves,
  como o SD2PSX. O `bootkit` prevê o build `BOOT-EXFAT-MMCE-XFROM.ELF` do
  wLaunchELF_ISR para esse cenário.

### 3.5 USB (mass:) — universal, mas lento

- Funciona em **todo** PS2 (2 portas USB 1.1) e é o caminho mais documentado em
  guias antigos. O gargalo é físico: **USB 1.1 = 12 Mbit/s** de enlace (~1 MB/s
  útil na prática) — jogos pesados sofrem stutter de streaming (GoW, GTA, GT4…).
- Veredicto para Slim em 2026: **fallback/transporte** — para a experiência
  certa, migre para MX4SIO.

### 3.6 SMB via OPL — a biblioteca no PC

- O OPL monta compartilhamentos **SMB** (Windows/Linux/NAS) pela Ethernet
  integrado do Slim: a coleção inteira mora no PC, o console só aponta.
- Ideal para **coleções grandes** (terabytes) — o "jukebox mode" do PS2. Requer
  infra de rede e um pouco de configuração (IP/credenciais no OPL).
- Complemento: **UDPBD** (BDM sobre UDP) para servir jogos de forma mais simples
  que SMB — integração no ps2-manager planejada (roadmap v0.4).

### 3.7 ZSO — compressão LZ4 padrão da cena

- **ZSO/ZISO:** imagem ISO9660 comprimida bloco-a-bloco em **LZ4** com índice de
  blocos; o OPL descomprime on-the-fly. Padrão da cena para economizar espaço
  em microSD/USB sem converter o jogo.
- **Trade-off real:** troca I/O por CPU. Em **mídia lenta (USB)** a compressão
  pode até **melhorar** o carregamento (menos bytes pela interface lenta); em
  mídia rápida (MX4SIO/HDD) o custo de descomprimir aparece. Comportamento
  reportado pela cena — **teste por jogo (verificar)**; não há número universal.
- **Ferramentas clássicas:** `pc/ziso.py` do próprio OPL é o conversor de
  referência; **ps2-packer** / **sjpack** comprimem **ELFs** (homebrew), não
  ISOs — por isso os ELFs do boot kit vêm "sjpacked" (menor consumo de MC).
  O **ps2-manager** implementa ZSO em Rust puro (`lz4_flex`, multithread) com
  compatibilidade binária com o driver do OPL — sem Python, sem Wine.
- **Cuidado:** ZSO exige o driver do OPL — não espere bootar em outros
  loaders/emuladores antigos (no PCSX2, prefira a ISO original para o QA do §8.2).

---

## 4. Memory Card — ferramentas

### 4.1 Memory Card Annihilator v2 — A ferramenta de formatação

Repo: [github.com/ffgriever-pl/Memory-Card-Annihilator](https://github.com/ffgriever-pl/Memory-Card-Annihilator)

É a resposta curta para "qual ferramenta formata memory card de PS2" — e faz mais:

| Função | Para quê |
|---|---|
| **Formatação completa** | Formata o MC de verdade (não só "apagar arquivos"), reconstruindo o sistema de arquivos |
| **Correção de superbloco** | Conserta cartões que o console acusa como corrompidos/sem formato |
| **Clonagem** | Duplica um MC noutro (migração de saves com segurança) |
| **Backup/restore** | Imagem completa do cartão para o PC e volta |
| **Teste de capacidade real** | **Revela a capacidade REAL de cartões falsificados** — os "128 MB" chineses geralmente têm 8/16/64 MB reais com firmware mentiroso; o Annihilator escreve/lê além do limite real e escancara a fraude |

- Interface multilíngue (inclui PT) e variante comprimida (`mca-packed.elf` — a
  que o `bootkit` do ps2-manager baixa para `APPS/`, com SHA256 auditado).
- **Recomendação:** para saves, prefira **MC Sony oficial de 8 MB** (usado
  serve) a genéricos de "128 MB" — falsificação altíssima; se já tem um, passe
  no Annihilator para saber a capacidade real.

### 4.2 Gerenciamento de saves no PC (legado)

- **mymc** — utilitário em Python (legado, anos 2000): importa/exporta saves
  de/para imagens de MC; referência para lote via linha de comando.
- **PS2 Save Builder** — GUI clássica de Windows (roda em Wine). Legado puro.
- **Formatos de save da cena:** `.psu` (backup direto de pasta do MC — o mais
  "sem perdas"), `.max` (Action Replay MAX), `.cbs` (CodeBreaker), `.sps`
  (SharkPort/X-Port) **(verificar origem exata do .sps)**.
- **Estado 2026:** para saves no Linux, o caminho moderno é VMC/ps2-manager
  (`vmc list` / `vmc export` para `.psu`) — o import reverso está no roadmap (v0.2).

### 4.3 VMC — memory cards virtuais do OPL

- O OPL cria **VMCs por jogo** (arquivos em `VMC/`): cada jogo ganha seu cartão
  virtual, imune a corrupção do MC físico e a "save crushing" de gerenciador.
- Tamanhos múltiplos suportados (8 MB nativo; superiores via genvmc
  **(verificar teto de tamanho da versão atual)**).
- **Avisos:** uma minoria de jogos tem incompatibilidade com VMC (lista na wiki
  do OPL) **(verificar)** — nesses casos, salve no MC físico. VMC no mesmo
  microSD dos jogos (via MX4SIO/MMCE) é a configuração mais elegante para Slim.

---

## 5. Vídeo & Widescreen hacks

### 5.1 Widescreen patches integrados ao OPL (o caminho "mais atualizado")

- O OPL traz **widescreen patches por jogo compilados no próprio loader** —
  basta habilitar **"Widescreen patches"** nas opções do jogo (menu per-game).
  Sem baixar nada, sem pendrive de patches.
- O banco é mantido pela comunidade **ps2-wide** (patches `.pnach` por Game ID,
  mesmos conceitos dos widescreen hacks do PCSX2) e **chega atualizado junto com
  os builds do OPL** — mais uma razão para usar builds recentes/daily em vez de
  ELFs antigos de tutorial **(verificar se o seu ELF stable 1.2.0 já inclui o
  banco da época do build)**.
- **Aviso honesto:** o hack é **por-jogo** — só existe patch onde a comunidade
  escreveu um. Jogos sem patch ficam em 4:3. Não há conversão 16:9 universal no
  PS2 real.

### 5.2 Cabos e 480p nativo

- **Cabo componente (YPbPr)** é praticamente obrigatório para o melhor vídeo no
  Slim — **atenção: o conector AV do Slim é diferente do Fat; cabos não são
  intercambiáveis** entre as famílias.
- Com componente, jogos com **"Progressive Scan" nativo** (opção no próprio
  jogo, segurando botões no boot — GT4 e posteriores) rodam em **480p**.
- RGB (SCART) é ótimo para 480i em CRTs europeus, mas **480p no PS2 sai por
  componente**, não por RGB — planeje o cabo conforme a TV.

### 5.3 GSM — GS Mode Selector

- **O que é:** força modos de vídeo por jogo (480p, 576p, 720p, 1080i, modos
  VGA) alterando registradores do GS (chip de vídeo). **Integrado ao OPL**:
  opção per-game e via `CFG/<ID>.cfg` com a chave `$GSMVMode` (o
  `ps2-manager cfg --gsm-vmode …` gera isso).
- **Usos no Slim + HDTV:** 480p em jogos sem progressive scan nativo; 720p/1080i
  como experimento — **expectativa calibrada**: forçar modos altos cobra da
  memória do GS e pode gerar slowdown/artefatos por jogo.
- **Modos VGA:** dependem de cabo componente com sync-on-green ou transcodificador
  **(verificar detalhes antes de prometer "VGA direto na mesa")**.
- **Regra de ouro:** GSM é "por jogo" — ligue, teste, decida. O per-game CFG do
  OPL/ps2-manager existe exatamente para isso.

---

## 6. Cheats & Controles

### 6.1 Cheats — engine PS2RD do OPL

- O OPL tem **cheat engine nativa (PS2RD)**: ative "Enable Cheats" e o jogo lê
  `CHT/<ID>.cht` — texto com códigos raw (formato do `cheatman.c` do OPL:
  16 dígitos hex, tipos byte/halfword/extended).
- **Fluxo moderno no Linux:** códigos circulam como **`.pnach` do PCSX2** — o
  `ps2-manager cheats --pnach-dir …` **converte PNACH → .cht automaticamente**,
  mapeando os tipos para raw compatível com PS2RD.
- Cuidados: cheats de "mestre" mal convertidos travam o jogo — teste um por vez;
  cheats que dependem de hooks de emulador não têm equivalente no console real.

### 6.2 PADEMU — controles modernos no OPL

- **PADEMU** (integrado ao OPL, per-game): suporta **controles USB genéricos** e
  **DualShock 3/4** — DS3/DS4 via USB e, em builds com suporte, sem fio com
  dongle Bluetooth compatível **(verificar lista de dongles)**. No Slim resolve
  o caso "meu DualShock 2 está morrendo".
- **Aviso:** a camada USB/BT pode introduzir latência perceptível em jogos de
  ritmo/precisão — para competição, DualShock 2 original segue padrão-ouro.

---

## 7. Rede & Online

- **OPL SMB** (§3.6) — a rede a serviço dos jogos locais: coleção no PC/NAS,
  Ethernet integrado do Slim.
- **UDPBD** — alternativa leve para servir jogos do PC ao OPL via UDP; integração
  no ps2-manager no roadmap (v0.4).
- **XLink Kai** ([xlink.ai](https://xlink.ai)) — **tunneling LAN over internet**:
  jogos com modo LAN jogam "online" entre consoles do mundo inteiro — revive
  **SOCOM II** e **TimeSplitters: FP** (GT4 também tem LAN **(verificar)**).
  Requer o client Kai no PC e o console na mesma rede/bridge dele.
- **Servidores oficiais** da Sony para PS2 encerraram ao longo de 2012–2014,
  título a título **(verificar datas exatas por jogo)**.
- **Renascimento comunitário:** grupos da cena mantêm servidores revival para
  títulos clássicos (SOCOM e outros) e há iniciativas tipo **OpenSpy** para
  jogos que usavam GameSpy **(verificar estado atual de cada serviço antes de
  recomendar)**. O XLink Kai segue a via mais estável por não depender de
  server-side do jogo.

---

## 8. PC-side & IA

### 8.1 OPL Manager (Windows/Wine) — o incumbente

- **OPL Manager** é o gerenciador de PC mais usado da cena (Windows): renomeia
  jogos, baixa capas, organiza cheats/VMCs/temas, converte mídias. No Linux, só
  via **Wine** — e é exatamente o espaço que o **ps2-manager** ocupa como
  **alternativa nativa**, em um binário estático.

### 8.2 PCSX2 — QA do workflow

- [pcsx2.net](https://pcsx2.net) — o emulador de referência. No contexto de
  console real, é a **bancada de teste**: antes de gravar uma ISO duvidosa no
  microSD (dumps antigos, "rips" de procedência desconhecida), **boota no
  emulador** — se travar aí, nem perca tempo com o cartão. Também é onde se
  valida widescreen/cheats antes de converter para OPL.

### 8.3 O que a IA trouxe de novo (resposta honesta)

"O que a IA trouxe de novo para a cena PS2?" — resposta menos espetacular que o
hype: **nenhuma ferramenta core (loader, exploit, formateador de MC) é feita por
IA**. O impacto real está na periferia:

| Área | Impacto da IA | Exemplo |
|---|---|---|
| **Traduções fan-made** | Alto — o maior ganho: tradução assistida por IA derrubou o custo de patchear RPGs japoneses nunca traduzidos | Traduções comunitárias recentes de títulos JP **(verificar cada projeto antes de citar)**; o próprio [OPL-Mod-PTBR](https://github.com/Chrispsz/OPL-Mod-PTBR) é localização de tooling mantida por humano com ajuda de ferramentas |
| **Arte/capas** | Médio — restauração e upscale de scans de capa para `ART/` | Bases curadas como [xlenore/ps2-covers](https://github.com/xlenore/ps2-covers) (possivelmente com itens aprimorados por IA **(verificar metodologia por item)**) |
| **Documentação/wiki** | Médio — resumos, traduções de docs antigas, guias | Risco conhecido: material de IA sem revisão propaga erros de versão/BIOS — daí a regra "(verificar)" deste documento |
| **Scraping de metadados** | Baixo/médio — enrichment de bancos de dados | Fontes tipo IGDB/vgmuseum seguem sendo as bases **(verificar políticas de uso)** |
| **Ferramentas core** | Nulo até agora | Nenhum exploit/loader/packer "por IA"; formatos binários (ZSO, ul.cfg, .cht) continuam trabalho humano validado contra o código-fonte |

**Síntese:** IA ajuda a cena a **traduzir, ilustrar e documentar** — o núcleo
técnico (OPL, Fortuna, MCA, formatos) permanece engenharia reversa clássica.
Para o ps2-manager, uso sensato de IA é no ecossistema em volta (metadados,
arte), nunca nas decisões de binário (sempre validadas contra o upstream do OPL).

### 8.4 ps2-manager — a peça nativa Linux

[github.com/Chrispsz/ps2-manager](https://github.com/Chrispsz/ps2-manager) —
gerenciador **100% Rust** nativo Linux para PS2/OPL:

- **Parsing ISO9660/ZSO sem mount** (Game ID direto do SYSTEM.CNF) e
  **conversão ZSO multithread** compatível bit-a-bit com o driver do OPL;
- **Pipeline anti-desastre**: staging em SSD (`~/.cache`), validação, `fallocate`
  + streaming na mídia, rollback automático — cancelamento nunca deixa arquivo
  parcial no cartão;
- **Seletor de mídia seguro**: lista só removíveis/USB/SD — discos do PC nunca
  aparecem;
- **Boot Kit Essencial** (`ps2-manager bootkit`): monta `BOOT_MC/` com
  OPL-Mod-PTBR + wLaunchELF_ISR + PS2BBL + Memory Card Annihilator, com SHA256
  e fonte de cada asset auditados (ver [docs/BOOT_KIT.md](BOOT_KIT.md));
- **Capas (ps2-covers), cheats (PNACH→CHT), CFG/GSM, VMC, ul.cfg** — tudo em um
  binário + **AppImage de 2 cliques** com TUI ratatui (mouse incluso) — a ponte
  entre o "PC com a coleção" e o "PS2 Slim na sala".

---

## 9. Workflow recomendado para PS2 Slim

O fluxo abaixo é o desenho-alvo deste projeto — cada passo mapeia a uma
ferramenta do catálogo acima.

### Passo 0 — Material (checklist de compra)

| Item | Por quê |
|---|---|
| PS2 Slim (o que você já tem) | — |
| Adaptador **MX4SIO** + microSD de marca (128–256 GB+) | Storage rápido e barato (§3.2) |
| Cabo **componente** para Slim | 480p/widescreen decente em HDTV (§5.2) |
| Memory Card **oficial 8 MB** Sony (usado serve) | Entrada de boot + saves à prova de fraude (§4.1) |
| (Opcional) SD2PSX ou MemCard PRO2 | Cartão definitivo: VMC + jogos no mesmo hardware (§3.3/§3.4) |

### Passo 1 — Boot Kit no memory card

```bash
# no PC Linux:
ps2-manager bootkit /media/$USER/PS2_SD
```

- Gera `BOOT_MC/` com **OPL-Mod-PTBR + wLaunchELF_ISR (exFAT/MX4SIO) + PS2BBL
  (CONFIG.INI com auto-boot) + Memory Card Annihilator**, com SHA256 de tudo.
- No console (uma única vez): use o entry point do seu modelo (§2.9) — no seu
  Slim, FMCB/FreeMcTuna; em 90000/2.30, Fortuna — e com o **wLaunchELF_ISR**
  copie `BOOT_MC/` do microSD para o memory card.
- Resultado: liga o console → PS2BBL → **OPL PT-BR** em segundos.

### Passo 2 — Preparar o microSD (MX4SIO) em exFAT

- Formate o microSD em **exFAT** no PC (sem FAT32 fatiado) — OPL e
  wLaunchELF_ISR atuais leem exFAT direto (§3.1).
- `ps2-manager devices` confirma que o cartão é reconhecido como mídia segura
  (nenhum disco interno aparece — anti-desastre).

### Passo 3 — Instalar jogos como ZSO (staging em SSD → fallocate)

```bash
ps2-manager install GranTurismo4.iso /media/$USER/PS2_SD --zso
```

- Conversão ZSO no **SSD do PC** (rápida, paralela), validação de integridade
  (header + Game ID) e só então escrita no microSD com `fallocate` + streaming —
  o cartão nunca recebe arquivo parcial.
- Naming/pastas no padrão OPL (`DVD/`/`CD/`, `GAMEID.Título.zso`).

### Passo 4 — Capas, cheats e CFGs automáticos

```bash
ps2-manager covers /media/$USER/PS2_SD      # ART/<ID>_COV.png 140×200
ps2-manager cheats /media/$USER/PS2_SD --pnach-dir ~/pnach   # PNACH → CHT
ps2-manager cfg    /media/$USER/PS2_SD --gsm-vmode 2          # GSM por jogo
```

- No OPL: ative "Widescreen patches" e "Enable Cheats" por jogo (§5.1, §6.1).

### Passo 5 — QA no PCSX2 antes de gravar

- ISO duvidosa? **Boota no [PCSX2](https://pcsx2.net) primeiro** (§8.2). Passou
  no emulador → instala no cartão. Falhou → poupe o microSD e a paciência.

### Mapa do fluxo

```text
PC Linux ── ps2-manager (staging SSD, ZSO, capas/cheats/cfg) ──► microSD exFAT (MX4SIO)
                                                                     │
PC ── PCSX2 (QA da ISO) ─────────────────────────────────────────────┤
                                                                     ▼
PS2 Slim ── MC c/ boot kit (FMCB/FreeMcTuna ─ PS2BBL ─ OPL PT-BR) ──► jogos
              └── wLaunchELF_ISR (emergência) · MCA (formata/verifica MC)
```

---

## 10. Fontes

### URLs com alta confiança (citadas no texto)

| Projeto | URL |
|---|---|
| Open PS2 Loader | https://github.com/ps2homebrew/Open-PS2-Loader |
| FreeMcTuna | https://github.com/israpps/FreeMcTuna |
| PS2BBL (PlayStation2 Basic BootLoader) | https://github.com/israpps/PlayStation2-Basic-BootLoader |
| wLaunchELF_ISR | https://github.com/israpps/wLaunchELF_ISR |
| FunTuna (israpps) | https://github.com/israpps/FunTuna |
| FreeDVDBoot | https://github.com/cturt/FreeDVDBoot |
| Memory Card Annihilator | https://github.com/ffgriever-pl/Memory-Card-Annihilator |
| SD2PSX (org) | https://github.com/sd2psx |
| OPL-Mod-PTBR | https://github.com/Chrispsz/OPL-Mod-PTBR |
| ps2-manager | https://github.com/Chrispsz/ps2-manager |
| ps2-covers (capas) | https://github.com/xlenore/ps2-covers |
| 8BitMods (MemCard PRO2, loja) | https://8bitmods.com |
| PCSX2 | https://pcsx2.net |
| XLink Kai | https://xlink.ai |

### Projetos citados SEM URL (confiança insuficiente para linkar)

- **FreeMcBoot (FMCB)** — versão clássica 1.966; buscar "FreeMcBoot 1.966" no
  fórum da cena **(verificar canal oficial de download)**.
- **Fortuna** — exploit para BIOS 2.30; buscar "Fortuna PS2" (repositório
  original e forks tipo Fortuna-mod) **(verificar)**.
- **FHDB** — FreeHardDiskBoot (Fat only) **(verificar)**.
- **GSM** — GS Mode Selector (integrado ao OPL; projeto original sem URL
  confiável atual) **(verificar)**.
- **ps2-wide** — banco de patches widescreen **(verificar repositório atualizado)**.
- **mymc** / **PS2 Save Builder** — ferramentas de save legadas **(verificar)**.
- **ps2-packer** / **sjpack** — compressão de ELFs **(verificar repositório
  canônico atual)**.
- **HDL Dump** / **pfsshell** — tooling HDD (Fat only) **(verificar)**.
- **OPL Manager** — gerenciador Windows **(verificar página oficial atual)**.
- **OpenSpy** e servidores revival (SOCOM etc.) — **(verificar estado atual)**.

> **Regra de manutenção:** se uma fonte acima mudar de URL ou versão, atualize
> aqui e no `bootkit.toml` (fonte da verdade dos assets do `ps2-manager bootkit`).
> O CI semanal do boot kit é o detector natural de links mortos.

---

*Documento: `docs/PESQUISA-2026-PS2-TOOLING.md` · Compilado 2026-11 · Uso interno do projeto ps2-manager · Mantenha o espírito "(verificar)" ao editar.*
