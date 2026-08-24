//! "Ralos do Windows": serviços dispensáveis, Defender, apps de inicialização e apps de sistema.
//! Leituras são diretas (SCM / registro); ações que exigem admin rodam via PowerShell elevado
//! (UAC) numa thread, devolvendo o resultado por canal.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::mpsc::Sender;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegEnumKeyExW, RegEnumValueW, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, REG_BINARY, REG_DWORD,
    REG_OPTION_NON_VOLATILE, REG_VALUE_TYPE,
};
use windows::Win32::System::Services::{
    ChangeServiceConfigW, CloseServiceHandle, ControlService, EnumServicesStatusExW, OpenSCManagerW, OpenServiceW,
    QueryServiceConfigW, ENUM_SERVICE_STATUS_PROCESSW, SC_ENUM_PROCESS_INFO, SC_MANAGER_ENUMERATE_SERVICE,
    SERVICE_STATE_ALL, SERVICE_WIN32,
    QueryServiceStatus, StartServiceW, QUERY_SERVICE_CONFIGW, SC_MANAGER_CONNECT, SERVICE_AUTO_START,
    SERVICE_CHANGE_CONFIG, SERVICE_CONTROL_STOP, SERVICE_DEMAND_START, SERVICE_DISABLED, SERVICE_ERROR,
    SERVICE_NO_CHANGE, SERVICE_QUERY_CONFIG, SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_START, SERVICE_STATUS,
    SERVICE_STOP, SERVICE_STOPPED, SERVICE_START_TYPE, SERVICE_STATUS_CURRENT_STATE,
};
use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

