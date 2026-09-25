<p align="center">
  <img src="docs/media/banners/hero.png" alt="RamDog — process manager for Windows, Linux, macOS and Omarchy" width="100%">
</p>

<p align="center">
  <a href="https://lucasol1337.github.io/RamDog/">Site</a>
  ·
  <a href="https://lucasol1337.github.io/RamDog/guia.html">Complete guide</a>
  ·
  <a href="README.md">Português</a>
  ·
  <a href="docs/releases/v0.12.1.md">v0.12.1 patch notes</a>
  ·
  <a href="CHANGELOG.md">Changelog</a>
</p>

<p align="center">
  <img alt="v0.12.1" src="https://img.shields.io/badge/v0.12.1-stable-73d8ee?style=flat-square">
  <img alt="Omarchy" src="https://img.shields.io/badge/Omarchy-native%20support-9ECE6A?style=flat-square&logo=archlinux&logoColor=white">
  <img alt="Hyprland" src="https://img.shields.io/badge/Hyprland-native%20windows-7AA2F7?style=flat-square">
  <img alt="Windows x64" src="https://img.shields.io/badge/Windows-x64-0B3A4A?style=flat-square&logo=windows&logoColor=4FC3F7">
  <img alt="Linux" src="https://img.shields.io/badge/Linux-x86_64%20%7C%20aarch64-333333?style=flat-square&logo=linux&logoColor=white">
  <img alt="macOS" src="https://img.shields.io/badge/macOS-arm64%20%7C%20x86_64-111111?style=flat-square&logo=apple&logoColor=white">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-000000?style=flat-square&logo=rust&logoColor=white">
  <img alt="MIT" src="https://img.shields.io/badge/license-MIT-69F0AE?style=flat-square">
</p>

<p align="center">
  Process manager for Windows, Linux and macOS: origin, categories and tree termination.
  On Linux, the first-class target is <a href="https://omarchy.org/">Omarchy</a> — Arch with Hyprland.
</p>

<img src="docs/media/lista.png" alt="RamDog on Linux: sidebar with Processes, Tree, Categories and addons; CPU, Memory, GPU and Disk cards with graphs; category chips; table with icon, name, PID · category · origin and status chip per row." width="100%">

<p align="center"><sub>Processes on Linux — Omarchy / Hyprland, NVIDIA RTX 4070 Ti SUPER. Group by app enabled; Android Emulator appears as leftover usage.</sub></p>

## Why it exists

Task Manager does not show the process origin — who launched it. It does not classify AI, Dev or Browser work. It does not terminate the whole tree while protecting processes that must not fall. It does not explain Windows waste. On Linux, `htop` does not arrange Hyprland windows, talk to systemd or understand what an agent is.

RamDog exists for that.

## v0.12.1 update

**v0.12.1** is an internal maintenance patch: it separates the table cache and process queries from the interface and expands regression tests. Filters, sorting, updates under the pointer, and the Portuguese and English features from v0.12.0 are preserved. There are no new visual features or measured performance gains.

- Cache and process queries in modules that can be tested without opening a window.
- 22 table-model tests cover hover transitions, groups, search and sorting criteria.
- Linux suite: 101 tests passed, three opt-in integrations ignored.

Full notes: [v0.12.1](docs/releases/v0.12.1.md) · [v0.12.0](docs/releases/v0.12.0.md) · [changelog](CHANGELOG.md).

## Built for Omarchy

<p align="center">
  <img src="docs/media/banners/omarchy.png" alt="Built for Omarchy — Hyprland, UWSM and Quickshell protected; stable long-session sampler on Wayland" width="100%">
</p>

RamDog **supports Omarchy as its reference Linux desktop**. It is not a generic port that merely “also opens on Hyprland”: the session, windows and inventory were written against this stack.

