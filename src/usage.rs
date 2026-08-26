//! Uso real dos aplicativos: quanto tempo cada exe fica aberto e em foco.
//!
//! Duas fontes, somadas por caminho de executável:
//!
//! 1. **UserAssist** (registro do Explorer) — histórico que o Windows já mantém desde
//!    sempre: contagem de execuções e, o que importa aqui, **tempo em foco** acumulado.
//!    É o que dá resposta útil no primeiro Scan, sem esperar semanas de coleta.
//! 2. **Contagem do próprio RamDog** — enquanto o app está aberto, soma os segundos em
//!    que cada exe esteve rodando. Vai além do foco (serviço/agente que fica no fundo)
//!    e vale para máquinas com UserAssist limpo por otimizador.
//!
//! O ranking soma as duas com peso: foco pesa 1, tempo aberto pesa 0,5. Tempo em foco é
//! a evidência mais forte de "eu uso isso"; tempo aberto sozinho premiaria qualquer
//! coisa que só fica no ar.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::procs::ProcInfo;

/// Peso do tempo aberto em relação ao tempo em foco no score do ranking.
const OPEN_WEIGHT: f64 = 0.5;
/// Intervalo mínimo entre duas contagens. Abaixo disso o tick sai fora — não vale
/// mexer no mapa a cada amostra de 1 s.
const TICK_SECS: f64 = 5.0;
/// Acima disso o intervalo não vira uso: é pausa, suspensão ou máquina travada.
const TICK_MAX_SECS: f64 = 90.0;
/// Segundos entre gravações do usage.json.
const SAVE_SECS: u64 = 60;
/// Teto de apps guardados no arquivo — poda pelos menos usados.
const MAX_APPS: usize = 1500;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppUsage {
    pub name: String,
    /// Segundos com o processo aberto, somados enquanto o RamDog esteve rodando.
    pub open_secs: u64,
    /// Quantas vezes o RamDog viu o app aparecer do nada.
    pub launches: u32,
    /// Unix secs da última vez visto aberto.
    pub last_seen: i64,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Store {
    apps: HashMap<String, AppUsage>,
}

/// Contador vivo de tempo aberto por executável. Vive no `App` porque é ele que recebe
/// cada amostra de processos.
pub struct Tracker {
    apps: HashMap<String, AppUsage>,
    present: HashSet<String>,
    last_tick: Option<Instant>,
    dirty: bool,
    last_save: Instant,
}

impl Tracker {
    pub fn load() -> Self {
        let apps = std::fs::read_to_string(store_path())
            .ok()
            .and_then(|s| serde_json::from_str::<Store>(&s).ok())
            .map(|s| s.apps)
            .unwrap_or_default();
        Self {
            apps,
            present: HashSet::new(),
            last_tick: None,
            dirty: false,
            last_save: Instant::now(),
        }
    }

    pub fn apps(&self) -> &HashMap<String, AppUsage> {
        &self.apps
    }

    /// Soma o intervalo desde o tick anterior em todo exe visto agora.
    pub fn tick(&mut self, procs: &[ProcInfo]) {
        let now = Instant::now();
        let Some(last) = self.last_tick else {
            self.last_tick = Some(now);
            self.present = live_paths(procs).into_keys().collect();
            return;
        };
        let dt = now.duration_since(last).as_secs_f64();
        if dt < TICK_SECS {
            return;
        }
        self.last_tick = Some(now);
        let add = if dt > TICK_MAX_SECS { 0 } else { dt.round() as u64 };
        let live = live_paths(procs);
        let stamp = unix_now();
        for (path, proper) in &live {
            let fresh = !self.present.contains(path);
            let e = self.apps.entry(path.clone()).or_insert_with(|| AppUsage {
                name: proper.clone(),
                ..Default::default()
            });
            if e.name.is_empty() {
                e.name = proper.clone();
            }
            e.open_secs += add;
            e.last_seen = stamp;
            if fresh {
                e.launches += 1;
            }
        }
        self.present = live.into_keys().collect();
        self.dirty = true;
    }

    pub fn save_if_due(&mut self) {
        if !self.dirty || self.last_save.elapsed().as_secs() < SAVE_SECS {
            return;
        }
        self.last_save = Instant::now();
        self.dirty = false;
        self.prune();
        let path = store_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let store = Store { apps: self.apps.clone() };
        if let Ok(s) = serde_json::to_string(&store) {
            let _ = std::fs::write(&path, s);
        }
    }

    fn prune(&mut self) {
        if self.apps.len() <= MAX_APPS {
            return;
        }
        let mut v: Vec<(String, u64)> = self.apps.iter().map(|(k, a)| (k.clone(), a.open_secs)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        for (k, _) in v.drain(MAX_APPS..) {
            self.apps.remove(&k);
        }
    }
}

/// Caminho em minúsculo → nome do arquivo com a caixa certa, dos apps abertos agora.
///
/// "Aberto" aqui é **com janela**, não "tem processo vivo". Sem esse corte, todo daemon —
/// python, node, uv, bash — somaria 24 h por dia e afogaria no ranking justamente os
/// programas que o usuário abre e usa. Nada de dentro do Windows entra: componente do
/// sistema não é "app que eu uso".
fn live_paths(procs: &[ProcInfo]) -> HashMap<String, String> {
    let windowed = windowed_pids();
    procs
        .iter()
        .filter(|p| !p.exe_path.is_empty())
        .filter(|p| windowed.as_ref().map(|w| w.contains(&p.pid)).unwrap_or(true))
        .map(|p| (p.exe_path.to_lowercase(), p.name.clone()))
        .filter(|(p, _)| !in_windows_dir(p))
        .collect()
}

/// PIDs donos de alguma janela de topo visível e com título. `None` fora do Windows —
/// aí a contagem volta a valer para todo processo.
#[cfg(not(windows))]
fn windowed_pids() -> Option<HashSet<u32>> {
    None
}

#[cfg(windows)]
fn windowed_pids() -> Option<HashSet<u32>> {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, TRUE};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextLengthW, GetWindowThreadProcessId, IsWindowVisible,
    };

    unsafe extern "system" fn cb(hwnd: HWND, lp: LPARAM) -> BOOL {
        unsafe {
            if IsWindowVisible(hwnd).as_bool() && GetWindowTextLengthW(hwnd) > 0 {
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                if pid != 0 {
                    let set = &mut *(lp.0 as *mut HashSet<u32>);
                    set.insert(pid);
                }
            }
        }
        TRUE
    }

    let mut set: HashSet<u32> = HashSet::new();
    unsafe {
        if EnumWindows(Some(cb), LPARAM(&mut set as *mut _ as isize)).is_err() {
            return None;
        }
    }
    Some(set)
}

