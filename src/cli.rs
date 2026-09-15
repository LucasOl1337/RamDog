use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::categories::{self, Category};
use crate::config::{Config, MemMetric};
use crate::hwtemp::HwTemp;
use crate::knowledge::{self, Risk};
use crate::metrics::{GpuInfo, SysSample};
use crate::procs::{self, KillOutcome, KernelMem, MemStatus, ProcInfo};
use crate::sampler::{self, SamplerHandle, Snapshot};
use crate::signature::{self, SigInfo, Trust};

const SAMPLE_TIMEOUT: Duration = Duration::from_secs(8);

struct Collector {
    sampler: SamplerHandle,
}

impl Collector {
    fn new() -> Self {
        procs::enable_debug_privilege();
        Self {
            sampler: sampler::spawn(egui::Context::default(), 500),
        }
    }

    fn snapshot(&mut self) -> Result<Snapshot, String> {
        self.sampler.force.store(true, Ordering::Relaxed);
        let mut snapshot = self
            .sampler
            .rx
            .recv_timeout(SAMPLE_TIMEOUT)
            .map_err(|e| format!("sample unavailable: {e}"))?;
        while let Ok(next) = self.sampler.rx.try_recv() {
            snapshot = next;
        }
        Ok(snapshot)
    }
}

pub fn run(args: Vec<String>) -> i32 {
    prepare_cli_stdio();
    let command = args.first().map(String::as_str).unwrap_or("help");
    let result = match command {
        "help" | "--help" | "-h" => {
            print_help();
            Ok(None)
        }
        "version" | "--version" | "-V" => {
            write_cli_line(&format!("ramdog {}", env!("CARGO_PKG_VERSION")), false);
            Ok(None)
        }
        "snapshot" | "--json" => command_snapshot(&args[1..]),
        "inspect" => command_inspect(&args[1..]),
        "tree" => command_tree(&args[1..]),
        "ai-sessions" | "sessions" => command_sessions(&args[1..]),
        "startup" => command_startup(),
        "drains" => command_drains(),
        "terminate" => command_terminate(&args[1..]),
        "mcp" | "--mcp" => {
            return match serve_mcp() {
                Ok(()) => 0,
                Err(e) => {
                    write_cli_line(&format!("ramdog mcp: {e}"), true);
                    1
                }
            };
        }
        _ => Err(format!("unknown command: {command}")),
    };

    match result {
        Ok(Some(value)) => {
            let pretty = args.iter().any(|a| a == "--pretty");
            let text = if pretty {
                serde_json::to_string_pretty(&value)
            } else {
                serde_json::to_string(&value)
            };
            match text {
                Ok(text) => {
                    write_cli_line(&text, false);
                    0
                }
                Err(e) => {
                    write_cli_line(&format!("ramdog: {e}"), true);
                    1
                }
            }
        }
        Ok(None) => 0,
        Err(e) => {
            write_cli_line(&format!("ramdog: {e}\nRun `ramdog help` for usage."), true);
            2
        }
    }
}

