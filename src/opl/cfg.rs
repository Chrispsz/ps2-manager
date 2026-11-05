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
pub fn write_game_cfg(
    device_root: &Path,
    info: &GameInfo,
    gsm: Option<GsmConfig>,
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
    w.flush().map_err(crate::error::Ps2Error::Io)?;
    Ok(path)
}