fn store_path() -> PathBuf {
    crate::config::config_path().with_file_name("usage.json")
}

fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn file_name(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_string()
}

fn in_windows_dir(path_lower: &str) -> bool {
    path_lower.contains(r"\windows\") || path_lower.starts_with(r"\??\")
}

// ---------- UserAssist ----------

/// Uma linha do UserAssist já decodificada.
pub struct UaEntry {
    /// Caminho absoluto quando o nome trazia um; vazio para identificador de app.
    pub path: String,
    /// Último pedaço do identificador, em minúsculo. É a ponte que liga `Brave` ou
    /// `com.squirrel.Discord.Discord` ao `brave.exe`/`discord.exe` de verdade — sem ela os
    /// dois apps mais usados da máquina ficariam de fora do ranking.
    pub token: String,
    pub focus_secs: u64,
    pub run_count: u32,
    /// Unix secs da última execução (0 quando o registro não trouxe).
    pub last_run: i64,
}

#[cfg(not(windows))]
pub fn user_assist() -> Vec<UaEntry> {
    Vec::new()
}

#[cfg(windows)]
pub fn user_assist() -> Vec<UaEntry> {
    use windows::Win32::System::Registry::HKEY_CURRENT_USER;

    const BASE: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\UserAssist";
    let folders = known_folders();
    let mut out: Vec<UaEntry> = Vec::new();
    let Some(root) = crate::sys::reg_open(HKEY_CURRENT_USER, BASE, false) else {
        return out;
    };
    for guid in crate::sys::reg_subkeys(&root) {
        let count_path = format!("{BASE}\\{guid}\\Count");
        let Some(k) = crate::sys::reg_open(HKEY_CURRENT_USER, &count_path, false) else { continue };
        for (name, _ty, data) in crate::sys::reg_values(&k) {
            // Win7+ grava 72 bytes; o formato antigo (16) não tem tempo de foco.
            if data.len() < 72 {
                continue;
            }
            let decoded = rot13(&name);
            if decoded.starts_with("UEME_") {
                continue;
            }
            let path = resolve_ua_path(&decoded, &folders);
            let token = match &path {
                Some(p) => stem(p),
                None => id_token(&decoded),
            };
            if path.is_none() && token.is_empty() {
                continue;
            }
            let run_count = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
            let focus_ms = u32::from_le_bytes([data[12], data[13], data[14], data[15]]);
            let ft = i64::from_le_bytes([
                data[60], data[61], data[62], data[63], data[64], data[65], data[66], data[67],
            ]);
            out.push(UaEntry {
                path: path.unwrap_or_default(),
                token,
                focus_secs: (focus_ms / 1000) as u64,
                run_count,
                last_run: filetime_to_unix(ft),
            });
        }
    }
    out
}

/// Nome do arquivo sem extensão, em minúsculo.
fn stem(path: &str) -> String {
    let n = file_name(path).to_lowercase();
    n.strip_suffix(".exe").unwrap_or(&n).to_string()
}

/// Reduz um identificador de app (`Brave`, `Microsoft.WindowsTerminal_8wekyb3d8bbwe!App`,
/// `com.squirrel.Discord.Discord`) ao pedaço que costuma ser o nome do executável.
/// Devolve vazio quando não sobra nada confiável — casar por token curto daria falso
/// positivo, e um ranking com app errado é pior que um ranking mais curto.
fn id_token(name: &str) -> String {
    let base = name.split('!').next().unwrap_or(name);
    let base = base.split('.').next_back().unwrap_or(base);
    let base = base.split('_').next().unwrap_or(base);
    let t = base.trim().to_lowercase();
    let ok = t.len() >= 4 && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '+') && t.chars().any(|c| c.is_ascii_alphabetic());
    if ok { t } else { String::new() }
}