// ---------- Serviços ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SvcState {
    Running,
    Stopped,
    Pending,
    Missing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SvcStart {
    Auto,
    Manual,
    Disabled,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct SvcStatus {
    pub state: SvcState,
    pub start: SvcStart,
}

/// Catálogo curado: (nome do serviço, rótulo, o que faz / custo de desligar, processo típico).
pub struct SvcEntry {
    pub name: &'static str,
    pub label: &'static str,
    pub why: &'static str,
    pub proc_hint: &'static str,
    /// Só faz sentido "parar agora"; desativar quebraria coisas / o Windows re-arma sozinho.
    pub stop_only: bool,
}

pub const SERVICES: &[SvcEntry] = &[
    SvcEntry { name: "WSearch", label: "Windows Search (indexador)", why: "Indexa arquivos o tempo todo (SearchIndexer.exe). Sem ele, a busca do Explorer/Iniciar fica mais lenta — o resto não muda.", proc_hint: "searchindexer.exe", stop_only: false },
    SvcEntry { name: "SysMain", label: "SysMain (Superfetch)", why: "Pré-carrega apps na RAM \"por precaução\". Em SSD é dispensável e compete pela memória.", proc_hint: "svchost.exe", stop_only: false },
    SvcEntry { name: "DiagTrack", label: "Telemetria (Experiências do Usuário Conectado)", why: "Coleta e envia dados de uso para a Microsoft. Nenhuma função local depende dele.", proc_hint: "svchost.exe", stop_only: false },
    SvcEntry { name: "dmwappushservice", label: "Telemetria WAP Push", why: "Roteamento de mensagens WAP para telemetria. Dispensável.", proc_hint: "svchost.exe", stop_only: false },
    SvcEntry { name: "DoSvc", label: "Otimização de Entrega", why: "Compartilha atualizações do Windows/Store com outros PCs (P2P). Consome RAM/rede; sem ele os updates continuam vindo direto.", proc_hint: "svchost.exe", stop_only: false },
    SvcEntry { name: "WerSvc", label: "Relatório de Erros do Windows", why: "Empacota e envia crash dumps para a Microsoft (WerFault.exe).", proc_hint: "werfault.exe", stop_only: false },
    SvcEntry { name: "MapsBroker", label: "Mapas offline", why: "Gerenciador de download de mapas do app Mapas. Dispensável se você não usa.", proc_hint: "svchost.exe", stop_only: false },
    SvcEntry { name: "PhoneSvc", label: "Serviço de Telefone (Phone Link)", why: "Suporte ao Phone Link / Vincular ao Celular.", proc_hint: "svchost.exe", stop_only: false },
    SvcEntry { name: "XblAuthManager", label: "Xbox Live Auth", why: "Login Xbox Live. Só importa se você usa jogos/app Xbox.", proc_hint: "svchost.exe", stop_only: false },
    SvcEntry { name: "XblGameSave", label: "Xbox Live Game Save", why: "Sincroniza saves de jogos Xbox.", proc_hint: "svchost.exe", stop_only: false },
    SvcEntry { name: "XboxNetApiSvc", label: "Xbox Live Networking", why: "Rede para jogos Xbox Live.", proc_hint: "svchost.exe", stop_only: false },
    SvcEntry { name: "lfsvc", label: "Geolocalização", why: "Serviço de localização. Apps que pedem sua localização deixam de funcionar.", proc_hint: "svchost.exe", stop_only: false },
    SvcEntry { name: "RemoteRegistry", label: "Registro Remoto", why: "Permite editar seu registro pela rede. Melhor desligado.", proc_hint: "svchost.exe", stop_only: false },
    SvcEntry { name: "Fax", label: "Fax", why: "É 2026.", proc_hint: "fxssvc.exe", stop_only: false },
    SvcEntry { name: "wuauserv", label: "Windows Update (parar só agora)", why: "Motor do Windows Update (TiWorker/MoUsoCoreWorker vêm daqui). O Windows o religa sozinho; parar dá alívio temporário durante trabalho pesado.", proc_hint: "tiworker.exe", stop_only: true },
];

/// Serviços que o Windows protege — mostrar, mas sem botão.
pub const PROTECTED_SERVICES: &[(&str, &str)] = &[
    ("WinDefend", "Microsoft Defender Antivirus (MsMpEng.exe)"),
    ("WdNisSvc", "Defender Network Inspection (NisSrv.exe)"),
    ("MDCoreSvc", "Defender Core Service (MpDefenderCoreService.exe)"),
];

pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Lê uma string terminada em NUL a partir de um ponteiro UTF-16 do Win32.
pub(crate) unsafe fn from_wide(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0usize;
    while *p.add(n) != 0 {
        n += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
}

/// Qual serviço roda dentro de cada PID.
///
/// Existe por causa do `svchost.exe`: vinte processos de nome idêntico não dizem nada, e
/// a linha de comando (`-k netsvcs`) diz menos ainda. Com isto cada um vira "Windows
/// Update", "Áudio", "Spooler". Não exige admin — `SC_MANAGER_ENUMERATE_SERVICE` é
/// concedido a usuários comuns.
///
/// Um processo pode hospedar vários serviços, daí o `Vec`. O par é
/// `(nome interno, nome de exibição)`.
pub fn services_by_pid() -> HashMap<u32, Vec<(String, String)>> {
    let mut out: HashMap<u32, Vec<(String, String)>> = HashMap::new();
    unsafe {
        let scm = match OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_ENUMERATE_SERVICE) {
            Ok(h) => ScHandle(h),
            Err(_) => return out,
        };
        let mut needed = 0u32;
        let mut returned = 0u32;
        // Primeira chamada só para dimensionar: ela falha com ERROR_MORE_DATA de propósito.
        let _ = EnumServicesStatusExW(
            scm.0,
            SC_ENUM_PROCESS_INFO,
            SERVICE_WIN32,
            SERVICE_STATE_ALL,
            None,
            &mut needed,
            &mut returned,
            None,
            PCWSTR::null(),
        );
        if needed == 0 {
            return out;
        }
        let mut buf = vec![0u8; needed as usize];
        if EnumServicesStatusExW(
            scm.0,
            SC_ENUM_PROCESS_INFO,
            SERVICE_WIN32,
            SERVICE_STATE_ALL,
            Some(&mut buf),
            &mut needed,
            &mut returned,
            None,
            PCWSTR::null(),
        )
        .is_err()
        {
            return out;
        }
        let items = buf.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW;
        for i in 0..returned as usize {
            let e = &*items.add(i);
            let pid = e.ServiceStatusProcess.dwProcessId;
            if pid == 0 {
                continue; // serviço parado: não pertence a processo nenhum
            }
            let name = from_wide(e.lpServiceName.0);
            let display = from_wide(e.lpDisplayName.0);
            out.entry(pid).or_default().push((name, display));
        }
    }
    for v in out.values_mut() {
        v.sort_by_key(|(_, display)| display.to_lowercase());
    }
    out
}

pub(crate) struct ScHandle(pub windows::Win32::System::Services::SC_HANDLE);
impl Drop for ScHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseServiceHandle(self.0);
        }
    }
}