#[cfg(windows)]
fn prepare_cli_stdio() {
    use windows::Win32::System::Console::{
        AttachConsole, GetConsoleMode, GetStdHandle, ATTACH_PARENT_PROCESS, CONSOLE_MODE,
        STD_OUTPUT_HANDLE,
    };

    unsafe {
        let Ok(handle) = GetStdHandle(STD_OUTPUT_HANDLE) else {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
            return;
        };
        let mut mode = CONSOLE_MODE(0);
        if GetConsoleMode(handle, &mut mode as *mut _).is_err()
            && (handle.0.is_null() || handle.is_invalid())
        {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

#[cfg(not(windows))]
fn prepare_cli_stdio() {}

fn write_cli_line(text: &str, error: bool) {
    #[cfg(windows)]
    {
        use windows::core::w;
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_WRITE, FILE_SHARE_READ,
            FILE_SHARE_WRITE, OPEN_EXISTING,
        };
        use windows::Win32::System::Console::{
            AttachConsole, GetConsoleMode, GetStdHandle, WriteConsoleW, ATTACH_PARENT_PROCESS,
            CONSOLE_MODE, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
        };

        let std_handle = if error {
            STD_ERROR_HANDLE
        } else {
            STD_OUTPUT_HANDLE
        };
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        wide.push(b'\n' as u16);
        unsafe {
            let mut handle = GetStdHandle(std_handle).ok();
            let usable = handle
                .as_ref()
                .map(|h| !h.0.is_null() && !h.is_invalid())
                .unwrap_or(false);
            if !usable {
                let _ = AttachConsole(ATTACH_PARENT_PROCESS);
                handle = GetStdHandle(std_handle).ok();
            }
            if let Some(handle) = handle {
                let mut mode = CONSOLE_MODE(0);
                if GetConsoleMode(handle, &mut mode as *mut _).is_ok() {
                    let _ = WriteConsoleW(handle, &wide, None, None);
                    return;
                }
                if !handle.0.is_null() && !handle.is_invalid() {
                    if error {
                        let mut output = io::stderr();
                        let _ = output.write_all(text.as_bytes());
                        let _ = output.write_all(b"\n");
                        let _ = output.flush();
                    } else {
                        let mut output = io::stdout();
                        let _ = output.write_all(text.as_bytes());
                        let _ = output.write_all(b"\n");
                        let _ = output.flush();
                    }
                    return;
                }
            }
            if !error {
                if let Ok(handle) = CreateFileW(
                    w!("CONOUT$"),
                    FILE_GENERIC_WRITE.0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    None,
                ) {
                    let _ = WriteConsoleW(handle, &wide, None, None);
                    let _ = CloseHandle(handle);
                }
            }
        }
        return;
    }

    #[cfg(not(windows))]
    {
        if error {
            let mut output = io::stderr();
            let _ = writeln!(output, "{text}");
            let _ = output.flush();
        } else {
            let mut output = io::stdout();
            let _ = writeln!(output, "{text}");
            let _ = output.flush();
        }
    }
}

fn print_help() {
    write_cli_line(
        "ramdog — process and system data for humans and agents\n\n\
         Usage:\n\
           ramdog                         Open the GUI\n\
           ramdog snapshot [--pretty]     Full JSON snapshot\n\
           ramdog inspect --pid PID       Process, signature, and knowledge\n\
           ramdog tree --pid PID          Process tree as JSON\n\
           ramdog ai-sessions             AI agent sessions as JSON\n\
           ramdog startup                 Startup entries as JSON\n\
           ramdog drains                  Defender/services/Appx as JSON\n\
           ramdog terminate --pid PID --create-time FILETIME --yes [--tree]\n\
           ramdog mcp                     Local MCP server over stdio\n\n\
         Read-only commands are safe for agents. Termination requires --yes and the\n\
         process creation time from a fresh snapshot to prevent PID reuse.\n\
         Add --full-command when a command line must be returned without redaction.",
        false,
    );
}

fn command_snapshot(args: &[String]) -> Result<Option<Value>, String> {
    let mut collector = Collector::new();
    let snapshot = collector.snapshot()?;
    let config = Config::load();
    Ok(Some(snapshot_value(
        &snapshot,
        &config,
        has_flag(args, "--full-command"),
    )))
}

fn command_inspect(args: &[String]) -> Result<Option<Value>, String> {
    let pid = required_u32(args, "--pid")?;
    let mut collector = Collector::new();
    let snapshot = collector.snapshot()?;
    let config = Config::load();
    let process = snapshot
        .procs
        .iter()
        .find(|p| p.pid == pid)
        .ok_or_else(|| format!("PID {pid} was not found in the snapshot"))?;
    Ok(Some(inspect_value(
        &snapshot,
        &config,
        process,
        has_flag(args, "--full-command"),
    )))
}

fn command_tree(args: &[String]) -> Result<Option<Value>, String> {
    let pid = required_u32(args, "--pid")?;
    let mut collector = Collector::new();
    let snapshot = collector.snapshot()?;
    let config = Config::load();
    Ok(Some(tree_value(
        &snapshot,
        &config,
        pid,
        has_flag(args, "--full-command"),
    )?))
}

fn command_sessions(_args: &[String]) -> Result<Option<Value>, String> {
    let mut collector = Collector::new();
    let snapshot = collector.snapshot()?;
    let config = Config::load();
    let overrides: HashMap<String, Category> = config.overrides.clone().into_iter().collect();
    let categories = categories::classify(&snapshot.procs, &overrides);
    Ok(Some(Value::Array(ai_sessions(&snapshot.procs, &categories))))
}

fn command_startup() -> Result<Option<Value>, String> {
    #[cfg(not(windows))]
    return Ok(Some(json!({ "supported": false, "reason": "Startup inventory is Windows-only" })));
    #[cfg(windows)]
    {
    let mut startup = crate::boot::Boot::new();
    Ok(Some(startup.snapshot_json()))
    }
}

fn command_drains() -> Result<Option<Value>, String> {
    #[cfg(not(windows))]
    return Ok(Some(json!({ "supported": false, "reason": "Defender, services, and Appx are Windows-only" })));
    #[cfg(windows)]
    {
    let mut drains = crate::drains::Drains::new();
    Ok(Some(drains.snapshot_json()))
    }
}

fn command_terminate(args: &[String]) -> Result<Option<Value>, String> {
    if !has_flag(args, "--yes") {
        return Err("termination is disabled without --yes".into());
    }
    let pid = required_u32(args, "--pid")?;
    let create_time = required_i64(args, "--create-time")?;
    let mut collector = Collector::new();
    let snapshot = collector.snapshot()?;
    let result = terminate_snapshot(
        &snapshot,
        pid,
        create_time,
        has_flag(args, "--tree"),
        has_flag(args, "--force-critical"),
        true,
    )?;
    Ok(Some(result))
}

fn required_u32(args: &[String], name: &str) -> Result<u32, String> {
    option(args, name)
        .ok_or_else(|| format!("missing {name}"))?
        .parse()
        .map_err(|_| format!("invalid {name}"))
}

fn required_i64(args: &[String], name: &str) -> Result<i64, String> {
    option(args, name)
        .ok_or_else(|| format!("missing {name}"))?
        .parse()
        .map_err(|_| format!("invalid {name}"))
}

fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].as_str())
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

