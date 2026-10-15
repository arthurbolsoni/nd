//! Estado do daemon em `~/.nd/`: `daemon.json` (porta, token, pids) e `daemon.log`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Estado {
    /// Pid do processo `nd daemon`.
    pub pid: u32,
    /// Porta HTTP do daemon em 127.0.0.1.
    pub porta: u16,
    pub token: String,
    /// Pasta do perfil, ou `temp`.
    pub perfil: String,
    pub oculto: bool,
    /// Pid do processo do navegador.
    pub edge_pid: Option<u32>,
    /// Segundos desde 1970 em que o daemon ficou pronto.
    pub iniciado: u64,
}

pub fn home() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `~/.nd`, ou `ND_DIR` se definido (os testes usam um diretório próprio).
pub fn dir() -> PathBuf {
    let d = std::env::var_os("ND_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".nd"));
    // Caminho com `/` e `\` misturados (ND_DIR vindo do Git Bash) faz o Edge com janela falhar
    // o download; `absolute` normaliza.
    let d = std::path::absolute(&d).unwrap_or(d);
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn arquivo() -> PathBuf {
    dir().join("daemon.json")
}

pub fn log() -> PathBuf {
    dir().join("daemon.log")
}

pub fn ler() -> Option<Estado> {
    let texto = std::fs::read_to_string(arquivo()).ok()?;
    serde_json::from_str(&texto).ok()
}

pub fn gravar(e: &Estado) -> std::io::Result<()> {
    let tmp = dir().join("daemon.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(e)?)?;
    std::fs::rename(tmp, arquivo())
}

/// Apaga o estado só se ele for do daemon `pid` (outro daemon pode ter subido depois).
pub fn apagar_se_for(pid: u32) {
    if ler().is_some_and(|e| e.pid == pid) {
        let _ = std::fs::remove_file(arquivo());
    }
}

pub fn agora() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Perfil padrão do nodriver: `~/nodriver/profile-edge`.
pub fn perfil_padrao() -> String {
    home().join("nodriver").join("profile-edge").display().to_string()
}
