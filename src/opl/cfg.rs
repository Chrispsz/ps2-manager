//! CFG/<GAMEID>.cfg — arquivo de configuração por jogo do OPL
//! (`supportbase.c:782`: `"%sCFG%s%s.cfg"`), linhas `key=value\r\n`
//! (`config.c:552`). Chaves GSM conforme `include/config.h:49-61`.

use crate::error::Result;
use crate::iso::parser::GameInfo;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Default)]
pub struct GsmConfig {
    pub vmode: i32,
    pub x_offset: i32,
    pub y_offset: i32,
    pub field_fix: i32,
}

/// Grava/Atualiza o CFG do jogo. Nota: o build OPL-Mod-PTBR não compila GSM,
/// mas as chaves `$GSM*` são inofensivas e ativam o recurso em builds completos.
/// `enable_cheat` grava `$EnableCheat=1` (`include/config.h` — cheatman.c
/// lê essa chave do CFG do jogo e liga o motor PS2RD automaticamente; o
/// caller deve checar se `CHT/<GAMEID>.cht` existe).
pub fn write_game_cfg(
    device_root: &Path,
    info: &GameInfo,
    gsm: Option<GsmConfig>,
    enable_cheat: bool,
) -> Result<PathBuf> {
    let dir = device_root.join("CFG");
    std::fs::create_dir_all(&dir).map_err(crate::error::Ps2Error::Io)?;
    let path = dir.join(format!("{}.cfg", info.game_id));
    let mut w = std::io::BufWriter::new(std::fs::File::create(&path).map_err(crate::error::Ps2Error::Io)?);
    let title = crate::opl::library::sanitize_title(&info.volume_id);
    writeln!(w, "#Name={title}\r").map_err(crate::error::Ps2Error::Io)?;
    writeln!(w, "#Size={}\r", info.size / (1024 * 1024)).map_err(crate::error::Ps2Error::Io)?;
    writeln!(w, "#Format=ISO\r").map_err(crate::error::Ps2Error::Io)?;
    writeln!(w, "#Media={}\r", info.media.as_str()).map_err(crate::error::Ps2Error::Io)?;
    writeln!(w, "#Startup={}\r", info.game_id).map_err(crate::error::Ps2Error::Io)?;
    if let Some(g) = gsm {
        writeln!(w, "$EnableGSM=1\r").map_err(crate::error::Ps2Error::Io)?;
        writeln!(w, "$GSMSource=1\r").map_err(crate::error::Ps2Error::Io)?;
        writeln!(w, "$GSMVMode={}\r", g.vmode).map_err(crate::error::Ps2Error::Io)?;
        writeln!(w, "$GSMXOffset={}\r", g.x_offset).map_err(crate::error::Ps2Error::Io)?;
        writeln!(w, "$GSMYOffset={}\r", g.y_offset).map_err(crate::error::Ps2Error::Io)?;
        writeln!(w, "$GSMFIELDFix={}\r", g.field_fix).map_err(crate::error::Ps2Error::Io)?;
    }
    if enable_cheat {
        writeln!(w, "$EnableCheat=1\r").map_err(crate::error::Ps2Error::Io)?;
    }
    w.flush().map_err(crate::error::Ps2Error::Io)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iso::parser::{GameInfo, Media};

    fn info() -> GameInfo {
        GameInfo {
            game_id: "SLUS_214.44".into(),
            volume_id: "Teste".into(),
            media: Media::Dvd,
            size: 40 * 1024 * 1024,
            total_sectors: 20480,
        }
    }

    #[test]
    fn cfg_com_e_sem_cheat() {
        let tmp = std::env::temp_dir().join(format!("ps2m-cfg-t{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let i = info();

        let p = write_game_cfg(&tmp, &i, None, false).unwrap();
        let body = std::fs::read_to_string(&p).unwrap();
        assert!(body.contains("#Startup=SLUS_214.44\r"));
        assert!(!body.contains("$EnableCheat"));

        let p = write_game_cfg(&tmp, &i, Some(GsmConfig { vmode: 2, ..Default::default() }), true).unwrap();
        let body = std::fs::read_to_string(&p).unwrap();
        assert!(body.contains("$EnableCheat=1\r"));
        assert!(body.contains("$GSMVMode=2\r"));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
