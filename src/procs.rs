//! Coleta de processos via NtQuerySystemInformation (mesma fonte do Task Manager),
//! caminho/linha de comando via NtQueryInformationProcess, kill via TerminateProcess.

use std::collections::HashMap;
#[cfg(windows)]
use std::time::Instant;

#[cfg(windows)]
use std::ffi::c_void;
#[cfg(windows)]
use windows::core::PWSTR;
#[cfg(windows)]
use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SystemProcessInformation};
#[cfg(windows)]
use windows::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation, ProcessCommandLineInformation};
#[cfg(windows)]
use windows::Win32::Foundation::{CloseHandle, HANDLE, STATUS_INFO_LENGTH_MISMATCH, UNICODE_STRING};
#[cfg(windows)]
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
#[cfg(windows)]
use windows::Win32::Security::{
    AdjustTokenPrivileges, LookupPrivilegeValueW, SE_DEBUG_NAME, SE_PRIVILEGE_ENABLED,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES,
};
#[cfg(windows)]
use windows::Win32::System::ProcessStatus::{GetPerformanceInfo, PERFORMANCE_INFORMATION};
#[cfg(windows)]
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
#[cfg(windows)]
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetSystemTimes, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
    TerminateProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    PROCESS_VM_READ,
};
#[cfg(windows)]
use windows::Win32::Foundation::FILETIME;
#[cfg(windows)]
use windows::Win32::UI::Shell::IsUserAnAdmin;

/// Layout real de SYSTEM_PROCESS_INFORMATION (o crate `windows` expõe campos como Reserved*).
#[cfg(windows)]
#[repr(C)]
#[allow(dead_code)]
struct SysProcInfo {
    next_entry_offset: u32,
    number_of_threads: u32,
    working_set_private_size: i64,
    hard_fault_count: u32,
    number_of_threads_high_watermark: u32,
    cycle_time: u64,
    create_time: i64,
    user_time: i64,
    kernel_time: i64,
    image_name: UNICODE_STRING,
    base_priority: i32,
    unique_process_id: HANDLE,
    inherited_from_unique_process_id: HANDLE,
    handle_count: u32,
    session_id: u32,
    unique_process_key: usize,
    peak_virtual_size: usize,
    virtual_size: usize,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
    private_page_count: usize,
    read_operation_count: i64,
    write_operation_count: i64,
    other_operation_count: i64,
    read_transfer_count: i64,
    write_transfer_count: i64,
    other_transfer_count: i64,
}

#[cfg(windows)]
struct OwnedHandle(HANDLE);
#[cfg(windows)]
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Quem lançou o processo, deduzido das variáveis de ambiente herdadas
/// (sobrevive mesmo quando o pai já morreu). Só guardamos os campos abaixo — nunca o ambiente inteiro.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Launcher {
    /// Agente de IA que originou a cadeia: "Claude Code", "Codex", "Cursor Agent", "Gemini CLI"...
    pub agent: Option<String>,
    /// PID do agente (CLAUDE_PID), se informado.
    pub agent_pid: Option<u32>,
    /// ID da sessão do agente (abreviado).
    pub session: Option<String>,
    /// Terminal/host: "Maestri", "VS Code", "Cursor", "Windows Terminal", "Alacritty"...
    pub host: Option<String>,
    /// INIT_CWD (diretório do projeto quando veio de npm/pnpm/yarn).
    pub init_cwd: Option<String>,
    /// npm_lifecycle_event ("dev", "start"...).
    pub npm_script: Option<String>,
}