fn snapshot_value(snapshot: &Snapshot, config: &Config, full_command: bool) -> Value {
    let overrides: HashMap<String, Category> = config.overrides.clone().into_iter().collect();
    let categories = categories::classify(&snapshot.procs, &overrides);
    let (subtrees, counts) = subtree_totals(&snapshot.procs, config.mem_metric);
    let mut cat_counts: HashMap<Category, usize> = HashMap::new();
    let processes: Vec<Value> = snapshot
        .procs
        .iter()
        .map(|p| {
            let category = categories.get(&p.pid).copied().unwrap_or(Category::Other);
            *cat_counts.entry(category).or_insert(0) += 1;
            process_value(
                p,
                category,
                subtrees.get(&p.pid).copied(),
                counts.get(&p.pid).copied(),
                full_command,
            )
        })
        .collect();
    let category_counts = Category::ALL
        .iter()
        .map(|category| {
            (category.label().to_string(), json!(cat_counts.get(category).unwrap_or(&0)))
        })
        .collect::<Map<String, Value>>();

    json!({
        "schema": 1,
        "timestamp_unix_ms": now_unix_ms(),
        "platform": std::env::consts::OS,
        "ramdog_version": env!("CARGO_PKG_VERSION"),
        "is_admin": procs::is_admin(),
        "sample_ms": snapshot.sample_ms,
        "process_count": snapshot.procs.len(),
        "category_counts": category_counts,
        "memory": memory_value(&snapshot.mem),
        "kernel_memory": kernel_value(&snapshot.kernel),
        "system": system_value(&snapshot.sys),
        "thermal": thermal_value(&snapshot.hwtemp),
        "ai_sessions": ai_sessions(&snapshot.procs, &categories),
        "processes": processes,
    })
}

