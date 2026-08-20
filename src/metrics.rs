//! Métricas de sistema além de RAM: GPU (por processo e total), temperatura e CPU total.
//!
//! - **GPU por processo**: contadores PDH `\GPU Engine(*)\Utilization Percentage`. Os nomes de
//!   instância carregam o PID (`pid_10804_luid_..._engtype_3d`). Usamos
//!   `PdhAddEnglishCounterW` de propósito: neste host (Windows pt-BR) o nome localizado do
//!   contador de disco nem existe, e nome em inglês não depende do idioma do sistema.
//! - **GPU total / temperatura / memória / potência**: `nvml.dll` carregada em tempo de execução
//!   (só existe com driver NVIDIA). Sem NVIDIA, tudo vira `None` — nada de número inventado.
//! - **CPU total**: `GetSystemTimes`, que é a conta exata do kernel, não a soma dos processos.
//! - **Temperatura de CPU**: não implementada de propósito. Medido neste host:
//!   `MSAcpi_ThermalZoneTemperature` responde "sem suporte", `\Thermal Zone Information` não
//!   existe e `Win32_TemperatureProbe` vem vazio. Num Ryzen o sensor (Tctl) fica atrás do SMU,
//!   acessível só em ring-0 — precisaria de driver em modo kernel assinado.

use std::collections::HashMap;
use std::ffi::{c_void, CString};

use windows::core::{PCSTR, PCWSTR};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhGetFormattedCounterValue, PdhOpenQueryW, PDH_FMT, PDH_FMT_COUNTERVALUE, PDH_FMT_COUNTERVALUE_ITEM_W,
    PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY,
};
use windows::Win32::System::Threading::GetSystemTimes;

const PDH_MORE_DATA: u32 = 0x800007D2;
/// PDH_FMT_NOCAP100 — não existe como constante no crate `windows`; sem ela um processo que
/// usa duas engines ao mesmo tempo é truncado em 100 antes de chegarmos ao máximo.
const PDH_FMT_NOCAP100: PDH_FMT = PDH_FMT(0x8000);
const GPU_ENGINE_COUNTER: &str = r"\GPU Engine(*)\Utilization Percentage";
/// "% Idle Time" em vez de "% Disk Time": no Task Manager/Resource Monitor, "uso de disco" e'
/// `100 - idle`. Contador em ingles por `PdhAddEnglishCounterW` -- o nome localizado
/// (`\Disco fisico(_Total)\% Tempo Ocioso`) nem existe neste host pt-BR.
const DISK_IDLE_COUNTER: &str = r"\PhysicalDisk(_Total)\% Idle Time";
/// Throughput do sistema inteiro — o "% ocupado" sozinho fica sempre baixo/zero num SSD NVMe
/// rápido mesmo com I/O real acontecendo; mostrar bytes/s junto é o que prova pro usuário que
/// o contador não está travado.
const DISK_BYTES_COUNTER: &str = r"\PhysicalDisk(_Total)\Disk Bytes/sec";

