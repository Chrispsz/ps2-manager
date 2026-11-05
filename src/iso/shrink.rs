//! Smart ISO Shrink — remove setores vazios à direita dos arquivos
//! (dummy/ghost sectors) e reloca os extents sequencialmente, reescrevendo
//! PVD, path tables e directory records (campos LE + BE) com os novos LBAs.
//!
//! Estratégia conservadora (à prova de crashes no PS2):
//!  1. Setores 0–17 (system area + PVD) mantêm posição fixa — PVD é patchado.
//!  2. Extents (path tables, diretórios, arquivos) são ordenados por LBA
//!     original e realocados sequencialmente a partir do setor 18.
//!  3. Arquivos têm apenas setores COMPLETOS vazios (0x00/0xFF) removidos do
//!     fim — o conteúdo visível ao jogo permanece byte-a-byte idêntico.
//!  4. Todo record apontando para um LBA movido é patchado (LE e BE).

use crate::error::{Ps2Error, Result};
use crate::iso::parser::{be32, le32, parse_image, FileSource, PVD_LBA, SectorSource, SECTOR};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

#[derive(Debug, Default, Clone)]
pub struct ShrinkReport {
    pub original_bytes: u64,
    pub new_bytes: u64,
    pub files_trimmed: u32,
    pub extents_moved: u32,
}

impl ShrinkReport {
    pub fn saved_bytes(&self) -> u64 {
        self.original_bytes.saturating_sub(self.new_bytes)
    }
}

#[derive(Debug, Clone, Copy)]
struct NewExtent {
    new_lba: u32,
    new_size: u32,
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    Dir,
    PathTableL,
    PathTableM,
    File,
}

#[derive(Debug, Clone, Copy)]
struct Extent {
    old_lba: u32,
    old_size: u32,
    kind: Kind,
    trim: bool,
}

fn collect_tree(node: &crate::iso::parser::TreeNode, out: &mut Vec<Extent>) {
    // empurra o próprio nó (root incluso) e recursiona nos filhos
    if node.is_dir && node.size > 0 {
        out.push(Extent {
            old_lba: node.lba,
            old_size: node.size,
            kind: Kind::Dir,
            trim: false,
        });
    }
    for c in &node.children {
        collect_tree(c, out);
    }
    if !node.is_dir && node.size > 0 {
        // somente o extent FINAL (flag 0x80 clear) pode sofrer trim
        out.push(Extent {
            old_lba: node.lba,
            old_size: node.size,
            kind: Kind::File,
            trim: !node.multi_extent,
        });
    }
}

/// Remove setores completos vazios (0x00 ou 0xFF) do fim do arquivo.
fn trim_file(src: &mut dyn SectorSource, lba: u32, size: u32) -> Result<u32> {
    let mut new_size = size;
    let mut buf = vec![0u8; SECTOR as usize];
    while new_size >= 2 * SECTOR as u32 {
        let k = (new_size / SECTOR as u32) - 1; // último setor completo
        src.read_at(lba as u64 + k as u64, &mut buf)
            .map_err(|e| Ps2Error::InvalidIso(format!("trim: falha lendo setor {}: {e}", lba + k)))?;
        let all_zero = buf.iter().all(|&b| b == 0);
        let all_ff = buf.iter().all(|&b| b == 0xFF);
        if all_zero || all_ff {
            new_size -= SECTOR as u32;
        } else {
            break;
        }
    }
    Ok(new_size)
}

fn patch_record_lba(buf: &mut [u8], off: usize, new_lba: u32) {
    buf[off + 2..off + 6].copy_from_slice(&new_lba.to_le_bytes());
    buf[off + 6..off + 10].copy_from_slice(&new_lba.to_be_bytes());
}

fn patch_dir(buf: &mut [u8], map: &BTreeMap<u32, NewExtent>) {
    let sec = SECTOR as usize;
    let mut pos: usize = 0;
    while pos < buf.len() {
        let rl = *buf.get(pos).unwrap_or(&0) as usize;
        if rl == 0 {
            pos = ((pos / sec) + 1) * sec;
            continue;
        }
        if rl >= 34 && pos + rl <= buf.len() {
            let old = le32(buf, pos + 2);
            if let Some(ne) = map.get(&old) {
                patch_record_lba(buf, pos, ne.new_lba);
                let old_size = le32(buf, pos + 10);
                if ne.new_size != old_size {
                    buf[pos + 10..pos + 14].copy_from_slice(&ne.new_size.to_le_bytes());
                    buf[pos + 14..pos + 18].copy_from_slice(&ne.new_size.to_be_bytes());
                }
            }
        }
        pos += rl;
    }
}

fn patch_path_table(buf: &mut [u8], map: &BTreeMap<u32, NewExtent>, big_endian: bool) {
    let mut pos: usize = 0;
    while pos + 8 <= buf.len() {
        let len_di = buf[pos] as usize;
        if len_di == 0 {
            break; // padding até o fim da tabela
        }
        let old = if big_endian { be32(buf, pos + 2) } else { le32(buf, pos + 2) };
        if let Some(ne) = map.get(&old) {
            if big_endian {
                buf[pos + 2..pos + 6].copy_from_slice(&ne.new_lba.to_be_bytes());
            } else {
                buf[pos + 2..pos + 6].copy_from_slice(&ne.new_lba.to_le_bytes());
            }
        }
        pos += 8 + len_di + (len_di & 1);
    }
}