- **Protected session.** Hyprland, UWSM and Quickshell cannot be terminated by RamDog. They are classified as System.
- **Native windows.** Monitor map, grids, scenes and return to tiling through `hyprctl` and the Hyprland 0.55+ Lua API. Compatibility messages name Omarchy directly.
- **Wayland with NVIDIA.** eframe's explicit VSync is disabled on Linux — it could stall the event loop when hiding a window with EGL/Wayland.
- **Real Arch integration.** Icons come from `.desktop` files; package origin and SHA-256 are checked against the local pacman mtree (comparison with the local database, not a signature).
- **User systemd.** Startup and waste views read services, sockets and timers. `ramdog-launch` starts the app in a transient unit independent of the terminal.
- **Machine GPU.** NVIDIA through `nvidia-smi`; AMD/Intel through DRM/hwmon. The reference validation used an RTX 4070 Ti SUPER plus integrated AMD graphics in the same Omarchy session.

Other Linux desktops still get processes, categories, origin, kill, GPU, Startup/Waste and Thermal. Windows is required for Windows-only features, and the Windows layout view requires Hyprland on Linux.

Details, dependencies and limits: [linux/README.md](linux/README.md).

## What it does

### Language / Idioma

Existing installs start in Portuguese. Open **Preferences / Preferências**, choose **English** under **Language / Idioma**, and the choice is saved in the RamDog config. It applies to navigation, metrics, startup, screens and addon entry points without changing existing settings. For the Portuguese documentation, see [README.md](README.md).