fn rot13(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' => (((c as u8 - b'a' + 13) % 26) + b'a') as char,
            'A'..='Z' => (((c as u8 - b'A' + 13) % 26) + b'A') as char,
            _ => c,
        })
        .collect()
}

fn filetime_to_unix(ft: i64) -> i64 {
    if ft <= 0 {
        return 0;
    }
    ft / 10_000_000 - 11_644_473_600
}

/// Nomes do UserAssist vêm como `{GUID-da-pasta-conhecida}\resto\do\caminho`, ou já como
/// caminho absoluto. GUID que não sabemos resolver vira `None` — sem caminho não dá para
/// mostrar ícone nem colocar na partida.
fn resolve_ua_path(name: &str, folders: &HashMap<String, String>) -> Option<String> {
    let full = if let Some(rest) = name.strip_prefix('{') {
        let (guid, tail) = rest.split_once('}')?;
        let base = folders.get(&guid.to_ascii_uppercase())?;
        format!("{}{}", base, tail)
    } else if name.len() > 2 && name.as_bytes()[1] == b':' {
        name.to_string()
    } else {
        return None;
    };
    let full = full.replace("\\\\", "\\");
    if !full.to_ascii_lowercase().ends_with(".exe") {
        return None;
    }
    Some(full)
}

/// GUIDs de KNOWNFOLDERID que aparecem no UserAssist, resolvidos por variável de ambiente.
/// Resolver via `SHGetKnownFolderPath` seria mais correto no papel, mas exigiria montar
/// cada GUID como struct — para este punhado de pastas a variável dá o mesmo caminho.
fn known_folders() -> HashMap<String, String> {
    let mut m = HashMap::new();
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let mut put = |guid: &str, val: Option<String>| {
        if let Some(v) = val {
            m.insert(guid.to_ascii_uppercase(), v.trim_end_matches('\\').to_string());
        }
    };
    let win = env("WINDIR").or_else(|| env("SystemRoot")).unwrap_or_else(|| r"C:\Windows".into());
    let profile = env("USERPROFILE");
    let appdata = env("APPDATA");
    let local = env("LOCALAPPDATA");
    let pf64 = env("ProgramW6432").or_else(|| env("ProgramFiles"));
    let pf86 = env("ProgramFiles(x86)").or_else(|| env("ProgramFiles"));
    let progdata = env("ProgramData");

    put("6D809377-6AF0-444B-8957-A3773F02200E", pf64.clone());
    put("7C5A40EF-A0FB-4BFC-874A-C0F2E0B9FA8E", pf86.clone());
    put("905E63B6-C1BF-494E-B29C-65B732D3D21A", pf64.clone());
    put("F7F1ED05-9F6D-47A2-AAAE-29D317C6F066", pf64.as_ref().map(|p| format!(r"{p}\Common Files")));
    put("DE974D24-D9C6-4D3E-BF91-F4455120B917", pf86.as_ref().map(|p| format!(r"{p}\Common Files")));
    put("F38BF404-1D43-42F2-9305-67DE0B28FC23", Some(win.clone()));
    put("1AC14E77-02E7-4E5D-B744-2EB1AE5198B7", Some(format!(r"{win}\System32")));
    put("D65231B0-B2F1-4857-A4CE-A8E7C6EA7D27", Some(format!(r"{win}\SysWOW64")));
    put("5E6C858F-0E22-4760-9AFE-EA3317B67173", profile.clone());
    put("3EB685DB-65F9-4CF6-A03A-E3EF65729F3D", appdata.clone());
    put("F1B32785-6FBA-4FCF-9D55-7B8E7F157091", local.clone());
    put("A520A1A4-1780-4FF6-BD18-167343C5AF16", local.as_ref().map(|p| format!(r"{p}Low")));
    put("62AB5D82-FDC1-4DC3-A9DD-070D1D495D97", progdata.clone());
    put("B4BFCC3A-DB2C-424C-B029-7FE99A87C641", profile.as_ref().map(|p| format!(r"{p}\Desktop")));
    put("FDD39AD0-238F-46AF-ADB4-6C85480369C7", profile.as_ref().map(|p| format!(r"{p}\Documents")));
    put("374DE290-123F-4565-9164-39C4925E467B", profile.as_ref().map(|p| format!(r"{p}\Downloads")));
    put(
        "A77F5D77-2E2B-44C3-A6A2-ABA601054A51",
        appdata.as_ref().map(|p| format!(r"{p}\Microsoft\Windows\Start Menu\Programs")),
    );
    put(
        "0139D44E-6AFE-49F2-8690-3DAFCAE6FFB8",
        progdata.as_ref().map(|p| format!(r"{p}\Microsoft\Windows\Start Menu\Programs")),
    );
    put(
        "9E3995AB-1F9C-4F13-B827-48B24B6C7174",
        appdata.as_ref().map(|p| format!(r"{p}\Microsoft\Internet Explorer\Quick Launch\User Pinned")),
    );
    m
}