// ---------- NVML ----------

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct NvmlUtilization {
    gpu: u32,
    memory: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct NvmlMemory {
    total: u64,
    free: u64,
    used: u64,
}

type FnInit = unsafe extern "C" fn() -> i32;
type FnShutdown = unsafe extern "C" fn() -> i32;
type FnHandle = unsafe extern "C" fn(u32, *mut *mut c_void) -> i32;
type FnName = unsafe extern "C" fn(*mut c_void, *mut u8, u32) -> i32;
type FnTemp = unsafe extern "C" fn(*mut c_void, u32, *mut u32) -> i32;
type FnUtil = unsafe extern "C" fn(*mut c_void, *mut NvmlUtilization) -> i32;
type FnMem = unsafe extern "C" fn(*mut c_void, *mut NvmlMemory) -> i32;
type FnPower = unsafe extern "C" fn(*mut c_void, *mut u32) -> i32;
type FnFan = unsafe extern "C" fn(*mut c_void, *mut u32) -> i32;

struct Nvml {
    dev: *mut c_void,
    name: String,
    temp: Option<FnTemp>,
    util: Option<FnUtil>,
    mem: Option<FnMem>,
    power: Option<FnPower>,
    fan: Option<FnFan>,
    shutdown: Option<FnShutdown>,
}

/// Estado da GPU num instante. Campo `None` = o driver não respondeu; a UI mostra "–".
#[derive(Clone, Debug, Default)]
pub struct GpuInfo {
    pub name: String,
    pub util_pct: Option<f32>,
    pub temp_c: Option<u32>,
    pub mem_used: u64,
    pub mem_total: u64,
    pub power_w: Option<f32>,
    pub fan_pct: Option<u32>,
}

impl Nvml {
    fn load() -> Option<Self> {
        unsafe {
            let dll = CString::new("nvml.dll").ok()?;
            let lib = LoadLibraryA(PCSTR(dll.as_ptr() as *const u8)).ok()?;
            let sym = |n: &str| -> Option<*const c_void> {
                let c = CString::new(n).ok()?;
                GetProcAddress(lib, PCSTR(c.as_ptr() as *const u8)).map(|f| f as *const c_void)
            };
            let init: FnInit = std::mem::transmute(sym("nvmlInit_v2").or_else(|| sym("nvmlInit"))?);
            if init() != 0 {
                return None;
            }
            let get_handle: FnHandle = std::mem::transmute(
                sym("nvmlDeviceGetHandleByIndex_v2").or_else(|| sym("nvmlDeviceGetHandleByIndex"))?,
            );
            let mut dev: *mut c_void = std::ptr::null_mut();
            if get_handle(0, &mut dev) != 0 || dev.is_null() {
                return None;
            }
            let mut name = String::new();
            if let Some(f) = sym("nvmlDeviceGetName") {
                let f: FnName = std::mem::transmute(f);
                let mut buf = [0u8; 96];
                if f(dev, buf.as_mut_ptr(), buf.len() as u32) == 0 {
                    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
                    name = String::from_utf8_lossy(&buf[..end]).to_string();
                }
            }
            Some(Self {
                dev,
                name,
                temp: sym("nvmlDeviceGetTemperature").map(|f| std::mem::transmute(f)),
                util: sym("nvmlDeviceGetUtilizationRates").map(|f| std::mem::transmute(f)),
                mem: sym("nvmlDeviceGetMemoryInfo").map(|f| std::mem::transmute(f)),
                power: sym("nvmlDeviceGetPowerUsage").map(|f| std::mem::transmute(f)),
                fan: sym("nvmlDeviceGetFanSpeed").map(|f| std::mem::transmute(f)),
                shutdown: sym("nvmlShutdown").map(|f| std::mem::transmute(f)),
            })
        }
    }

    fn read(&self) -> GpuInfo {
        let mut g = GpuInfo { name: self.name.clone(), ..Default::default() };
        unsafe {
            if let Some(f) = self.util {
                let mut u = NvmlUtilization::default();
                if f(self.dev, &mut u) == 0 {
                    g.util_pct = Some(u.gpu as f32);
                }
            }
            if let Some(f) = self.temp {
                let mut t = 0u32;
                // 0 = NVML_TEMPERATURE_GPU
                if f(self.dev, 0, &mut t) == 0 {
                    g.temp_c = Some(t);
                }
            }
            if let Some(f) = self.mem {
                let mut m = NvmlMemory::default();
                if f(self.dev, &mut m) == 0 {
                    g.mem_used = m.used;
                    g.mem_total = m.total;
                }
            }
            if let Some(f) = self.power {
                let mut mw = 0u32;
                if f(self.dev, &mut mw) == 0 {
                    g.power_w = Some(mw as f32 / 1000.0);
                }
            }
            if let Some(f) = self.fan {
                let mut pct = 0u32;
                if f(self.dev, &mut pct) == 0 {
                    g.fan_pct = Some(pct);
                }
            }
        }
        g
    }
}

impl Drop for Nvml {
    fn drop(&mut self) {
        unsafe {
            if let Some(f) = self.shutdown {
                f();
            }
        }
    }
}

// ---------- PDH: GPU por processo ----------

struct Pdh {
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
    disk_counter: Option<PDH_HCOUNTER>,
    disk_bytes_counter: Option<PDH_HCOUNTER>,
    /// A primeira coleta só estabelece a linha de base; valores valem da segunda em diante.
    primed: bool,
    buf: Vec<u8>,
}

impl Pdh {
    fn open() -> Option<Self> {
        unsafe {
            let mut query = PDH_HQUERY::default();
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != 0 {
                return None;
            }
            let path: Vec<u16> = GPU_ENGINE_COUNTER.encode_utf16().chain(std::iter::once(0)).collect();
            let mut counter = PDH_HCOUNTER::default();
            if PdhAddEnglishCounterW(query, PCWSTR(path.as_ptr()), 0, &mut counter) != 0 {
                let _ = PdhCloseQuery(query);
                return None;
            }
            let disk_path: Vec<u16> = DISK_IDLE_COUNTER.encode_utf16().chain(std::iter::once(0)).collect();
            let mut disk_counter = PDH_HCOUNTER::default();
            let disk_counter = if PdhAddEnglishCounterW(query, PCWSTR(disk_path.as_ptr()), 0, &mut disk_counter) == 0 {
                Some(disk_counter)
            } else {
                None
            };
            let bytes_path: Vec<u16> = DISK_BYTES_COUNTER.encode_utf16().chain(std::iter::once(0)).collect();
            let mut disk_bytes_counter = PDH_HCOUNTER::default();
            let disk_bytes_counter = if PdhAddEnglishCounterW(query, PCWSTR(bytes_path.as_ptr()), 0, &mut disk_bytes_counter) == 0 {
                Some(disk_bytes_counter)
            } else {
                None
            };
            let _ = PdhCollectQueryData(query);
            Some(Self { query, counter, disk_counter, disk_bytes_counter, primed: false, buf: vec![0u8; 64 * 1024] })
        }
    }

    /// % de disco *total* do sistema (todos os volumes), estilo Task Manager: `100 - % ocioso`.
    fn disk_pct(&self) -> Option<f32> {
        let hc = self.disk_counter?;
        unsafe {
            let mut v = PDH_FMT_COUNTERVALUE::default();
            if PdhGetFormattedCounterValue(hc, PDH_FMT_DOUBLE, None, &mut v) != 0 {
                return None;
            }
            Some((100.0 - v.Anonymous.doubleValue).clamp(0.0, 100.0) as f32)
        }
    }

    /// Throughput do sistema inteiro (leitura + escrita, todos os volumes), em bytes/s.
    fn disk_bps(&self) -> Option<f64> {
        let hc = self.disk_bytes_counter?;
        unsafe {
            let mut v = PDH_FMT_COUNTERVALUE::default();
            if PdhGetFormattedCounterValue(hc, PDH_FMT_DOUBLE, None, &mut v) != 0 {
                return None;
            }
            Some(v.Anonymous.doubleValue.max(0.0))
        }
    }

    /// PID → % de GPU. Igual ao Gerenciador de Tarefas, usamos o **máximo** entre engines
    /// (3D, copy, compute...) em vez da soma: somar passa de 100% e não significa nada.
    fn sample(&mut self) -> HashMap<u32, f32> {
        let mut out: HashMap<u32, f32> = HashMap::new();
        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                return out;
            }
            if !self.primed {
                self.primed = true;
                return out;
            }
            let fmt = PDH_FMT(PDH_FMT_DOUBLE.0 | PDH_FMT_NOCAP100.0);
            let mut size: u32 = self.buf.len() as u32;
            let mut count: u32 = 0;
            let mut st = PdhGetFormattedCounterArrayW(
                self.counter,
                fmt,
                &mut size,
                &mut count,
                Some(self.buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W),
            );
            if st == PDH_MORE_DATA {
                self.buf.resize(size as usize + 8192, 0);
                size = self.buf.len() as u32;
                st = PdhGetFormattedCounterArrayW(
                    self.counter,
                    fmt,
                    &mut size,
                    &mut count,
                    Some(self.buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W),
                );
            }
            if st != 0 {
                return out;
            }
            let items =
                std::slice::from_raw_parts(self.buf.as_ptr() as *const PDH_FMT_COUNTERVALUE_ITEM_W, count as usize);
            for it in items {
                if it.szName.is_null() {
                    continue;
                }
                let name = it.szName.to_string().unwrap_or_default();
                let Some(pid) = parse_pid(&name) else { continue };
                let v = it.FmtValue.Anonymous.doubleValue as f32;
                if !v.is_finite() || v <= 0.0 {
                    continue;
                }
                let e = out.entry(pid).or_insert(0.0);
                if v > *e {
                    *e = v;
                }
            }
        }
        out
    }
}