pub fn query_service(name: &str) -> SvcStatus {
    unsafe {
        let scm = match OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) {
            Ok(h) => ScHandle(h),
            Err(_) => return SvcStatus { state: SvcState::Missing, start: SvcStart::Unknown },
        };
        let w = wide(name);
        let svc = match OpenServiceW(scm.0, PCWSTR(w.as_ptr()), SERVICE_QUERY_STATUS | SERVICE_QUERY_CONFIG) {
            Ok(h) => ScHandle(h),
            Err(_) => return SvcStatus { state: SvcState::Missing, start: SvcStart::Unknown },
        };
        let mut st = SERVICE_STATUS::default();
        let state = if QueryServiceStatus(svc.0, &mut st).is_ok() {
            match st.dwCurrentState {
                SERVICE_RUNNING => SvcState::Running,
                SERVICE_STOPPED => SvcState::Stopped,
                _ => SvcState::Pending,
            }
        } else {
            SvcState::Missing
        };
        let mut needed = 0u32;
        let _ = QueryServiceConfigW(svc.0, None, 0, &mut needed);
        let mut start = SvcStart::Unknown;
        if needed > 0 {
            let mut buf = vec![0u8; needed as usize];
            let cfg = buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW;
            if QueryServiceConfigW(svc.0, Some(cfg), needed, &mut needed).is_ok() {
                start = match (*cfg).dwStartType {
                    SERVICE_AUTO_START => SvcStart::Auto,
                    SERVICE_DEMAND_START => SvcStart::Manual,
                    SERVICE_DISABLED => SvcStart::Disabled,
                    _ => SvcStart::Unknown,
                };
            }
        }
        SvcStatus { state, start }
    }
}

fn open_svc_for_change(name: &str, access: u32) -> Result<(ScHandle, ScHandle), String> {
    unsafe {
        let scm = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT).map_err(|e| e.message())?;
        let scm = ScHandle(scm);
        let w = wide(name);
        let svc = OpenServiceW(scm.0, PCWSTR(w.as_ptr()), access).map_err(|e| {
            let c = e.code().0 as u32 & 0xFFFF;
            if c == 5 { "acesso negado — precisa de admin".to_string() } else { e.message() }
        })?;
        Ok((scm, ScHandle(svc)))
    }
}

/// Para o serviço (precisa de admin). Não espera o stop completar.
pub fn stop_service(name: &str) -> Result<(), String> {
    let (_scm, svc) = open_svc_for_change(name, SERVICE_STOP)?;
    unsafe {
        let mut st = SERVICE_STATUS::default();
        ControlService(svc.0, SERVICE_CONTROL_STOP, &mut st).map_err(|e| {
            let c = e.code().0 as u32 & 0xFFFF;
            match c {
                1062 => "já estava parado".to_string(),
                1051 => "outros serviços dependem dele — pare-os primeiro".to_string(),
                1052 | 1061 => "o serviço não aceita parar agora".to_string(),
                _ => e.message(),
            }
        })
    }
}

pub fn start_service(name: &str) -> Result<(), String> {
    let (_scm, svc) = open_svc_for_change(name, SERVICE_START)?;
    unsafe { StartServiceW(svc.0, None).map_err(|e| e.message()) }
}

pub fn set_start_type(name: &str, start: SvcStart) -> Result<(), String> {
    let (_scm, svc) = open_svc_for_change(name, SERVICE_CHANGE_CONFIG)?;
    let st: SERVICE_START_TYPE = match start {
        SvcStart::Auto => SERVICE_AUTO_START,
        SvcStart::Manual => SERVICE_DEMAND_START,
        SvcStart::Disabled => SERVICE_DISABLED,
        SvcStart::Unknown => return Err("tipo inválido".into()),
    };
    unsafe {
        ChangeServiceConfigW(
            svc.0,
            windows::Win32::System::Services::ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
            st,
            SERVICE_ERROR(SERVICE_NO_CHANGE),
            PCWSTR::null(),
            PCWSTR::null(),
            None,
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR::null(),
        )
        .map_err(|e| e.message())
    }
}