// ---------- ranking ----------

/// Uma linha do Scan: um executável, com o que se sabe do uso dele.
#[derive(Clone)]
pub struct Ranked {
    /// Caminho real, com a caixa como está no disco.
    pub path: String,
    pub name: String,
    pub focus_secs: u64,
    pub open_secs: u64,
    pub launches: u32,
    pub last_used: i64,
    pub score: f64,
}

/// Nada disto é "app que eu uso": é encanamento que roda sozinho.
const DENY: [&str; 14] = [
    "unins",
    "setup.exe",
    "install",
    "vcredist",
    "crashpad",
    "dllhost.exe",
    "rundll32.exe",
    "regsvr32.exe",
    "msiexec.exe",
    "wusa.exe",
    "ramdog.exe",
    "hwtemp.exe",
    "werfault.exe",
    "conhost.exe",
];

/// Junta UserAssist + contagem local, tira ruído e ordena por score.
/// `limit` 0 devolve tudo — é assim que a coluna "Uso" da lista é alimentada.
pub fn rank(live: &HashMap<String, AppUsage>, ua: &[UaEntry], limit: usize) -> Vec<Ranked> {
    let mut by_path: HashMap<String, Ranked> = HashMap::new();

    // Nome do exe → caminho, montado do que o RamDog já viu rodar. É contra este mapa que
    // um identificador de app vira caminho de verdade.
    let mut by_stem: HashMap<String, String> = HashMap::new();
    for key in live.keys() {
        by_stem.entry(stem(key)).or_insert_with(|| key.clone());
    }
    for e in ua {
        if e.path.is_empty() {
            continue;
        }
        let key = e.path.to_lowercase();
        by_stem.entry(stem(&key)).or_insert(key);
    }

    for e in ua {
        let key = if e.path.is_empty() {
            match by_stem.get(&e.token) {
                Some(p) => p.clone(),
                None => continue,
            }
        } else {
            e.path.to_lowercase()
        };
        if !keep(&key) {
            continue;
        }
        // O caminho do registro vem com a caixa de verdade; a chave é só minúscula.
        let shown = if e.path.is_empty() { key.clone() } else { e.path.clone() };
        let r = by_path.entry(key).or_insert_with(|| Ranked {
            name: file_name(&shown),
            path: shown,
            focus_secs: 0,
            open_secs: 0,
            launches: 0,
            last_used: 0,
            score: 0.0,
        });
        // O mesmo exe aparece em mais de um GUID (execução direta e via atalho): soma.
        r.focus_secs += e.focus_secs;
        r.launches += e.run_count;
        r.last_used = r.last_used.max(e.last_run);
    }

    for (key, a) in live {
        if !keep(key) {
            continue;
        }
        let r = by_path.entry(key.clone()).or_insert_with(|| Ranked {
            path: key.clone(),
            name: if a.name.is_empty() { file_name(key) } else { a.name.clone() },
            focus_secs: 0,
            open_secs: 0,
            launches: 0,
            last_used: 0,
            score: 0.0,
        });
        if !a.name.is_empty() {
            r.name = a.name.clone();
        }
        r.open_secs += a.open_secs;
        r.launches += a.launches;
        r.last_used = r.last_used.max(a.last_seen);
    }

    let mut out: Vec<Ranked> = by_path
        .into_values()
        .map(|mut r| {
            r.score = r.focus_secs as f64 + r.open_secs as f64 * OPEN_WEIGHT;
            r
        })
        .filter(|r| r.score > 0.0)
        .collect();
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    if limit > 0 {
        out.truncate(limit);
    }
    out
}