fn subtree_totals(procs: &[ProcInfo], metric: MemMetric) -> (HashMap<u32, u64>, HashMap<u32, usize>) {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    let by_pid: HashMap<u32, &ProcInfo> = procs.iter().map(|p| (p.pid, p)).collect();
    for process in procs {
        children.entry(process.ppid).or_default().push(process.pid);
    }

    fn bytes(process: &ProcInfo, metric: MemMetric) -> u64 {
        match metric {
            #[cfg(target_os = "linux")]
            MemMetric::Proportional => process.linux_memory.map(|m| m.1).unwrap_or(0),
            MemMetric::WorkingSet => process.working_set,
            MemMetric::Private => process.private_ws,
            MemMetric::Commit => process.commit,
        }
    }

    fn visit(
        pid: u32,
        by_pid: &HashMap<u32, &ProcInfo>,
        children: &HashMap<u32, Vec<u32>>,
        metric: MemMetric,
        seen: &mut HashSet<u32>,
    ) -> (u64, usize) {
        if !seen.insert(pid) {
            return (0, 0);
        }
        let Some(process) = by_pid.get(&pid) else { return (0, 0) };
        let mut total = bytes(process, metric);
        let mut count = 1;
        for child in children.get(&pid).into_iter().flatten() {
            let (child_total, child_count) = visit(*child, by_pid, children, metric, seen);
            total = total.saturating_add(child_total);
            count += child_count;
        }
        (total, count)
    }

    let mut totals = HashMap::new();
    let mut counts = HashMap::new();
    for process in procs {
        let mut seen = HashSet::new();
        let (total, count) = visit(process.pid, &by_pid, &children, metric, &mut seen);
        totals.insert(process.pid, total);
        counts.insert(process.pid, count);
    }
    (totals, counts)
}

fn process_value(
    p: &ProcInfo,
    category: Category,
    subtree_bytes: Option<u64>,
    subtree_count: Option<usize>,
    full_command: bool,
) -> Value {
    let launcher = &p.launcher;
    json!({
        "pid": p.pid,
        "parent_pid": p.ppid,
        "raw_parent_pid": p.raw_ppid,
        "name": p.name,
        "path": p.exe_path,
        "command": if full_command { p.cmdline.clone() } else { redact_command(&p.cmdline) },
        "created_filetime": p.create_time,
        "session": p.session,
        "threads": p.threads,
        "handles": p.handles,
        "cpu_pct": p.cpu_pct,
        "gpu_pct": p.gpu_pct,
        "disk_bytes_per_sec": p.disk_bps,
        "memory": {
            "private_bytes": p.private_ws,
            "working_set_bytes": p.working_set,
            "commit_bytes": p.commit,
            "subtree_bytes": subtree_bytes,
            "subtree_processes": subtree_count,
        },
        "category": category.label(),
        "launcher": {
            "agent": launcher.agent,
            "agent_pid": launcher.agent_pid,
            "session": launcher.session,
            "host": launcher.host,
            "project": launcher.init_cwd,
            "script": launcher.npm_script,
        },
    })
}

fn inspect_value(
    snapshot: &Snapshot,
    config: &Config,
    process: &ProcInfo,
    full_command: bool,
) -> Value {
    let overrides: HashMap<String, Category> = config.overrides.clone().into_iter().collect();
    let categories = categories::classify(&snapshot.procs, &overrides);
    let (subtrees, counts) = subtree_totals(&snapshot.procs, config.mem_metric);
    let category = categories
        .get(&process.pid)
        .copied()
        .unwrap_or(Category::Other);
    let mut value = process_value(
        process,
        category,
        subtrees.get(&process.pid).copied(),
        counts.get(&process.pid).copied(),
        full_command,
    );
    if let Value::Object(object) = &mut value {
        let signature = signature::verify(&process.exe_path);
        object.insert("signature".into(), signature_value(&signature));
        object.insert("knowledge".into(), knowledge_value(&process.name_lower));
        object.insert(
            "children".into(),
            json!(snapshot
                .procs
                .iter()
                .filter(|p| p.ppid == process.pid)
                .map(|p| p.pid)
                .collect::<Vec<_>>()),
        );
    }
    value
}

fn tree_value(
    snapshot: &Snapshot,
    config: &Config,
    root_pid: u32,
    full_command: bool,
) -> Result<Value, String> {
    let tree = process_tree(&snapshot.procs, root_pid)?;
    let overrides: HashMap<String, Category> = config.overrides.clone().into_iter().collect();
    let categories = categories::classify(&snapshot.procs, &overrides);
    let (subtrees, counts) = subtree_totals(&snapshot.procs, config.mem_metric);
    let processes = tree
        .iter()
        .map(|(p, depth)| {
            let category = categories.get(&p.pid).copied().unwrap_or(Category::Other);
            let mut value = process_value(
                p,
                category,
                subtrees.get(&p.pid).copied(),
                counts.get(&p.pid).copied(),
                full_command,
            );
            if let Value::Object(object) = &mut value {
                object.insert("depth".into(), json!(depth));
            }
            value
        })
        .collect::<Vec<_>>();
    Ok(json!({ "root_pid": root_pid, "count": processes.len(), "processes": processes }))
}