impl Launcher {
    pub fn is_empty(&self) -> bool {
        self.agent.is_none() && self.host.is_none() && self.init_cwd.is_none()
    }
    /// Rótulo curto: "Claude Code · Maestri", "Maestri", "VS Code"...
    pub fn short(&self) -> String {
        match (&self.agent, &self.host) {
            (Some(a), Some(h)) => format!("{a} · {h}"),
            (Some(a), None) => a.clone(),
            (None, Some(h)) => h.clone(),
            (None, None) => String::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProcInfo {
    pub pid: u32,
    /// Pai validado (existe e foi criado antes deste). 0 = sem pai conhecido.
    pub ppid: u32,
    /// PPID cru como reportado pelo kernel (pode ser reciclado).
    pub raw_ppid: u32,
    pub name: String,
    pub name_lower: String,
    pub exe_path: String,
    pub cmdline: String,
    pub private_ws: u64,
    pub working_set: u64,
    pub commit: u64,
    pub threads: u32,
    pub handles: u32,
    pub session: u32,
    /// FILETIME (100 ns desde 1601)
    pub create_time: i64,
    /// % de CPU (todos os núcleos = 100%) já suavizada — é o que a coluna mostra e o que
    /// a ordenação usa. Ver `CPU_EMA_TAU`.
    pub cpu_pct: f32,
    /// A mesma medida sem suavização, do último intervalo. Só aparece no detalhe/tooltip:
    /// serve para conferir um pico que a média ainda está subindo para alcançar.
    pub cpu_raw_pct: f32,
    /// Bytes/s de disco (leitura + escrita), delta entre amostras.
    pub disk_bps: f64,
    /// % de uso de GPU somado entre engines (preenchido por `gpu::Gpu`).
    pub gpu_pct: f32,
    pub launcher: Launcher,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MemStatus {
    pub total_phys: u64,
    pub avail_phys: u64,
    pub total_commit: u64,
    pub avail_commit: u64,
}

impl MemStatus {
    pub fn used_phys(&self) -> u64 {
        self.total_phys.saturating_sub(self.avail_phys)
    }
    pub fn used_commit(&self) -> u64 {
        self.total_commit.saturating_sub(self.avail_commit)
    }
}

/// Memória do kernel que não pertence a processo nenhum e por isso nunca aparece numa
/// lista de processos — a maior parte do buraco entre "em uso" e a soma da lista.
///
/// Fonte: `GetPerformanceInfo` (psapi), documentada e estável. Não usamos o campo
/// `SystemCache` dela: no Windows 10+ ele reporta standby + cache (23,6 GB numa máquina
/// com 1,8 GB de cache realmente residente), ou seja memória *disponível*, não em uso.
#[derive(Clone, Copy, Debug, Default)]
pub struct KernelMem {
    /// Pool paginado (`KernelPaged`). Drivers e o próprio kernel; pode ser paginado ao disco,
    /// então superestima o residente em ~2% na prática.
    pub paged_pool: u64,
    /// Pool não-paginado (`KernelNonpaged`). Sempre residente na RAM física.
    pub nonpaged_pool: u64,
    /// `false` quando a chamada falhou — a UI omite as linhas em vez de mostrar zero.
    pub ok: bool,
}

#[cfg(windows)]
pub fn kernel_mem() -> KernelMem {
    let mut pi = PERFORMANCE_INFORMATION::default();
    let cb = std::mem::size_of::<PERFORMANCE_INFORMATION>() as u32;
    unsafe {
        if GetPerformanceInfo(&mut pi, cb).is_err() {
            return KernelMem::default();
        }
    }
    let ps = pi.PageSize as u64;
    KernelMem {
        paged_pool: pi.KernelPaged as u64 * ps,
        nonpaged_pool: pi.KernelNonpaged as u64 * ps,
        ok: true,
    }
}

#[cfg(not(windows))]
pub fn kernel_mem() -> KernelMem {
    // macOS/Linux: a wired memory do kernel não tem equivalente barato via sysinfo.
    KernelMem::default()
}

#[cfg(windows)]
pub fn mem_status() -> MemStatus {
    let mut m = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    unsafe {
        if GlobalMemoryStatusEx(&mut m).is_err() {
            return MemStatus::default();
        }
    }
    MemStatus {
        total_phys: m.ullTotalPhys,
        avail_phys: m.ullAvailPhys,
        total_commit: m.ullTotalPageFile,
        avail_commit: m.ullAvailPageFile,
    }
}

/// Info imutável durante a vida do processo (cacheada por (pid, create_time)).
#[cfg(windows)]
#[derive(Clone, Default)]
struct StaticInfo {
    exe_path: String,
    cmdline: String,
    launcher: Launcher,
}

/// Constante de tempo da média móvel da coluna CPU, em segundos.
///
/// O valor cru de um intervalo de 1 s é honesto e ilegível: qualquer processo real
/// alterna entre rajada e espera, então a coluna piscava 0 → 14 → 2 → 9 e a lista se
/// reordenava inteira a cada amostra — dava para ver que *alguém* estava comendo CPU,
/// nunca *quem*. Com τ = 1 s a média chega a ~63% do valor novo na primeira amostra e a
/// ~95% na terceira: rápido o bastante para um pico aparecer, lento o bastante para o
/// topo da lista parar quieto o tempo de você ler.
#[cfg(windows)]
const CPU_EMA_TAU: f64 = 1.0;

#[cfg(windows)]
#[derive(Clone, Copy)]
struct CpuSample {
    total_100ns: i64,
    /// Ciclos de CPU realmente gastos pelo processo (`CycleTime` do kernel).
    cycles: u64,
    io_bytes: u64,
    /// Última média móvel, para o próximo passo do EMA.
    ema: f32,
    at: Instant,
}

#[cfg(windows)]
pub struct Sampler {
    statics: HashMap<(u32, i64), StaticInfo>,
    cpu_prev: HashMap<(u32, i64), CpuSample>,
    /// `kernel + user` da última leitura de `GetSystemTimes` — a capacidade total de CPU
    /// contabilizada pelo kernel, que é o denominador de todo o cálculo abaixo.
    sys_prev: Option<u64>,
    last_at: Option<Instant>,
    ncpu: f64,
    buf: Vec<u8>,
}

#[cfg(windows)]
fn ft(f: FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

#[cfg(windows)]
impl Sampler {
    pub fn new() -> Self {
        let ncpu = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1) as f64;
        Self {
            statics: HashMap::new(),
            cpu_prev: HashMap::new(),
            sys_prev: None,
            last_at: None,
            ncpu,
            buf: vec![0u8; 512 * 1024],
        }
    }

    /// Quanto tempo-de-CPU o kernel contabilizou desde a amostra anterior, em unidades de
    /// 100 ns somadas sobre todos os núcleos (o `kernel` de `GetSystemTimes` já inclui o
    /// ocioso, então `kernel + user` é a capacidade total, não a ocupada).
    ///
    /// Serve de denominador no lugar de `núcleos × relógio de parede`. A diferença
    /// aparece justo na hora que importa: quando a máquina está afogada a nossa thread
    /// acorda atrasada, o relógio de parede conta esse atraso e o denominador incha,
    /// diluindo o culpado exatamente quando você foi olhar quem era.
    fn capacity_delta(&mut self) -> Option<u64> {
        unsafe {
            let (mut idle, mut kern, mut user) =
                (FILETIME::default(), FILETIME::default(), FILETIME::default());
            if GetSystemTimes(Some(&mut idle), Some(&mut kern), Some(&mut user)).is_err() {
                return None;
            }
            let total = ft(kern) + ft(user);
            let prev = self.sys_prev.replace(total);
            prev.and_then(|p| total.checked_sub(p)).filter(|d| *d > 0)
        }
    }

    pub fn sample(&mut self) -> Vec<ProcInfo> {
        let now = Instant::now();
        let raw = self.query_raw();
        let cpu = self.cpu_shares(&raw, now);
        let mut out: Vec<ProcInfo> = Vec::with_capacity(raw.len());
        let mut seen: HashMap<(u32, i64), ()> = HashMap::with_capacity(raw.len());

        for (i, r) in raw.iter().enumerate() {
            let key = (r.pid, r.create_time);
            seen.insert(key, ());
            let st = match self.statics.get(&key) {
                Some(s) => s.clone(),
                None => {
                    let s = query_static(r.pid);
                    self.statics.insert(key, s.clone());
                    s
                }
            };
            let total = r.user_time + r.kernel_time;
            let (cpu_raw_pct, cpu_pct) = cpu[i];
            let disk_bps = match self.cpu_prev.get(&key) {
                Some(prev) => {
                    let dt = now.duration_since(prev.at).as_secs_f64();
                    if dt > 0.0 {
                        r.io_bytes.saturating_sub(prev.io_bytes) as f64 / dt
                    } else {
                        0.0
                    }
                }
                None => 0.0,
            };
            self.cpu_prev.insert(
                key,
                CpuSample {
                    total_100ns: total,
                    cycles: r.cycle_time,
                    io_bytes: r.io_bytes,
                    ema: cpu_pct,
                    at: now,
                },
            );

            let name = if r.name.is_empty() {
                if r.pid == 4 { "System".into() } else { format!("[pid {}]", r.pid) }
            } else {
                r.name.clone()
            };
            out.push(ProcInfo {
                pid: r.pid,
                ppid: 0,
                raw_ppid: r.ppid,
                name_lower: name.to_lowercase(),
                name,
                exe_path: st.exe_path,
                cmdline: st.cmdline,
                private_ws: r.private_ws,
                working_set: r.working_set,
                commit: r.commit,
                threads: r.threads,
                handles: r.handles,
                session: r.session,
                create_time: r.create_time,
                cpu_pct,
                cpu_raw_pct,
                disk_bps,
                gpu_pct: 0.0,
                launcher: st.launcher,
            });
        }
        self.statics.retain(|k, _| seen.contains_key(k));
        self.cpu_prev.retain(|k, _| seen.contains_key(k));

        // Valida pai: precisa existir e ter sido criado antes do filho (PIDs são reciclados).
        let by_pid: HashMap<u32, i64> = out.iter().map(|p| (p.pid, p.create_time)).collect();
        for p in out.iter_mut() {
            if p.raw_ppid != 0 && p.raw_ppid != p.pid {
                if let Some(&pct) = by_pid.get(&p.raw_ppid) {
                    if pct <= p.create_time {
                        p.ppid = p.raw_ppid;
                    }
                }
            }
        }
        out
    }

    /// `(cru, suavizado)` de CPU para cada entrada de `raw`, na mesma ordem.
    ///
    /// Duas decisões que separam este número do delta de `KernelTime + UserTime` que
    /// estava aqui antes:
    ///
    /// 1. **A repartição é por ciclos, não por tempo.** O kernel só sabe cobrar tempo em
    ///    fatias inteiras de 15,625 ms, e cobra a fatia toda de quem estava rodando no
    ///    instante do tique. Quem acorda em rajadas de 1 ms leva 0 numa amostra e 15 ms na
    ///    seguinte — é dessa loteria que vinha o número dançando. `CycleTime` é contado a
    ///    cada troca de contexto e já vinha de graça no mesmo buffer, apenas descartado.
    /// 2. **O total repartido é o tempo medido, não os ciclos.** Ciclo não converte para
    ///    porcentagem: o mesmo ciclo vale menos num E-core e menos ainda com o clock
    ///    baixo. Então a soma a distribuir vem do relógio (`Σ tempo / capacidade`) e os
    ///    ciclos só dizem *como* dividir essa soma entre os processos.
    ///
    /// O que morreu entre as duas amostras fica de fora dos dois lados da conta, e por
    /// isso não é redistribuído: se um `cl.exe` queimou 30% e saiu, ninguém herda esses
    /// 30% — eles somem da lista, como devem, e a diferença para o medidor do topo
    /// continua sendo o que a lista honestamente não sabe explicar.
    fn cpu_shares(&mut self, raw: &[RawProc], now: Instant) -> Vec<(f32, f32)> {
        let capacity = self.capacity_delta().map(|d| d as f64).unwrap_or_else(|| {
            // `GetSystemTimes` falhou: cai no relógio de parede × núcleos.
            let dt = self.last_at.map(|t| now.duration_since(t).as_secs_f64()).unwrap_or(0.0);
            dt * self.ncpu * 1e7
        });
        let dt_wall = self.last_at.map(|t| now.duration_since(t).as_secs_f64()).unwrap_or(0.0);
        self.last_at = Some(now);

        // Deltas de quem existe nas duas amostras. Processo novo entra com zero: sem o
        // instante anterior não há taxa nenhuma para medir.
        let mut d_time: Vec<i64> = Vec::with_capacity(raw.len());
        let mut d_cycles: Vec<u64> = Vec::with_capacity(raw.len());
        let mut sum_time: i64 = 0;
        let mut sum_cycles: u128 = 0;
        for r in raw {
            let (dt, dc) = match self.cpu_prev.get(&(r.pid, r.create_time)) {
                Some(prev) => (
                    (r.user_time + r.kernel_time - prev.total_100ns).max(0),
                    r.cycle_time.saturating_sub(prev.cycles),
                ),
                None => (0, 0),
            };
            sum_time += dt;
            sum_cycles += dc as u128;
            d_time.push(dt);
            d_cycles.push(dc);
        }

        if capacity <= 0.0 {
            return vec![(0.0, 0.0); raw.len()];
        }
        // O quanto da máquina os processos vivos nas duas pontas consumiram, somados.
        let matched_pct = (sum_time as f64 / capacity * 100.0).clamp(0.0, 100.0);
        // Passo do EMA para este intervalo. Amarrado ao tempo, não à contagem de amostras:
        // trocar o intervalo de 0,5 s para 5 s não pode mudar o quanto a coluna alisa.
        let alpha = if dt_wall > 0.0 { 1.0 - (-dt_wall / CPU_EMA_TAU).exp() } else { 1.0 };

        (0..raw.len())
            .map(|i| {
                let pct = if sum_cycles > 0 {
                    (d_cycles[i] as f64 / sum_cycles as f64 * matched_pct) as f32
                } else {
                    // Sem `CycleTime` (Windows antigo ou máquina virtual que zera o campo):
                    // o tempo cru ainda dá a resposta certa, só mais granulada.
                    (d_time[i] as f64 / capacity * 100.0) as f32
                };
                let ema = match self.cpu_prev.get(&(raw[i].pid, raw[i].create_time)) {
                    // Processo já conhecido: continua a média de onde ela estava.
                    Some(prev) => prev.ema + (pct - prev.ema) * alpha as f32,
                    // Primeira vez que o vemos: `pct` é 0 e a média nasce aí mesmo.
                    None => pct,
                };
                (pct, ema)
            })
            .collect()
    }

    fn query_raw(&mut self) -> Vec<RawProc> {
        let mut out = Vec::new();
        unsafe {
            loop {
                let mut ret: u32 = 0;
                let status = NtQuerySystemInformation(
                    SystemProcessInformation,
                    self.buf.as_mut_ptr() as *mut c_void,
                    self.buf.len() as u32,
                    &mut ret,
                );
                if status == STATUS_INFO_LENGTH_MISMATCH {
                    let want = (ret as usize).max(self.buf.len() * 2) + 64 * 1024;
                    self.buf.resize(want, 0);
                    continue;
                }
                if status.is_err() {
                    return out;
                }
                break;
            }
            let base = self.buf.as_ptr();
            let mut off: usize = 0;
            loop {
                let p = base.add(off) as *const SysProcInfo;
                let e = &*p;
                let pid = e.unique_process_id.0 as usize as u32;
                if pid != 0 {
                    let name = if e.image_name.Buffer.is_null() || e.image_name.Length == 0 {
                        String::new()
                    } else {
                        let len = (e.image_name.Length / 2) as usize;
                        let slice = std::slice::from_raw_parts(e.image_name.Buffer.0, len);
                        String::from_utf16_lossy(slice)
                    };
                    out.push(RawProc {
                        pid,
                        ppid: e.inherited_from_unique_process_id.0 as usize as u32,
                        name,
                        private_ws: e.working_set_private_size.max(0) as u64,
                        working_set: e.working_set_size as u64,
                        commit: e.pagefile_usage as u64,
                        threads: e.number_of_threads,
                        handles: e.handle_count,
                        session: e.session_id,
                        create_time: e.create_time,
                        user_time: e.user_time,
                        kernel_time: e.kernel_time,
                        cycle_time: e.cycle_time,
                        io_bytes: (e.read_transfer_count.max(0) + e.write_transfer_count.max(0)) as u64,
                    });
                }
                if e.next_entry_offset == 0 {
                    break;
                }
                off += e.next_entry_offset as usize;
            }
        }
        out
    }
}

#[cfg(windows)]
struct RawProc {
    pid: u32,
    ppid: u32,
    name: String,
    private_ws: u64,
    working_set: u64,
    commit: u64,
    threads: u32,
    handles: u32,
    session: u32,
    create_time: i64,
    user_time: i64,
    kernel_time: i64,
    /// Ciclos de CPU do processo. Contado a cada troca de contexto, sem a granularidade
    /// de 15,625 ms do tempo de kernel/usuário.
    cycle_time: u64,
    io_bytes: u64,
}

#[cfg(windows)]
fn query_static(pid: u32) -> StaticInfo {
    let mut st = StaticInfo::default();
    if pid == 4 {
        return st;
    }
    unsafe {
        let h = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(h) => OwnedHandle(h),
            Err(_) => return st,
        };
        // Caminho completo do executável
        let mut buf = vec![0u16; 1024];
        let mut size = buf.len() as u32;
        if QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut size).is_ok() {
            st.exe_path = String::from_utf16_lossy(&buf[..size as usize]);
        }
        // Linha de comando (ProcessCommandLineInformation: não precisa de VM_READ)
        let mut ret: u32 = 0;
        let status = NtQueryInformationProcess(h.0, ProcessCommandLineInformation, std::ptr::null_mut(), 0, &mut ret);
        if status == STATUS_INFO_LENGTH_MISMATCH && ret > 0 {
            let mut cbuf = vec![0u8; ret as usize + 16];
            let status = NtQueryInformationProcess(
                h.0,
                ProcessCommandLineInformation,
                cbuf.as_mut_ptr() as *mut c_void,
                cbuf.len() as u32,
                &mut ret,
            );
            if status.is_ok() {
                let us = &*(cbuf.as_ptr() as *const UNICODE_STRING);
                if !us.Buffer.is_null() && us.Length > 0 {
                    let len = (us.Length / 2) as usize;
                    let slice = std::slice::from_raw_parts(us.Buffer.0, len);
                    st.cmdline = String::from_utf16_lossy(slice);
                }
            }
        }
        st.launcher = read_launcher(pid);
    }
    st
}

/// Lê o bloco de ambiente do processo (PEB → ProcessParameters → Environment) e extrai só as
/// variáveis-impressão-digital de agentes/terminais. Precisa de PROCESS_VM_READ (mesmo usuário
/// ou admin); em caso de falha devolve Launcher vazio.
#[cfg(target_pointer_width = "64")]
#[cfg(windows)]
fn read_launcher(pid: u32) -> Launcher {
    const PEB_PROCESS_PARAMETERS: usize = 0x20;
    const RUPP_ENVIRONMENT: usize = 0x80;
    const RUPP_ENVIRONMENT_SIZE: usize = 0x3F0;
    const MAX_ENV: usize = 2 * 1024 * 1024;
    unsafe {
        let h = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, false, pid) {
            Ok(h) => OwnedHandle(h),
            Err(_) => return Launcher::default(),
        };
        // PROCESS_BASIC_INFORMATION: ExitStatus(4)+pad(4), PebBaseAddress(8), ...
        let mut pbi = [0u8; 48];
        let mut ret: u32 = 0;
        let status = NtQueryInformationProcess(
            h.0,
            ProcessBasicInformation,
            pbi.as_mut_ptr() as *mut c_void,
            pbi.len() as u32,
            &mut ret,
        );
        if status.is_err() {
            return Launcher::default();
        }
        let peb = usize::from_le_bytes(pbi[8..16].try_into().unwrap());
        if peb == 0 {
            return Launcher::default();
        }
        let read_usize = |addr: usize| -> Option<usize> {
            let mut b = [0u8; 8];
            let mut n: usize = 0;
            ReadProcessMemory(h.0, addr as *const c_void, b.as_mut_ptr() as *mut c_void, 8, Some(&mut n)).ok()?;
            (n == 8).then(|| usize::from_le_bytes(b))
        };
        let Some(params) = read_usize(peb + PEB_PROCESS_PARAMETERS) else { return Launcher::default() };
        if params == 0 {
            return Launcher::default();
        }
        let Some(env) = read_usize(params + RUPP_ENVIRONMENT) else { return Launcher::default() };
        if env == 0 {
            return Launcher::default();
        }
        let mut size = read_usize(params + RUPP_ENVIRONMENT_SIZE).unwrap_or(0);
        if size == 0 || size > MAX_ENV {
            size = 64 * 1024;
        }
        let mut buf = vec![0u8; size];
        let mut n: usize = 0;
        if ReadProcessMemory(h.0, env as *const c_void, buf.as_mut_ptr() as *mut c_void, size, Some(&mut n)).is_err() || n < 4 {
            return Launcher::default();
        }
        let words: Vec<u16> = buf[..n].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        parse_launcher(&words)
    }
}

#[cfg(not(target_pointer_width = "64"))]
#[cfg(windows)]
fn read_launcher(_pid: u32) -> Launcher {
    Launcher::default()
}

/// Extrai as impressões digitais de um bloco "KEY=VAL NUL KEY=VAL NUL NUL".
#[cfg(windows)]
fn parse_launcher(words: &[u16]) -> Launcher {
    let lines: Vec<String> = words
        .split(|&w| w == 0)
        .filter(|e| !e.is_empty())
        .map(String::from_utf16_lossy)
        .collect();
    launcher_from_env_lines(&lines)
}

pub(crate) fn launcher_from_env_lines(lines: &[String]) -> Launcher {
    let mut l = Launcher::default();
    let mut host_rank = 0u8; // prioridade: Maestri > VS Code/Cursor > Windows Terminal/outros
    for s in lines {
        let Some((k, v)) = s.split_once('=') else { continue };
        let ku = k.to_ascii_uppercase();
        let mut set_host = |name: &str, rank: u8, l: &mut Launcher| {
            if host_rank < rank {
                l.host = Some(name.to_string());
                host_rank = rank;
            }
        };
        match ku.as_str() {
            "CLAUDECODE" | "CLAUDE_CODE_ENTRYPOINT" | "CLAUDE_CODE_SESSION_ID" | "CLAUDE_CODE_CHILD_SESSION" => {
                l.agent.get_or_insert_with(|| "Claude Code".into());
                if ku == "CLAUDE_CODE_SESSION_ID" {
                    l.session = Some(v.chars().take(8).collect());
                }
            }
            "CLAUDE_PID" => {
                l.agent.get_or_insert_with(|| "Claude Code".into());
                l.agent_pid = v.trim().parse().ok();
            }
            "CODEX_SANDBOX" | "CODEX_SANDBOX_NETWORK_DISABLED" | "CODEX_THREAD_ID" | "CODEX_SESSION_ID"
            | "CODEX_MANAGED_BY_NPM" | "CODEX_CI" => {
                l.agent.get_or_insert_with(|| "Codex".into());
                if ku == "CODEX_THREAD_ID" || ku == "CODEX_SESSION_ID" {
                    l.session = Some(v.chars().take(8).collect());
                }
            }
            "CURSOR_AGENT" => l.agent = Some("Cursor Agent".into()),
            "GEMINI_CLI" => {
                l.agent.get_or_insert_with(|| "Gemini CLI".into());
            }
            "GROK_SESSION_ID" | "GROK_CLI" => {
                l.agent.get_or_insert_with(|| "Grok CLI".into());
            }
            "HERMES_SESSION_ID" | "HERMES_AGENT" => {
                l.agent.get_or_insert_with(|| "Hermes".into());
            }
            "MAESTRI_TERMINAL_ID" | "MAESTRI_WORKSPACE_ID" | "MAESTRI_PIPE" => set_host("Maestri", 3, &mut l),
            "CURSOR_TRACE_ID" => set_host("Cursor", 2, &mut l),
            "TERM_PROGRAM" => {
                let vl = v.to_ascii_lowercase();
                let name = if vl == "vscode" { Some("VS Code") } else if vl.contains("cursor") { Some("Cursor") }
                    else if vl.contains("zed") { Some("Zed") } else if vl.contains("warp") { Some("Warp") }
                    else if vl.contains("iterm") { Some("iTerm") } else if vl.contains("apple_terminal") { Some("Terminal") }
                    else if vl.contains("wezterm") { Some("WezTerm") } else { None };
                if let Some(nm) = name {
                    set_host(nm, 2, &mut l);
                }
            }
            "VSCODE_PID" | "VSCODE_CWD" => set_host("VS Code", 1, &mut l),
            "WT_SESSION" => set_host("Windows Terminal", 1, &mut l),
            "ALACRITTY_WINDOW_ID" | "ALACRITTY_SOCKET" => set_host("Alacritty", 1, &mut l),
            "CONEMUPID" => set_host("ConEmu", 1, &mut l),
            "INIT_CWD" => l.init_cwd = Some(v.to_string()),
            "NPM_LIFECYCLE_EVENT" => l.npm_script = Some(v.to_string()),
            _ => {}
        }
    }
    l
}

#[cfg(windows)]
pub fn kill(pid: u32) -> Result<(), String> {
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, false, pid).map_err(|e| fmt_err(&e))?;
        let h = OwnedHandle(h);
        TerminateProcess(h.0, 1).map_err(|e| fmt_err(&e))
    }
}