fn keep(path_lower: &str) -> bool {
    if !path_lower.ends_with(".exe") || in_windows_dir(path_lower) {
        return false;
    }
    let name = file_name(path_lower);
    if DENY.iter().any(|d| name.contains(d)) {
        return false;
    }
    Path::new(path_lower).is_file()
}

/// Formata segundos como "3 h 20 min" / "45 min" / "20 s".
pub fn fmt_secs(s: u64) -> String {
    if s == 0 {
        return "—".into();
    }
    if s < 60 {
        return format!("{s} s");
    }
    let m = s / 60;
    if m < 60 {
        return format!("{m} min");
    }
    let h = m / 60;
    let rem = m % 60;
    if h < 48 {
        if rem == 0 {
            format!("{h} h")
        } else {
            format!("{h} h {rem} min")
        }
    } else {
        format!("{} d {} h", h / 24, h % 24)
    }
}

/// "hoje", "ontem", "há 12 dias" — a partir de um unix secs.
pub fn fmt_ago(unix: i64) -> String {
    if unix <= 0 {
        return "—".into();
    }
    let d = (unix_now() - unix).max(0) / 86_400;
    match d {
        0 => "hoje".into(),
        1 => "ontem".into(),
        2..=60 => format!("há {d} dias"),
        _ => format!("há {} meses", d / 30),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rot13_ida_e_volta() {
        assert_eq!(rot13("Oenir"), "Brave");
        assert_eq!(rot13(r"P:\Cebwrgbf\XnzhvG.rkr"), r"C:\Projetos\KamuiT.exe");
        assert_eq!(rot13("HRZR_PGYFRFFVBA"), "UEME_CTLSESSION");
    }

    #[test]
    fn token_reduz_identificador_ao_nome_do_exe() {
        assert_eq!(id_token("Brave"), "brave");
        assert_eq!(id_token("com.squirrel.Discord.Discord"), "discord");
        assert_eq!(id_token("Microsoft.WindowsTerminal_8wekyb3d8bbwe!App"), "windowsterminal");
        assert_eq!(id_token("Exafunction.Windsurf"), "windsurf");
        // Curto ou sem letra não vale casar: daria falso positivo.
        assert_eq!(id_token("com.sharingan.app"), "");
        assert_eq!(id_token("{46057DD9-242B-4089-6560-E0F4B0DF80D3}"), "");
    }

    #[test]
    fn caminho_do_userassist_sai_do_guid_de_pasta_conhecida() {
        let f = known_folders();
        let pf = f.get("6D809377-6AF0-444B-8957-A3773F02200E").cloned();
        if let Some(pf) = pf {
            let got = resolve_ua_path(r"{6D809377-6AF0-444B-8957-A3773F02200E}\Notepad++\notepad++.exe", &f);
            assert_eq!(got, Some(format!(r"{pf}\Notepad++\notepad++.exe")));
        }
        // GUID desconhecido e coisa que não é .exe ficam de fora.
        assert_eq!(resolve_ua_path(r"{00000000-0000-0000-0000-000000000000}\x.exe", &f), None);
        assert_eq!(resolve_ua_path(r"C:\algo\arquivo.txt", &f), None);
    }

    #[test]
    fn identificador_de_app_casa_com_exe_que_ja_rodou() {
        // O UserAssist só sabe "Brave"; quem tem o caminho é a contagem local.
        let mut live = HashMap::new();
        live.insert(
            r"c:\program files\bravesoftware\brave-browser\application\brave.exe".to_string(),
            AppUsage { name: "brave.exe".into(), open_secs: 100, launches: 1, last_seen: 1 },
        );
        let ua = vec![UaEntry {
            path: String::new(),
            token: "brave".into(),
            focus_secs: 3600,
            run_count: 7,
            last_run: 2,
        }];
        // `keep` exige arquivo no disco; sem ele o teste só confere a ponte do token.
        let mut by_stem: HashMap<String, String> = HashMap::new();
        for k in live.keys() {
            by_stem.insert(stem(k), k.clone());
        }
        assert_eq!(by_stem.get(&ua[0].token).map(String::as_str), Some(r"c:\program files\bravesoftware\brave-browser\application\brave.exe"));
    }

    #[test]
    fn tempo_em_portugues() {
        assert_eq!(fmt_secs(0), "—");
        assert_eq!(fmt_secs(45), "45 s");
        assert_eq!(fmt_secs(600), "10 min");
        assert_eq!(fmt_secs(3600), "1 h");
        assert_eq!(fmt_secs(3600 * 2 + 1200), "2 h 20 min");
    }

    /// Roda contra o registro real desta máquina. Não é asserção — é a prova de que o
    /// Scan enxerga alguma coisa. `cargo test -- --ignored --nocapture dump`
    #[test]
    #[ignore]
    fn dump_do_ranking_real() {
        let ua = user_assist();
        println!("UserAssist: {} entradas ({} com caminho)", ua.len(), ua.iter().filter(|e| !e.path.is_empty()).count());
        let t = Tracker::load();
        println!("contagem local: {} apps", t.apps().len());
        let r = rank(t.apps(), &ua, 25);
        println!("{:<34} {:>10} {:>10} {:>7}  {}", "app", "foco", "aberto", "vezes", "caminho");
        for x in &r {
            println!(
                "{:<34} {:>10} {:>10} {:>7}  {}",
                x.name,
                fmt_secs(x.focus_secs),
                fmt_secs(x.open_secs),
                x.launches,
                x.path
            );
        }
        assert!(!r.is_empty(), "ranking vazio: o Scan não teria o que mostrar");
    }
}