fn memory_value(memory: &MemStatus) -> Value {
    json!({
        "total_physical_bytes": memory.total_phys,
        "available_physical_bytes": memory.avail_phys,
        "used_physical_bytes": memory.used_phys(),
        "total_commit_bytes": memory.total_commit,
        "available_commit_bytes": memory.avail_commit,
        "used_commit_bytes": memory.used_commit(),
    })
}

fn kernel_value(kernel: &KernelMem) -> Value {
    json!({ "available": kernel.ok, "paged_pool_bytes": kernel.paged_pool, "nonpaged_pool_bytes": kernel.nonpaged_pool })
}

fn system_value(system: &SysSample) -> Value {
    let gpu_by_pid = system
        .gpu_by_pid
        .iter()
        .map(|(pid, value)| (pid.to_string(), json!(value)))
        .collect::<Map<String, Value>>();
    json!({
        "cpu_pct": system.cpu_pct,
        "disk_pct": system.disk_pct,
        "disk_bytes_per_sec": system.disk_bps,
        "gpu": system.gpu.as_ref().map(gpu_value).unwrap_or(Value::Null),
        "gpu_by_pid_pct": gpu_by_pid,
    })
}

fn gpu_value(gpu: &GpuInfo) -> Value {
    json!({
        "name": gpu.name,
        "util_pct": gpu.util_pct,
        "temperature_c": gpu.temp_c,
        "memory_used_bytes": gpu.mem_used,
        "memory_total_bytes": gpu.mem_total,
        "power_w": gpu.power_w,
        "fan_pct": gpu.fan_pct,
    })
}

fn thermal_value(thermal: &HwTemp) -> Value {
    json!({
        "cpu_temperature_c": thermal.cpu_temp,
        "dimm_temperatures_c": thermal.dimm_temps,
        "stabilize": { "on": thermal.stab.on, "held_fan_pct": thermal.stab.held },
        "sensors": thermal.sensors.iter().map(|s| json!({ "hardware": s.hw, "name": s.name, "kind": s.kind, "value": s.value })).collect::<Vec<_>>(),
        "fans": thermal.fans.iter().map(|f| json!({ "name": f.name, "pct": f.pct, "rpm": f.rpm, "auto": f.auto, "thermal_guard": f.guard })).collect::<Vec<_>>(),
    })
}

#[derive(Default)]
struct SessionSummary {
    agent: Option<String>,
    session: Option<String>,
    host: Option<String>,
    project: Option<String>,
    pids: Vec<u32>,
    working_set_bytes: u64,
    private_bytes: u64,
    cpu_pct_sum: f32,
    gpu_pct_sum: f32,
}

fn ai_sessions(procs: &[ProcInfo], categories: &HashMap<u32, Category>) -> Vec<Value> {
    let mut groups: BTreeMap<String, SessionSummary> = BTreeMap::new();
    for p in procs {
        let category = categories.get(&p.pid).copied().unwrap_or(Category::Other);
        if p.launcher.agent.is_none() && category != Category::Ai {
            continue;
        }
        let agent = p.launcher.agent.clone().or_else(|| Some(p.name.clone()));
        let key = format!(
            "{}|{}|{}|{}",
            agent.as_deref().unwrap_or_default(),
            p.launcher.session.as_deref().unwrap_or_default(),
            p.launcher.host.as_deref().unwrap_or_default(),
            p.launcher.init_cwd.as_deref().unwrap_or_default()
        );
        let group = groups.entry(key).or_default();
        group.agent = agent;
        group.session = group.session.clone().or_else(|| p.launcher.session.clone());
        group.host = group.host.clone().or_else(|| p.launcher.host.clone());
        group.project = group
            .project
            .clone()
            .or_else(|| p.launcher.init_cwd.clone());
        group.pids.push(p.pid);
        group.working_set_bytes = group.working_set_bytes.saturating_add(p.working_set);
        group.private_bytes = group.private_bytes.saturating_add(p.private_ws);
        group.cpu_pct_sum += p.cpu_pct;
        group.gpu_pct_sum += p.gpu_pct;
    }
    groups
        .into_values()
        .map(|g| {
            json!({
                "agent": g.agent,
                "session": g.session,
                "host": g.host,
                "project": g.project,
                "pids": g.pids,
                "process_count": g.pids.len(),
                "working_set_bytes": g.working_set_bytes,
                "private_bytes": g.private_bytes,
                "cpu_pct_sum": g.cpu_pct_sum,
                "gpu_pct_sum": g.gpu_pct_sum,
            })
        })
        .collect()
}