pub fn shrink_iso(input: &Path, output: &Path) -> Result<ShrinkReport> {
    let mut src = FileSource::open(input).map_err(Ps2Error::Io)?;
    let original = File::open(input)
        .map_err(Ps2Error::Io)?
        .metadata()
        .map_err(Ps2Error::Io)?
        .len();
    let info = parse_image(&mut src)?;

    const VD_SECTORS: u32 = 18; // setores 0..=17 fixos (system area + PVD)

    let mut extents: Vec<Extent> = Vec::new();
    if info.l_path_table.1 > 0 {
        extents.push(Extent {
            old_lba: info.l_path_table.0,
            old_size: info.l_path_table.1,
            kind: Kind::PathTableL,
            trim: false,
        });
    }
    if info.m_path_table.1 > 0 {
        extents.push(Extent {
            old_lba: info.m_path_table.0,
            old_size: info.m_path_table.1,
            kind: Kind::PathTableM,
            trim: false,
        });
    }
    collect_tree(&info.root, &mut extents);
    extents.sort_by_key(|e| e.old_lba);
    extents.dedup_by_key(|e| e.old_lba);
    extents.retain(|e| e.old_lba > 0 && e.old_size > 0);

    // fase 1: trims + tamanho novo
    let mut resolved: Vec<(Extent, u32)> = Vec::with_capacity(extents.len());
    let mut files_trimmed = 0u32;
    for e in &extents {
        let ns = if e.trim {
            let t = trim_file(&mut src, e.old_lba, e.old_size)?;
            if t != e.old_size {
                files_trimmed += 1;
            }
            t
        } else {
            e.old_size
        };
        resolved.push((*e, ns));
    }

    // fase 2: alocação sequencial dos novos LBAs
    let mut map: BTreeMap<u32, NewExtent> = BTreeMap::new();
    let mut next = VD_SECTORS;
    for (e, ns) in &resolved {
        let secs = (*ns as u64).div_ceil(SECTOR) as u32;
        map.insert(
            e.old_lba,
            NewExtent {
                new_lba: next,
                new_size: *ns,
            },
        );
        next += secs;
    }
    let total_sectors = next;
    let root_new = map
        .get(&info.root.lba)
        .copied()
        .ok_or_else(|| Ps2Error::InvalidIso("root dir não mapeado".into()))?;

    // fase 3: escrita
    let mut out = BufWriter::new(File::create(output).map_err(Ps2Error::Io)?);

    // 3a) região de VDs com PVD patchado
    // atenção: o PVD fica no setor 16 → offset DENTRO do buffer = 32768+campo
    const PVD_OFF: usize = (PVD_LBA * SECTOR) as usize;
    let mut vd = vec![0u8; VD_SECTORS as usize * SECTOR as usize];
    src.read_at(0, &mut vd)
        .map_err(|e| Ps2Error::InvalidIso(format!("falha lendo região de VDs: {e}")))?;
    patch_record_lba(&mut vd, PVD_OFF + 124, root_new.new_lba);
    if let Some(x) = map.get(&info.l_path_table.0) {
        vd[PVD_OFF + 108..PVD_OFF + 112].copy_from_slice(&x.new_lba.to_le_bytes());
    }
    if let Some(x) = map.get(&info.m_path_table.0) {
        vd[PVD_OFF + 116..PVD_OFF + 120].copy_from_slice(&x.new_lba.to_be_bytes());
    }
    out.write_all(&vd).map_err(Ps2Error::Io)?;

    // 3b) extents em ordem sequencial — cada um ocupa `secs` setores
    // completos (padding zero no fim) para manter o alinhamento dos LBAs
    for (e, ns) in &resolved {
        let secs = (*ns as u64).div_ceil(SECTOR) as usize;
        let buf_len = secs * SECTOR as usize;
        let mut buf = vec![0u8; buf_len];
        let read_len = (*ns as usize).min(buf_len);
        src.read_at(e.old_lba as u64, &mut buf[..read_len])
            .map_err(|err| {
                Ps2Error::InvalidIso(format!(
                    "falha lendo extent LBA {}: {err}",
                    e.old_lba
                ))
            })?;
        match e.kind {
            Kind::Dir => patch_dir(&mut buf, &map),
            Kind::PathTableL => patch_path_table(&mut buf, &map, false),
            Kind::PathTableM => patch_path_table(&mut buf, &map, true),
            Kind::File => {}
        }
        out.write_all(&buf).map_err(Ps2Error::Io)?;
    }
    out.flush().map_err(Ps2Error::Io)?;
    let f = out.into_inner().map_err(|e| Ps2Error::Other(e.to_string()))?;
    f.set_len(total_sectors as u64 * SECTOR).map_err(Ps2Error::Io)?;
    f.sync_all().ok();

    Ok(ShrinkReport {
        original_bytes: original,
        new_bytes: total_sectors as u64 * SECTOR,
        files_trimmed,
        extents_moved: map.len() as u32,
    })
}