// ---------- Registro (helpers) ----------

pub(crate) struct RegKey(pub HKEY);
impl Drop for RegKey {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

pub(crate) fn reg_open(root: HKEY, path: &str, write: bool) -> Option<RegKey> {
    unsafe {
        let mut h = HKEY::default();
        let w = wide(path);
        let access = if write { KEY_READ | KEY_SET_VALUE } else { KEY_READ };
        RegOpenKeyExW(root, PCWSTR(w.as_ptr()), None, access, &mut h).ok().ok()?;
        Some(RegKey(h))
    }
}

pub(crate) fn reg_dword(root: HKEY, path: &str, value: &str) -> Option<u32> {
    let k = reg_open(root, path, false)?;
    unsafe {
        let w = wide(value);
        let mut ty = REG_VALUE_TYPE(0);
        let mut data = [0u8; 4];
        let mut len = 4u32;
        RegQueryValueExW(k.0, PCWSTR(w.as_ptr()), None, Some(&mut ty), Some(data.as_mut_ptr()), Some(&mut len)).ok().ok()?;
        (ty == REG_DWORD).then(|| u32::from_le_bytes(data))
    }
}

/// Enumera valores (nome, dado-como-string-ou-bytes) de uma chave.
pub(crate) fn reg_values(k: &RegKey) -> Vec<(String, REG_VALUE_TYPE, Vec<u8>)> {
    let mut out = Vec::new();
    unsafe {
        let mut i = 0u32;
        loop {
            let mut name = vec![0u16; 16384];
            let mut name_len = name.len() as u32;
            let mut ty = REG_VALUE_TYPE(0);
            let mut data = vec![0u8; 65536];
            let mut data_len = data.len() as u32;
            let r = RegEnumValueW(
                k.0,
                i,
                Some(PWSTR(name.as_mut_ptr())),
                &mut name_len,
                None,
                Some(&mut ty.0),
                Some(data.as_mut_ptr()),
                Some(&mut data_len),
            );
            if r.is_err() {
                break;
            }
            data.truncate(data_len as usize);
            out.push((String::from_utf16_lossy(&name[..name_len as usize]), ty, data));
            i += 1;
        }
    }
    out
}

pub(crate) fn reg_subkeys(k: &RegKey) -> Vec<String> {
    let mut out = Vec::new();
    unsafe {
        let mut i = 0u32;
        loop {
            let mut name = vec![0u16; 512];
            let mut len = name.len() as u32;
            if RegEnumKeyExW(k.0, i, Some(PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None).is_err() {
                break;
            }
            out.push(String::from_utf16_lossy(&name[..len as usize]));
            i += 1;
        }
    }
    out
}

pub(crate) fn utf16_bytes_to_string(b: &[u8]) -> String {
    let w: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let s = String::from_utf16_lossy(&w);
    s.trim_end_matches('\0').to_string()
}

pub(crate) fn reg_create(root: HKEY, path: &str) -> Result<RegKey, String> {
    unsafe {
        let mut h = HKEY::default();
        let w = wide(path);
        let mut disp = windows::Win32::System::Registry::REG_CREATE_KEY_DISPOSITION::default();
        RegCreateKeyExW(
            root,
            PCWSTR(w.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_SET_VALUE,
            None,
            &mut h,
            Some(&mut disp),
        )
        .ok()
        .map_err(|e| {
            let c = e.code().0 as u32 & 0xFFFF;
            if c == 5 { "acesso negado — precisa de admin".into() } else { e.message() }
        })?;
        Ok(RegKey(h))
    }
}

pub(crate) fn reg_set_binary(root: HKEY, path: &str, name: &str, data: &[u8]) -> Result<(), String> {
    let k = match reg_open(root, path, true) {
        Some(k) => k,
        None => reg_create(root, path)?,
    };
    unsafe {
        let w = wide(name);
        RegSetValueExW(k.0, PCWSTR(w.as_ptr()), None, REG_BINARY, Some(data)).ok().map_err(|e| e.message())
    }
}

pub(crate) fn reg_set_dword(root: HKEY, path: &str, name: &str, value: u32) -> Result<(), String> {
    let k = match reg_open(root, path, true) {
        Some(k) => k,
        None => reg_create(root, path)?,
    };
    unsafe {
        let w = wide(name);
        let bytes = value.to_le_bytes();
        RegSetValueExW(k.0, PCWSTR(w.as_ptr()), None, REG_DWORD, Some(&bytes)).ok().map_err(|e| e.message())
    }
}

pub(crate) fn reg_delete_value(root: HKEY, path: &str, name: &str) -> Result<(), String> {
    let k = reg_open(root, path, true).ok_or_else(|| "acesso negado — precisa de admin".to_string())?;
    unsafe {
        let w = wide(name);
        RegDeleteValueW(k.0, PCWSTR(w.as_ptr())).ok().map_err(|e| e.message())
    }
}

// ---------- Defender ----------

#[derive(Clone, Debug, Default)]
pub struct DefenderStatus {
    pub realtime_disabled: Option<bool>,
    pub tamper_protection: Option<bool>,
    pub scan_cpu_factor: Option<u32>,
}

pub fn defender_status() -> DefenderStatus {
    let base = "SOFTWARE\\Microsoft\\Windows Defender";
    let mut d = DefenderStatus::default();
    // Real-Time Protection: valor só existe quando desativado por política; sem valor = ativo.
    d.realtime_disabled = Some(
        reg_dword(HKEY_LOCAL_MACHINE, &format!("{base}\\Real-Time Protection"), "DisableRealtimeMonitoring") == Some(1)
            || reg_dword(HKEY_LOCAL_MACHINE, "SOFTWARE\\Policies\\Microsoft\\Windows Defender\\Real-Time Protection", "DisableRealtimeMonitoring") == Some(1),
    );
    d.tamper_protection = reg_dword(HKEY_LOCAL_MACHINE, &format!("{base}\\Features"), "TamperProtection").map(|v| v == 5);
    d.scan_cpu_factor = reg_dword(HKEY_LOCAL_MACHINE, &format!("{base}\\Scan"), "AvgCPULoadFactor")
        .or_else(|| reg_dword(HKEY_LOCAL_MACHINE, "SOFTWARE\\Policies\\Microsoft\\Windows Defender\\Scan", "AvgCPULoadFactor"));
    d
}

// ---------- Apps de sistema (Appx) ----------

pub struct AppxEntry {
    pub family_prefix: &'static str,
    pub label: &'static str,
    pub why: &'static str,
    /// nomes de processo (minúsculo) que ele mantém vivos
    pub procs: &'static [&'static str],
    /// nome para Get-AppxPackage
    pub pkg_name: &'static str,
}

pub const APPX: &[AppxEntry] = &[
    AppxEntry { family_prefix: "MicrosoftWindows.Client.WebExperience", label: "Widgets (Web Experience Pack)", why: "O painel de widgets/notícias da barra de tarefas. Mantém Widgets.exe + WebView2 vivos.", procs: &["widgets.exe", "widgetservice.exe"], pkg_name: "MicrosoftWindows.Client.WebExperience" },
    AppxEntry { family_prefix: "Microsoft.YourPhone", label: "Phone Link (Vincular ao Celular)", why: "Espelhamento do celular. Roda PhoneExperienceHost.exe em segundo plano.", procs: &["phoneexperiencehost.exe", "yourphone.exe"], pkg_name: "Microsoft.YourPhone" },
    AppxEntry { family_prefix: "Microsoft.XboxGamingOverlay", label: "Xbox Game Bar", why: "Overlay Win+G, gravação de jogos. GameBar.exe + GameBarPresenceWriter.", procs: &["gamebar.exe", "gamebarpresencewriter.exe", "gamebarftserver.exe"], pkg_name: "Microsoft.XboxGamingOverlay" },
    AppxEntry { family_prefix: "Microsoft.Copilot", label: "Copilot (app)", why: "App Copilot do Windows (WebView2).", procs: &["copilot.exe"], pkg_name: "Microsoft.Copilot" },
    AppxEntry { family_prefix: "Microsoft.549981C3F5F10", label: "Cortana", why: "Assistente antigo, sem função no Windows 11 atual.", procs: &["cortana.exe", "searchapp.exe"], pkg_name: "Microsoft.549981C3F5F10" },
    AppxEntry { family_prefix: "MicrosoftTeams", label: "Teams (pessoal, integrado)", why: "Teams \"consumer\" que vem com o Windows e abre sozinho.", procs: &["ms-teams.exe", "msteams.exe"], pkg_name: "MicrosoftTeams" },
    AppxEntry { family_prefix: "Microsoft.BingNews", label: "Microsoft News", why: "App de notícias com tarefas em segundo plano.", procs: &[], pkg_name: "Microsoft.BingNews" },
    AppxEntry { family_prefix: "Microsoft.GetHelp", label: "Obter Ajuda", why: "App de suporte da Microsoft.", procs: &[], pkg_name: "Microsoft.GetHelp" },
    AppxEntry { family_prefix: "Microsoft.MicrosoftSolitaireCollection", label: "Solitaire Collection", why: "Jogos com anúncios pré-instalados.", procs: &[], pkg_name: "Microsoft.MicrosoftSolitaireCollection" },
];

/// Pacotes Appx instalados para o usuário atual (só os nomes de família), lido do registro — sem PowerShell.
pub fn installed_appx_families() -> Vec<String> {
    let path = "Software\\Classes\\Local Settings\\Software\\Microsoft\\Windows\\CurrentVersion\\AppModel\\Repository\\Packages";
    match reg_open(HKEY_CURRENT_USER, path, false) {
        Some(k) => reg_subkeys(&k),
        None => Vec::new(),
    }
}

// ---------- PowerShell elevado ----------

/// Resultado assíncrono de uma ação de sistema (para toast + refresh).
pub struct SysResult {
    pub label: String,
    pub result: Result<(), String>,
}

/// Roda um script PowerShell elevado (UAC) e manda o resultado pelo canal. Nunca bloqueia a UI.
pub fn run_elevated_ps(label: String, script: String, tx: Sender<SysResult>) {
    std::thread::spawn(move || {
        let result = run_elevated_ps_blocking(&script);
        let _ = tx.send(SysResult { label, result });
    });
}

fn run_elevated_ps_blocking(script: &str) -> Result<(), String> {
    // -Command com o script; erro vira exit code 1 (`$ErrorActionPreference='Stop'` + try/catch).
    let wrapped = format!(
        "$ErrorActionPreference='Stop'; try {{ {script}; exit 0 }} catch {{ [Console]::Error.WriteLine($_); exit 1 }}"
    );
    let args = format!("-NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -Command \"{}\"", wrapped.replace('"', "\\\""));
    unsafe {
        let verb = wide("runas");
        let file = wide("powershell.exe");
        let params = wide(&args);
        let mut sei = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS,
            lpVerb: PCWSTR(verb.as_ptr()),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(params.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        ShellExecuteExW(&mut sei).map_err(|e| {
            let c = e.code().0 as u32 & 0xFFFF;
            if c == 1223 { "cancelado no UAC".to_string() } else { e.message() }
        })?;
        if sei.hProcess.is_invalid() {
            return Err("não foi possível iniciar o PowerShell elevado".into());
        }
        let h: HANDLE = sei.hProcess;
        let w = WaitForSingleObject(h, INFINITE);
        let mut code = 1u32;
        if w == WAIT_OBJECT_0 {
            let _ = GetExitCodeProcess(h, &mut code);
        }
        let _ = CloseHandle(h);
        if code == 0 {
            Ok(())
        } else {
            Err(format!("PowerShell terminou com código {code} (Tamper Protection ou política pode ter bloqueado)"))
        }
    }
}

/// Escapa uma string para literal PowerShell entre aspas simples.
pub fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

// silencia import não usado em alguns builds
#[allow(dead_code)]
fn _unused(_: *const c_void, _: SERVICE_STATUS_CURRENT_STATE) -> u32 {
    ERROR_INSUFFICIENT_BUFFER.0
}