fn process_tree<'a>(
    procs: &'a [ProcInfo],
    root_pid: u32,
) -> Result<Vec<(&'a ProcInfo, usize)>, String> {
    if !procs.iter().any(|p| p.pid == root_pid) {
        return Err(format!("PID {root_pid} was not found in the snapshot"));
    }
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    let by_pid: HashMap<u32, &ProcInfo> = procs.iter().map(|p| (p.pid, p)).collect();
    for p in procs {
        children.entry(p.ppid).or_default().push(p.pid);
    }
    let mut result = Vec::new();
    let mut stack = vec![(root_pid, 0usize)];
    let mut seen = HashSet::new();
    while let Some((pid, depth)) = stack.pop() {
        if !seen.insert(pid) {
            continue;
        }
        if let Some(process) = by_pid.get(&pid) {
            result.push((*process, depth));
            for child in children.get(&pid).into_iter().flatten().rev() {
                stack.push((*child, depth + 1));
            }
        }
    }
    result.sort_by_key(|(_, depth)| *depth);
    Ok(result)
}

fn terminate_snapshot(
    snapshot: &Snapshot,
    pid: u32,
    create_time: i64,
    tree: bool,
    force_critical: bool,
    confirm: bool,
) -> Result<Value, String> {
    if !confirm {
        return Err("termination requires explicit confirmation".into());
    }
    if pid == std::process::id() {
        return Err("refusing to terminate RamDog itself".into());
    }
    let root = snapshot
        .procs
        .iter()
        .find(|p| p.pid == pid)
        .ok_or_else(|| format!("PID {pid} was not found in the snapshot"))?;
    if root.create_time != create_time {
        return Err(format!(
            "PID {pid} changed since the snapshot; inspect it again"
        ));
    }
    let mut targets = if tree {
        process_tree(&snapshot.procs, pid)?
    } else {
        vec![(root, 0)]
    };
    if !force_critical {
        if let Some(critical) = targets.iter().find(|(p, _)| is_fatal(p)) {
            return Err(format!(
                "refusing to terminate critical process {} ({})",
                critical.0.name, critical.0.pid
            ));
        }
    }
    targets.sort_by_key(|(_, depth)| std::cmp::Reverse(*depth));
    let mut killed = Vec::new();
    let mut errors = Vec::new();
    for (process, _) in targets {
        match procs::kill(process.pid) {
            KillOutcome::Signaled | KillOutcome::AlreadyGone => killed.push(process.pid),
            KillOutcome::Denied => errors.push(json!({
                "pid": process.pid,
                "name": process.name,
                "error": "access denied"
            })),
            KillOutcome::Invalid => errors.push(json!({
                "pid": process.pid,
                "name": process.name,
                "error": "invalid process"
            })),
            KillOutcome::Failed(error) => {
                errors.push(json!({ "pid": process.pid, "name": process.name, "error": error }))
            }
        }
    }
    Ok(json!({ "ok": errors.is_empty(), "tree": tree, "killed": killed, "errors": errors }))
}

fn is_fatal(process: &ProcInfo) -> bool {
    categories::is_critical(&process.name_lower, process.pid)
        || knowledge::lookup(&process.name_lower)
            .map(|known| known.risk == Risk::Fatal)
            .unwrap_or(false)
}