impl Drop for Pdh {
    fn drop(&mut self) {
        unsafe {
            let _ = PdhCloseQuery(self.query);
        }
    }
}

/// `pid_10804_luid_0x00000000_0x000137bc_phys_0_eng_0_engtype_3d` → 10804
fn parse_pid(instance: &str) -> Option<u32> {
    let rest = instance.strip_prefix("pid_")?;
    let end = rest.find('_').unwrap_or(rest.len());
    rest[..end].parse().ok()
}

// ---------- CPU total ----------

fn ft(f: FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

// ---------- fachada ----------

pub struct Metrics {
    nvml: Option<Nvml>,
    pdh: Option<Pdh>,
    /// (idle, kernel+user) da última leitura de `GetSystemTimes`.
    cpu_prev: Option<(u64, u64)>,
}

/// Uma amostra de tudo que não é RAM.
#[derive(Clone, Debug, Default)]
pub struct SysSample {
    pub cpu_pct: Option<f32>,
    pub disk_pct: Option<f32>,
    pub disk_bps: Option<f64>,
    pub gpu: Option<GpuInfo>,
    pub gpu_by_pid: HashMap<u32, f32>,
}

impl Metrics {
    pub fn new() -> Self {
        Self { nvml: Nvml::load(), pdh: Pdh::open(), cpu_prev: None }
    }

    /// Por que a coluna de GPU pode vir vazia — a UI explica em vez de mentir.
    pub fn gpu_per_process_available(&self) -> bool {
        self.pdh.is_some()
    }

    pub fn sample(&mut self) -> SysSample {
        let mut s = SysSample::default();
        if let Some(p) = self.pdh.as_mut() {
            s.gpu_by_pid = p.sample();
            s.disk_pct = p.disk_pct();
            s.disk_bps = p.disk_bps();
        }
        if let Some(n) = self.nvml.as_ref() {
            s.gpu = Some(n.read());
        }
        unsafe {
            let (mut idle, mut kern, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
            if GetSystemTimes(Some(&mut idle), Some(&mut kern), Some(&mut user)).is_ok() {
                // O tempo de kernel já inclui o idle.
                let (i, t) = (ft(idle), ft(kern) + ft(user));
                if let Some((pi, pt)) = self.cpu_prev {
                    let (di, dt) = (i.saturating_sub(pi) as f64, t.saturating_sub(pt) as f64);
                    if dt > 0.0 {
                        s.cpu_pct = Some((((dt - di) / dt) * 100.0).clamp(0.0, 100.0) as f32);
                    }
                }
                self.cpu_prev = Some((i, t));
            }
        }
        s
    }
}
