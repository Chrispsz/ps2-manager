//! Conversor PNACH (PCSX2) → `.cht` nativo do OPL.
//!
//! Formato .cht (réplica de `src/cheatman.c` do OPL):
//!   • comentários: linhas iniciadas por `//` ou `#`
//!   • código: linha de exatamente 16 hex digits — `AAAAAAAA VVVVVVVV`
//!   • qualquer linha não-vazia que não é código = nome do cheat seguinte
//!   • arquivo: `CHT/<GAMEID>.cht` (supportbase.c:859)
//!   • nomes "mastercode"/"master code"/"enable code (must be on)" ficam
//!     sempre ativos no modo select (cheatconfig.c:40-49)
//!
//! Mapeamento de tipos PNACH → raw CB/PS2rd (o tipo vai nos bits altos do
//! endereço): byte → 0x0xxxxxxx, short/word → 0x1xxxxxxx, extended → 0x2xxxxxxx.

use crate::error::Result;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy)]
pub struct RawCode {
    pub address: u32,
    pub value: u32,
}

#[derive(Debug, Clone)]
pub struct CheatGroup {
    pub name: String,
    pub codes: Vec<RawCode>,
}

fn norm_value(v: &str) -> Option<u32> {
    u32::from_str_radix(v.trim(), 16).ok()
}

fn convert_code(addr: u32, ptype: &str, value: u32) -> Option<RawCode> {
    let base = addr & 0x0FFF_FFFF;
    let (a, v) = match ptype.trim().to_ascii_lowercase().as_str() {
        "byte" => (base, value & 0xFF),
        "short" | "word" => (0x1000_0000 | base, value & 0xFFFF),
        "extended" | "dword" => (0x2000_0000 | base, value),
        _ => return None,
    };
    Some(RawCode { address: a, value: v })
}

/// Faz o parse de um arquivo .pnach do PCSX2.
pub fn parse_pnach(text: &str) -> Vec<CheatGroup> {
    let mut groups: Vec<CheatGroup> = Vec::new();
    let mut current: Option<CheatGroup> = None;
    let push_group = |g: Option<CheatGroup>, groups: &mut Vec<CheatGroup>| {
        if let Some(g) = g {
            if !g.codes.is_empty() {
                groups.push(g);
            }
        }
    };

    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(rest) = t.strip_prefix("//") {
            // comentário livre → se nada aberto, cria grupo
            let name = rest.trim().to_string();
            if !name.is_empty() && current.is_none() {
                current = Some(CheatGroup { name, codes: Vec::new() });
            }
            continue;
        }
        if let Some(rest) = t.strip_prefix('#') {
            let name = rest.trim().to_string();
            if !name.is_empty() && current.is_none() {
                current = Some(CheatGroup { name, codes: Vec::new() });
            }
            continue;
        }
        if let Some(rest) = t.strip_prefix("comment=") {
            push_group(current.take(), &mut groups);
            current = Some(CheatGroup { name: rest.trim().to_string(), codes: Vec::new() });
            continue;
        }
        if let Some(rest) = t.strip_prefix("patch=") {
            let f: Vec<&str> = rest.split(',').map(str::trim).collect();
            // patch=1,EE,ADDR,TYPE,VALUE
            if f.len() >= 5 && f[1].eq_ignore_ascii_case("EE") {
                if let (Some(addr), Some(val)) = (
                    u32::from_str_radix(f[2], 16).ok(),
                    norm_value(f[4]),
                ) {
                    if let Some(code) = convert_code(addr, f[3], val) {
                        current
                            .get_or_insert_with(|| CheatGroup {
                                name: "Cheats".into(),
                                codes: Vec::new(),
                            })
                            .codes
                            .push(code);
                    }
                }
            }
            continue;
        }
        // linha solta não-código → nome de grupo (mesma regra do cheatman.c)
        if !t.chars().all(|c| c.is_ascii_hexdigit() || c.is_whitespace()) {
            push_group(current.take(), &mut groups);
            current = Some(CheatGroup { name: t.to_string(), codes: Vec::new() });
        }
    }
    push_group(current.take(), &mut groups);
    groups
}

/// Grava `CHT/<game_id>.cht`. O OPL aceita até 250 códigos (MAX_CODES,
/// include/cheatman.h) — o excedente é descartado com aviso.
pub fn write_cht(device_root: &Path, game_id: &str, groups: &[CheatGroup]) -> Result<PathBuf> {
    let dir = device_root.join("CHT");
    std::fs::create_dir_all(&dir).map_err(crate::error::Ps2Error::Io)?;
    let path = dir.join(format!("{game_id}.cht"));
    let mut w = std::io::BufWriter::new(
        std::fs::File::create(&path).map_err(crate::error::Ps2Error::Io)?,
    );
    let total: usize = groups.iter().map(|g| g.codes.len()).sum();
    writeln!(w, "// Gerado por PS2 Manager — formato PS2RD/OPL")
        .map_err(crate::error::Ps2Error::Io)?;
    writeln!(w, "// Jogo: {game_id} | {total} códigos")
        .map_err(crate::error::Ps2Error::Io)?;
    let mut written = 0usize;
    let mut dropped = 0usize;
    for g in groups {
        writeln!(w).map_err(crate::error::Ps2Error::Io)?;
        writeln!(w, "{}", g.name).map_err(crate::error::Ps2Error::Io)?;
        for c in &g.codes {
            if written >= 250 {
                dropped += 1;
                continue;
            }
            writeln!(w, "{:08X} {:08X}", c.address, c.value)
                .map_err(crate::error::Ps2Error::Io)?;
            written += 1;
        }
    }
    w.flush().map_err(crate::error::Ps2Error::Io)?;
    if dropped > 0 {
        eprintln!("⚠ {game_id}: {dropped} códigos descartados (limite do OPL: 250)");
    }
    Ok(path)
}

/// Converte todos os `<GAMEID>.pnach` de um diretório para CHT/.
pub fn convert_dir(device_root: &Path, pnach_dir: &Path) -> Result<(usize, usize)> {
    let mut ok = 0;
    let mut skip = 0;
    for entry in std::fs::read_dir(pnach_dir).map_err(crate::error::Ps2Error::Io)? {
        let path = entry.map_err(crate::error::Ps2Error::Io)?.path();
        if path.extension().and_then(|e| e.to_str())
            != Some("pnach")
        {
            continue;
        }
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_ascii_uppercase())
            .unwrap_or_default();
        if !crate::iso::parser::looks_like_game_id(&stem) {
            skip += 1;
            continue;
        }
        let text = std::fs::read_to_string(&path).map_err(crate::error::Ps2Error::Io)?;
        let groups = parse_pnach(&text);
        if groups.is_empty() {
            skip += 1;
            continue;
        }
        write_cht(device_root, &stem, &groups)?;
        ok += 1;
    }
    Ok((ok, skip))
}