fn signature_value(info: &SigInfo) -> Value {
    let trust = match &info.trust {
        #[cfg(target_os = "linux")]
        Trust::Package { verified, .. } => {
            if *verified { "package" } else { "package-unverified" }
        }
        Trust::Valid => "valid",
        Trust::Unsigned => "unsigned",
        Trust::Invalid(_) => "invalid",
        Trust::Unknown(_) => "unknown",
    };
    json!({ "trust": trust, "label": info.label(), "signer": info.signer })
}

fn knowledge_value(name_lower: &str) -> Value {
    knowledge::lookup(name_lower)
        .map(|known| json!({ "what": known.what, "why": known.why, "risk": known.risk.label(), "risk_tip": known.risk.tip() }))
        .unwrap_or(Value::Null)
}

fn redact_command(command: &str) -> String {
    let mut output = Vec::new();
    let mut redact_next = false;
    for token in command.split_whitespace() {
        if redact_next {
            output.push("<redacted>".to_string());
            redact_next = false;
            continue;
        }
        let lower = token.to_ascii_lowercase();
        if [
            "--api-key",
            "--token",
            "--password",
            "--secret",
            "--access-token",
            "-apikey",
            "-token",
        ]
        .contains(&lower.as_str())
        {
            output.push(token.to_string());
            redact_next = true;
        } else if let Some((key, _)) = token.split_once('=') {
            if [
                "api_key",
                "apikey",
                "token",
                "password",
                "secret",
                "access_token",
                "authorization",
            ]
            .contains(&key.trim_start_matches('-').to_ascii_lowercase().as_str())
            {
                output.push(format!("{key}=<redacted>"));
            } else {
                output.push(token.to_string());
            }
        } else {
            output.push(token.to_string());
        }
    }
    output.join(" ")
}

fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

// ---------- MCP over stdio ----------

fn serve_mcp() -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = BufWriter::new(stdout.lock());
    let mut collector = Collector::new();
    loop {
        let Some(message) = read_mcp_message(&mut reader)? else {
            break;
        };
        let should_exit = message.get("method").and_then(Value::as_str) == Some("exit");
        if let Some(response) = handle_mcp_message(&mut collector, &message) {
            write_mcp_message(&mut writer, &response)?;
        }
        if should_exit {
            break;
        }
    }
    Ok(())
}

fn read_mcp_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = Some(
                    value
                        .trim()
                        .parse::<usize>()
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
                );
            }
        }
    }
    let Some(length) = content_length else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "missing Content-Length",
        ));
    };
    let mut bytes = vec![0u8; length];
    reader.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn write_mcp_message(writer: &mut impl Write, value: &Value) -> io::Result<()> {
    let bytes =
        serde_json::to_vec(value).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write!(writer, "Content-Length: {}\r\n\r\n", bytes.len())?;
    writer.write_all(&bytes)?;
    writer.flush()
}

fn handle_mcp_message(collector: &mut Collector, message: &Value) -> Option<Value> {
    let method = message.get("method").and_then(Value::as_str)?;
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    if method.starts_with("notifications/") {
        return None;
    }
    let response = match method {
        "initialize" => rpc_result(
            id,
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "ramdog", "version": env!("CARGO_PKG_VERSION") }
            }),
        ),
        "ping" => rpc_result(id, json!({})),
        "tools/list" => rpc_result(id, tool_list()),
        "resources/list" => rpc_result(id, json!({ "resources": [] })),
        "prompts/list" => rpc_result(id, json!({ "prompts": [] })),
        "tools/call" => {
            let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
            match call_tool(collector, &params) {
                Ok(value) => rpc_result(id, tool_result(value, false)),
                Err(error) => rpc_result(id, tool_result(json!({ "error": error }), true)),
            }
        }
        _ => rpc_error(id, -32601, format!("method not found: {method}")),
    };
    Some(response)
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: Value, code: i32, message: String) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn tool_list() -> Value {
    json!({
        "tools": [
            {
                "name": "ramdog_snapshot",
                "description": "Read the current RamDog process, memory, CPU, GPU, disk, thermal, category, and AI-session snapshot.",
                "inputSchema": { "type": "object", "properties": { "full_command": { "type": "boolean", "description": "Return command lines without common-secret redaction." } } }
            },
            {
                "name": "ramdog_inspect_process",
                "description": "Inspect one process, including launcher/session, memory, signature, knowledge, and children.",
                "inputSchema": { "type": "object", "required": ["pid"], "properties": { "pid": { "type": "integer" }, "full_command": { "type": "boolean" } } }
            },
            {
                "name": "ramdog_process_tree",
                "description": "Return a process and all descendants with memory totals.",
                "inputSchema": { "type": "object", "required": ["pid"], "properties": { "pid": { "type": "integer" }, "full_command": { "type": "boolean" } } }
            },
            {
                "name": "ramdog_ai_sessions",
                "description": "Group active AI-agent processes by agent, session, host, and project.",
                "inputSchema": { "type": "object", "properties": {} }
            },
            {
                "name": "ramdog_startup",
                "description": "Read startup entries from the Windows registry, startup folder, tasks, services, UWP, and Winlogon.",
                "inputSchema": { "type": "object", "properties": {} }
            },
            {
                "name": "ramdog_drains",
                "description": "Read Windows Defender, optional service, protected service, and system Appx status.",
                "inputSchema": { "type": "object", "properties": {} }
            },
            {
                "name": "ramdog_terminate_process",
                "description": "Terminate a process or tree only after explicit confirmation and creation-time validation. Never use for arbitrary shell execution.",
                "inputSchema": { "type": "object", "required": ["pid", "create_time", "confirm"], "properties": { "pid": { "type": "integer" }, "create_time": { "type": "integer" }, "tree": { "type": "boolean" }, "confirm": { "type": "boolean" } } }
            }
        ]
    })
}