#[cfg(windows)]
fn fmt_err(e: &windows::core::Error) -> String {
    let code = e.code().0 as u32 & 0xFFFF;
    match code {
        5 => "acesso negado (tente reabrir como admin)".to_string(),
        87 => "parâmetro inválido (processo já encerrado?)".to_string(),
        _ => e.message().trim().to_string(),
    }
}

#[cfg(windows)]
pub fn is_admin() -> bool {
    unsafe { IsUserAnAdmin().as_bool() }
}

/// Habilita SeDebugPrivilege (só funciona elevado; ignorado silenciosamente caso contrário).
#[cfg(windows)]
pub fn enable_debug_privilege() {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES, &mut token).is_err() {
            return;
        }
        let token = OwnedHandle(token);
        let mut tp = TOKEN_PRIVILEGES::default();
        tp.PrivilegeCount = 1;
        if LookupPrivilegeValueW(None, SE_DEBUG_NAME, &mut tp.Privileges[0].Luid).is_err() {
            return;
        }
        tp.Privileges[0].Attributes = SE_PRIVILEGE_ENABLED;
        let _ = AdjustTokenPrivileges(token.0, false, Some(&tp), 0, None, None);
    }
}

#[cfg(not(windows))]
#[path = "procs_unix.rs"]
mod procs_unix;
#[cfg(not(windows))]
pub use procs_unix::{enable_debug_privilege, is_admin, kill, mem_status, Sampler};

/// FILETIME atual (100 ns desde 1601-01-01 UTC).
pub fn now_filetime() -> i64 {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (d.as_nanos() / 100) as i64 + 116_444_736_000_000_000
}