- **Origin / launched by.** The Origin column shows the first living ancestor that is not a generic host (`cmd`, `bash`, `node`, …). When the parent chain has exited, RamDog reads inherited environment variables and shows the agent (Claude Code + session + PID, Codex, Cursor Agent, Gemini CLI, Hermes, …), host (Maestri, VS Code, Cursor, Windows Terminal, …) and `npm run <script>` project in purple.
- **Launched by.** The last column shows the complete launcher chain (`terminal › shell › agent`) instead of dumping command arguments. Right-click its header to switch back to **Command**. Clicking a parent selects it.
- **Contention.** When a game is open, RamDog highlights CPU-heavy processes, high load or swap, software GPU rendering, headless Android emulators, and credential helpers stuck in a loop. The CPU column can show equivalent saturated cores alongside machine percentage.
- **Honest zombie handling.** A zombie is already dead and holds no memory. RamDog asks its live parent to reap it, names the process holding the entry, and offers to terminate that parent when appropriate instead of claiming the zombie itself was killed.
- **Categories.** AI / Agents, Dev, Browser, Games, Personal, System and Other — automatic rules with a manual per-process override.
- **Kill, tree and lock.** Terminate a process or its tree (process plus children). A lock prevents RamDog from terminating the protected process. Critical OS processes (`System`/`csrss`/`dwm` on Windows; `systemd`/`kthreadd`/`Hyprland`/`UWSM`/`Quickshell`/`gnome-shell`/`Xorg` on Linux; `kernel_task`/`launchd`/`WindowServer` on macOS) are always protected.
- **Views.** List (flat), Tree (parent → children, subtree RAM) and Categories (grouped) sit beside search. The **Sweep**, **Startup**, **Waste**, **Thermal**, **Screens** and **Cleanup** addons live in the upper-right controls; clicking one swaps the window content, and clicking it again returns to the process view. On Linux, Startup and Waste use systemd/XDG, Screens uses Hyprland, Thermal uses hwmon with authenticated PWM control, Sweep sorts open-but-unused apps into can close / maybe / in use and closes whatever you select in bulk, and Cleanup combines kernel cache and zombies with disk locations (`~/.cache`, trash, pacman, journal, coredumps and orphans). See the [Linux port](linux/README.md).
- **Group by app.** In List view, recognized families (Claude, Codex, Grok, ChatGPT, Cursor, Gemini, Hermes, Maestri, OpenCode) and processes with the same executable become one row — `Claude (12)`, `chromium (66)` — summing RAM, CPU, GPU and disk in the header. The **✖** action terminates the whole app. Groups start collapsed; click to see their PIDs. Family keys follow versioned installs (`mise`, PATH, the ChatGPT folder) and the agent-launched `node`. Outside those families, the key is the executable path rather than its name, so two `worker` processes from different folders never merge; on Unix, case differences matter. A single-process app is not grouped.
- **Readable CPU column.** On Windows, process allocation uses `CycleTime` (counted at context switches), not kernel/user time — Windows charges that in 15.625 ms slices and assigns the whole slice to whoever ran at the tick, making bursty processes flicker between 0% and 15%. The total comes from `GetSystemTimes`, so a busy machine does not dilute the culprit. Linux compares `/proc/*/stat` against the machine's online cores, not RamDog's own affinity. Both use a time-bound 1-second moving average so the list stays still long enough to read; the raw last-interval value remains in the tooltip.
- **Meters.** CPU and RAM are shown at the top on all three operating systems. Windows uses NVML for NVIDIA; Linux uses `nvidia-smi` plus DRM/hwmon for NVIDIA/AMD/Intel. Disk percentage is PDH on Windows and `/proc/diskstats` on Linux; it is unavailable on macOS.
- **Thermal.** On Windows, embedded [TempHUD](https://github.com/LucasOl1337/TempHUD) provides CPU/GPU/RAM/motherboard sensors, SuperIO fan control (manual percentage or Auto/BIOS) and **STABILIZE** — fans locked to 50% up to 80 °C, a linear ramp to 100% at 92 °C and an immediate ceiling at 95 °C. The `hwtemp.exe` helper restores BIOS control if RamDog exits. Fans require admin; without them, the view still shows available readings. Linux reads `/sys/class/hwmon` and offers PWM/STABILIZE through compatible authenticated helpers that restore the prior state when they exit.
- **Startup.** Everything that starts with the PC, not just Task Manager's subset: `Run` and `RunOnce` (HKCU, HKLM and Wow64), the complete Startup folder (`.lnk`, `.vbs`, `.cmd`), scheduled boot/logon tasks, automatic services, UWP apps, Winlogon and Active Setup. The outer filter is **starts with the PC / does not start / broken**, with three clickable counters. Inside each block, startup phase runs from the surface inward: your programs, at sign-in, with the machine, before Windows. Each band shows entry and currently-running counts and collapses on click. The startup check answers “starts with the PC”; the **Now** column answers “has a process running”. The inner grouping can be changed to source type, phase, type or flat list. Linux reads systemd (user and system) and XDG autostart.
- **Screens.** The monitor map is drawn to scale: drag a window between monitors and drop it into a grid zone (halves, thirds, quadrants, primary+2, …). **Distribute** spreads everything on one monitor across the selected grid. **Scenes** save the arrangement as a fraction of the work area, not pixels, so presets survive resolution, scale and monitor changes; applying a scene moves open windows and launches missing ones when their windows appear. On Linux the backend is Hyprland (including Omarchy).
- **What is this, can I kill it?** The Windows details panel contains a catalog of 80 processes: what they do, why they are open and the risk of terminating them — 🟢 safe, 🟡 Windows restarts it, 🔴 it takes down the session.
- **Digital signature.** The signer comes from the certificate (`WinVerifyTrust`), not the file's `CompanyName`, which an impostor can fill with “Microsoft Corporation”. Verification is on demand for the selected process, outside the sampler. On Arch, the documented equivalent is pacman's SHA-256 comparison against the file on disk.
- **Mini mode.** The **◱ Mini** button turns the app into a borderless HUD with CPU, RAM, GPU and disk in a 2×2 layout, each with its temperature, plus fan RPM and **STABILIZE**. It stays above other windows (toggle **top**), can be dragged by its background, minimized, and restored with a double-click. The mode is remembered between sessions.

## Install

**Windows x64** (PowerShell):

```powershell
irm https://raw.githubusercontent.com/LucasOl1337/RamDog/main/install.ps1 | iex
```

Then run `ramdog` in a terminal or use the **RamDog** desktop shortcut.

RamDog **requests elevation when it opens** (one UAC prompt, whether started by shortcut, PATH or clicking the exe). This unlocks CPU/RAM temperature, terminating another user's service/process, and Startup/Waste actions without a prompt for every click. Users who cannot elevate are not blocked: RamDog opens in a limited mode instead of refusing to start (`highestAvailable`, not `requireAdministrator`).

**Linux** (x86_64 or aarch64, including Omarchy) and **macOS** (Apple Silicon or Intel):

```bash
curl -sSfL https://raw.githubusercontent.com/LucasOl1337/RamDog/main/install.sh | sh
```

The installer downloads the [release](https://github.com/LucasOl1337/RamDog/releases) (`RamDog-linux-x86_64.tar.gz`, `RamDog-linux-aarch64.tar.gz`, `RamDog-macos-aarch64.tar.gz` or `…-x86_64.tar.gz`) into `~/.local/bin/ramdog` and opens it. On macOS, if Gatekeeper blocks it: Settings → Privacy & Security → Open Anyway.

Linux provides list, categories, origin, tree, kill, USS/PSS/RSS, GPU, sensors and fans, Startup/Waste through systemd and Screens through Hyprland. v0.13 adds Sweep; v0.12 added the complete English interface; v0.11 added accurate CPU accounting, systemd-aware origins and contention diagnostics; v0.10 introduced the current interface and Cleanup. See the [patch notes](docs/releases/v0.13.0.md), [changelog](CHANGELOG.md), and [dependencies and limits](linux/README.md). The window works on Wayland or X11 (eframe). Release Linux binaries require glibc 2.39+ (Ubuntu 24.04, current Omarchy/Arch or a compatible distribution); older systems can compile from source. If no release binary is available, the script falls back to `cargo build` (requires [rustup](https://rustup.rs), git and native libraries listed below).

The installer verifies SHA-256 and, on Linux, also installs `ramdog-launch`, which keeps the app independent of the terminal through a user systemd unit when available. `RAMDOG_HOME` changes the destination, `RAMDOG_VERSION=v0.12.1` pins a version and `RAMDOG_NO_LAUNCH=1` installs without opening the app. Example: `curl -sSfL https://raw.githubusercontent.com/LucasOl1337/RamDog/main/install.sh | RAMDOG_NO_LAUNCH=1 sh`.

On macOS: list, categories, origin, tree and kill. No Waste, Screens, CPU temperature or NVML GPU support.

Prefer a manual download? Use the [release page](https://github.com/LucasOl1337/RamDog/releases) with the Windows zip or Linux/macOS tarballs.

From source:

```bash
git clone https://github.com/LucasOl1337/RamDog.git
cd RamDog
cargo build --release
```

On Linux (Debian/Ubuntu and derivatives), eframe with Wayland/X11 needs development libraries:

```bash
sudo apt install build-essential pkg-config libxkbcommon-dev libwayland-dev \
  libx11-dev libxcursor-dev libxi-dev libxrandr-dev libgl1-mesa-dev
```

On Omarchy / Arch:

```bash
sudo pacman -S --needed base-devel rust pkgconf libxkbcommon wayland libx11 \
  libxcursor libxi libxrandr mesa
```

On Windows, the elevation manifest is only included in the `release` profile — it applies to all crate targets, and a test binary with it would make `cargo test` fail with “the requested operation requires elevation” before running. `cargo test` (debug) runs normally; `cargo test --release` requires an elevated session.

Windows temperature helper (optional, [.NET 8 SDK](https://dotnet.microsoft.com/download/dotnet/8.0)):

```bash
dotnet publish hwtemp -c Release -o target/release --no-self-contained
```

## Usage

| Action | How |
|---|---|
| Terminate process | **✖** on the row, `Del`, right-click → Terminate, or the bottom panel |
| Terminate tree (process + children) | `Shift+Del`, `Shift`+**✖**, right-click → Terminate tree, or the bottom panel |
| Protect / unprotect | **🔒**/**🔓** on the row, right-click or the bottom panel. Protected processes are never terminated by RamDog |
| Manual category | right-click → Category, or the combo in the bottom panel (`auto` returns to automatic rules) |
| Views | **List**, **Tree**, **Categories** beside search; **Sweep**, **Startup**, **Waste**, **Thermal**, **Screens** and **Cleanup** in the upper-right control block — the lit button is the active view and clicking it returns to the last process view. Startup/Waste/Screens/Thermal are also implemented on Linux (systemd, Hyprland and hwmon) |
| Group by app | **Group by app** in List view; groups start collapsed, **▶**/**▼** reveal PIDs, **Expand all**/**Collapse** apply to the whole list, and **✖** in the header terminates every process in that app |
| Filter | search by name / PID / command; category chips (click toggles, double-click isolates); `hide below N MB` and the `RAM column shows` control on the right |
| Origin | *Origin* is the first living ancestor that is not a generic host (cmd, bash, node, …); the full chain is clickable in the bottom panel (**Go to parent**) |
| Launched by | when the parent chain has exited or contains only generic hosts, RamDog reads inherited environment variables and shows the originating agent (Claude Code + session + PID, Codex, Cursor Agent, Gemini CLI, Hermes, …), host (Maestri, VS Code, Cursor, Windows Terminal, …) and `npm run <script>` in the project |
| Refresh | **⏸ pause** and `every 0.5–5 s` in the footer beside the `sample X ms` value; `F5` forces a read |
| Mini mode | **◱ Mini** in the upper right. In the HUD, **top** toggles always-on-top, **–** minimizes, **⤢** (or double-clicking the background) restores the full app, **✕** closes; the interval button cycles 0.5 / 1 / 2 / 5 s; drag by the background |

Termination is immediate: there is no confirmation dialog. Protection comes from the **lock** (🔒 in the context menu) — a locked process survives `Del`, **✖** and tree termination. While the pointer is over the table, row order is **frozen** so **✖** never lands on a process that just moved.

**Startup** (Windows) lists what starts at boot and logon, with the source of each entry (`Run`, Startup folder, scheduled task, service, UWP, Winlogon, Active Setup), whether it is currently running and the executable's real path. Enable, disable and remove apply to the entry; the process itself remains terminable from List view. Machine entries (HKLM, services and tasks) require admin — without elevation, RamDog starts one elevated PowerShell action with **one UAC prompt per action**.

**Waste** (Windows) reads directly through the Service Control Manager and registry, without PowerShell. **Actions** use direct Win32 when RamDog is already elevated; otherwise they start one elevated PowerShell action with **one UAC prompt per action**. On macOS, the view exists only to explain that it is unavailable.

| Section | What it can do | Reversible? |
|---|---|---|
| **Microsoft Defender** | exclude project/agent folders from real-time scanning (`Add-MpPreference -ExclusionPath`); limit scheduled scan CPU to 5/10/20% (`-ScanAvgCPULoadFactor`); pause/re-enable real-time protection | Yes, all actions |
| **Optional services** | **Stop** (now only) or **Disable** (does not start again) — WSearch, SysMain, DiagTrack, DoSvc, WerSvc, MapsBroker, PhoneSvc, Xbox*, lfsvc, RemoteRegistry and Fax. `wuauserv` is **stop only** (Windows starts it again) | Yes, **Re-enable** button |
| **System apps (Appx)** | remove system packages you do not use | Reinstallable from the Store |
| **Startup** | enable/disable `Run` entries (HKCU and HKLM) and **remove** permanently; **Terminate** the process if it is already running | Enable/disable: yes; remove: no |

**Screens** (Windows and Linux/Hyprland) draws monitors at their real proportions, with each resolution and a ★ on the primary monitor.

| Action | How |
|---|---|
| Move a window between monitors | drag the rectangle on the map, or use the **→N** buttons in the list (the occupied fraction is preserved on the destination monitor) |
| Snap to a grid | with **snap while dragging** enabled, drop on the highlighted zone in the selected **Grid** (full, halves, thirds, quadrants, primary+2, center) |
| Distribute | **Distribute N** sends everything on monitor N to the current grid zones |
| Minimize / maximize | **—** and **▾** in the open-window list |
| Build a scene | **+** on a window row adds its slot to the selected scene; **Save current** captures the whole arrangement; **New empty** starts from scratch |
| Apply a scene | **Apply** moves open windows and, for slots with **open** enabled, launches missing apps and positions them when their windows appear (gives up after 25 s) |
| Match the right window | **title contains…** breaks ties when one executable has multiple windows |

Slots store position as a fraction of the monitor's **work area**, never as pixels — changing resolution, scale or monitor does not break a scene. If a slot's monitor is gone, it falls back to the primary. On Windows, RamDog subtracts invisible window shadow bounds (`DWMWA_EXTENDED_FRAME_BOUNDS`) when positioning, so half the screen is truly half the screen.

## What each system covers

| | Windows | Omarchy / Hyprland | Other Linux | macOS |
|---|---|---|---|---|
| List, tree, categories, origin, kill, lock | yes | yes | yes | yes |
| Group by app / agent families | yes | yes | yes | yes |
| Startup | Run, tasks, services, UWP… | systemd + XDG | systemd + XDG | — |
| Waste | Defender, services, Appx, Run | systemd services | systemd services | notice |
| Screens | DWM / `SetWindowPos` | native Hyprland | — | notice |
| Thermal | `hwtemp.exe` (admin) | hwmon + authenticated PWM | hwmon + authenticated PWM | — |
| GPU | NVIDIA (NVML) | NVIDIA / AMD / Intel | NVIDIA / AMD / Intel | — |
| Top disk meter | PDH `% Idle` | `/proc/diskstats` | `/proc/diskstats` | — |
| Executable integrity | Authenticode | pacman SHA-256 | pacman SHA-256 (Arch) | — |

## Limits

**All three operating systems**

- Processes belonging to other users: Windows handles this through elevation at startup (the **Reopen as admin** button appears only when elevation was refused or unavailable); Linux administrative controls require their own authentication and metrics for other users may be unavailable; macOS access depends on session permissions. The UI never invents a number — missing GPU/temp/disk values appear as “—”.
- Configuration: Windows `%APPDATA%\RamDog\config.json`; Linux `$XDG_CONFIG_HOME/RamDog/config.json` (or `~/.config/RamDog/config.json`); macOS `~/Library/Application Support/RamDog/config.json`.

**Windows only**

- Waste (Defender, services, Appx and Startup).
- Screens: `EnumDisplayMonitors`, `EnumWindows`, `SetWindowPos` and DWM (to subtract invisible window shadow). On macOS, the equivalent is the Accessibility API and requires explicit system permission; the view exists only to explain that.
- Startup: complete reading (HKLM, tasks and services) works without admin; enabling/disabling/removing machine entries requires elevation, requested through UAC when needed.
- Digital signature: `WinVerifyTrust` exists only on Windows. The field is hidden on macOS.
- Thermal: sensors and fans through `hwtemp.exe` (LibreHardwareMonitor). Without admin, Tctl/DIMM/fans are unavailable; NVIDIA GPU still reads. Only motherboard SuperIO fans are controlled — the GPU follows its own curve.
- CPU/RAM temperature: `hwtemp.exe` (LibreHardwareMonitor), elevated. Without helper/admin/sensor, the value is “—”.
- Top and per-process GPU: **NVIDIA** (`nvml.dll`). Without the driver, the value is “—”.
- `MsMpEng.exe` is kernel-protected: the Defender section reduces its work but does not terminate it. With Tamper Protection enabled, pausing real-time protection may have no effect.

**Linux only**

- Startup/Waste: systemd services and XDG autostart with startup presets. Screens: Hyprland (including Omarchy), map, arrangement and scenes. Icons come from desktop entries; SHA-256 integrity comes from the local pacman database, not Authenticode.
- Thermal: hwmon (CPU, GPU, DIMM when available), manual/automatic PWM and authenticated-helper STABILIZE; requires a driver that permits PWM writes.
- Top disk meter: `%util` and bytes/s from `/proc/diskstats` (whole disks; partitions, loop devices and zram excluded).
- With no display (plain SSH, no `WAYLAND_DISPLAY`/`DISPLAY`), the window cannot open.
- Runtime X11/Wayland libraries are required (`libxkbcommon`, `libwayland`, `libX11`, GL).
- Release binaries require glibc 2.39+. Current Omarchy/Arch meets this; older distributions can compile from source.

**macOS only**

- No Waste, Screens, CPU temperature or NVML. The top disk meter has no Task Manager-style idle percentage.
- Gatekeeper may block the binary the first time it opens.

## How it measures

**Windows:** RAM = *Private Working Set* (`NtQuerySystemInformation`, the same column as Task Manager's Memory). Top CPU = `GetSystemTimes`; per-process CPU = each process's `CycleTime` share of the same `GetSystemTimes` capacity, with a 1-second moving average. Without `CycleTime` (older Windows or a VM that returns zero), it falls back to kernel+user deltas. A process that exits between two samples is excluded from both sides of the calculation: its percentage disappears instead of being inherited by a process that remains. GPU = NVML + PDH `\GPU Engine(*)\Utilization Percentage` (maximum across the PID's engines). Top disk = PDH `% Idle Time` + bytes/s (`PdhAddEnglishCounterW`, English counter names). `hwtemp.exe` reads Tctl/Tdie and DIMM.

**Linux:** a dedicated `/proc` sampler — no threads shown as processes and no pinned `stat` file descriptors. CPU uses `utime+stime` against online cores with a 1-second moving average. The column shows equivalent cores (`1.6×`) when machine percentage would hide a process. Load (`/proc/loadavg`) and swap (`SwapTotal`/`SwapFree`) appear in the cards; the pressure banner and **Contention** chip surface emulator leftovers, `git-credential` loops and CPU-heavy processes with little RAM. RSS comes from `statm`; USS/PSS from cached `smaps_rollup`; without smaps, private memory is `RSS − shared`. Per-process virtual memory and global commit (`Committed_AS`/`CommitLimit`) are distinct counters. Per-process disk uses `/proc/PID/io`; top disk uses `/proc/diskstats`. GPU uses nvidia-smi and DRM. [Details](linux/README.md).

**macOS:** processes, RAM (RSS) and CPU through [sysinfo](https://crates.io/crates/sysinfo). Per-process disk is bytes read+written per second. No PDH, NVML or LibreHardwareMonitor.

## License

[MIT](LICENSE).

## Releases

[v0.12.1](docs/releases/v0.12.1.md) · [Changelog](CHANGELOG.md) · [v0.12.0 patch notes](docs/releases/v0.12.0.md) · [v0.11.1](docs/releases/v0.11.1.md) · [v0.11.0](docs/releases/v0.11.0.md) · [How to publish with `./release`](docs/RELEASING.md).

The repository's social preview image is [`docs/media/banners/og.png`](docs/media/banners/og.png) (1280×640).

## Also

[**TempHUD**](https://github.com/LucasOl1337/TempHUD) — thermal overlay for Windows.