fn call_tool(collector: &mut Collector, params: &Value) -> Result<Value, String> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or("tools/call missing name")?;
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let config = Config::load();
    match name {
        "ramdog_snapshot" => {
            let snapshot = collector.snapshot()?;
            Ok(snapshot_value(
                &snapshot,
                &config,
                args.get("full_command")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            ))
        }
        "ramdog_inspect_process" => {
            let pid = json_u32(&args, "pid")?;
            let snapshot = collector.snapshot()?;
            let process = snapshot
                .procs
                .iter()
                .find(|p| p.pid == pid)
                .ok_or_else(|| format!("PID {pid} was not found"))?;
            Ok(inspect_value(
                &snapshot,
                &config,
                process,
                args.get("full_command")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            ))
        }
        "ramdog_process_tree" => {
            let pid = json_u32(&args, "pid")?;
            let snapshot = collector.snapshot()?;
            tree_value(
                &snapshot,
                &config,
                pid,
                args.get("full_command")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            )
        }
        "ramdog_ai_sessions" => {
            let snapshot = collector.snapshot()?;
            let value = snapshot_value(&snapshot, &config, false);
            Ok(value["ai_sessions"].clone())
        }
        "ramdog_startup" => {
            let mut startup = crate::boot::Boot::new();
            Ok(startup.snapshot_json())
        }
        "ramdog_drains" => {
            let mut drains = crate::drains::Drains::new();
            Ok(drains.snapshot_json())
        }
        "ramdog_terminate_process" => {
            if args.get("confirm").and_then(Value::as_bool) != Some(true) {
                return Err("confirm=true is required".into());
            }
            let pid = json_u32(&args, "pid")?;
            let create_time = args
                .get("create_time")
                .and_then(Value::as_i64)
                .ok_or("missing create_time")?;
            let snapshot = collector.snapshot()?;
            terminate_snapshot(
                &snapshot,
                pid,
                create_time,
                args.get("tree").and_then(Value::as_bool).unwrap_or(false),
                false,
                true,
            )
        }
        _ => Err(format!("unknown tool: {name}")),
    }
}

fn tool_result(value: Value, is_error: bool) -> Value {
    let text = serde_json::to_string(&value)
        .unwrap_or_else(|_| "{\"error\":\"serialization failed\"}".into());
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

fn json_u32(value: &Value, name: &str) -> Result<u32, String> {
    value
        .get(name)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| format!("missing or invalid {name}"))
}

#[cfg(test)]
mod tests {
    use super::redact_command;

    #[test]
    fn redacts_common_command_secrets() {
        assert_eq!(
            redact_command("agent --token abc123 --mode run api_key=xyz"),
            "agent --token <redacted> --mode run api_key=<redacted>"
        );
    }
}
