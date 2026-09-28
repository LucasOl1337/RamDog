# RamDog reference

Everything RamDog does, per view and per operating system. For a quick start, see the [README](../README.md); for Linux internals, dependencies and validation, see the [Linux guide](../linux/README.md).

## Features

### Language

New installs follow the system language: Portuguese when the system locale is Portuguese, English otherwise. Change it any time in **Preferences → Language**; the choice is saved in the RamDog config and applies to every view. Configs saved by earlier versions without a language setting keep Portuguese.

- **Origin / launched by.** The Origin column shows the first living ancestor that is not a generic host (`cmd`, `bash`, `node`, …). When the parent chain has exited, RamDog reads inherited environment variables and shows the agent (Claude Code + session + PID, Codex, Cursor Agent, Gemini CLI, Hermes, …), host (Maestri, VS Code, Cursor, Windows Terminal, …) and `npm run <script>` project in purple.
- **Launched by.** The last column shows the complete launcher chain (`terminal › shell › agent`) instead of dumping command arguments. Right-click its header to switch back to **Command**. Clicking a parent selects it.
- **Contention.** When a game is open, RamDog highlights CPU-heavy processes, high load or swap, software GPU rendering, headless Android emulators, and credential helpers stuck in a loop. The CPU column can show equivalent saturated cores alongside machine percentage.
- **Honest zombie handling.** A zombie is already dead and holds no memory. RamDog asks its live parent to reap it, names the process holding the entry, and offers to terminate that parent when appropriate instead of claiming the zombie itself was killed.
- **Categories.** AI / Agents, Dev, Browser, Games, Personal, System and Other — automatic rules with a manual per-process override.
- **Kill, tree and lock.** Terminate a process or its tree (process plus children). A lock prevents RamDog from terminating the protected process. Critical OS processes (`System`/`csrss`/`dwm` on Windows; `systemd`/`kthreadd`/`Hyprland`/`UWSM`/`Quickshell`/`gnome-shell`/`Xorg` on Linux; `kernel_task`/`launchd`/`WindowServer` on macOS) are always protected.
- **Views.** The sidebar holds the process views, **Processes** (flat list), **Tree** (parent → children, subtree RAM) and **Categories** (grouped), and below them the **Add-ons**: **Sweep**, **Startup**, **Drains**, **Thermal**, **Screens** and **Cleanup**. Clicking one swaps the window content. On Linux, Startup and Drains use systemd/XDG, Screens uses Hyprland, Thermal uses hwmon with authenticated PWM control, Sweep sorts open-but-unused apps into can close / maybe / in use and closes whatever you select in bulk, and Cleanup combines kernel cache and zombies with disk locations (`~/.cache`, trash, pacman, journal, coredumps and orphans). See the [Linux guide](../linux/README.md).
- **Group by app** (on by default). In Processes and Categories, recognized families (Claude, Codex, Grok, ChatGPT, Cursor, Gemini, Hermes, Maestri, OpenCode), Python/Node projects and processes with the same executable become one row (`brave · 18 processes · 1.81 GB`) summing RAM, CPU, GPU and disk. On Linux the sum uses PSS, so shared pages are not counted once per process. The **✖** action terminates the whole app. Groups start collapsed; click **▶** to see their PIDs. Family keys follow versioned installs (`mise`, PATH, the ChatGPT folder), the agent-launched `node`, and interpreters that rename themselves (`python3.11` shown as `hermes`). Outside those families, the key is the executable path rather than its name, so two `worker` processes from different folders never merge. A single-process app is not grouped. The **Largest now** footer also sums by app.
- **Readable CPU column.** On Windows, process allocation uses `CycleTime` (counted at context switches), not kernel/user time — Windows charges that in 15.625 ms slices and assigns the whole slice to whoever ran at the tick, making bursty processes flicker between 0% and 15%. The total comes from `GetSystemTimes`, so a busy machine does not dilute the culprit. Linux compares `/proc/*/stat` against the machine's online cores, not RamDog's own affinity. Both use a time-bound 1-second moving average so the list stays still long enough to read; the raw last-interval value remains in the tooltip.
- **Meters.** CPU and RAM are shown at the top on all three operating systems. Windows uses NVML for NVIDIA; Linux uses `nvidia-smi` plus DRM/hwmon for NVIDIA/AMD/Intel. Disk percentage is PDH on Windows and `/proc/diskstats` on Linux; it is unavailable on macOS.
- **Thermal.** On Windows, embedded [TempHUD](https://github.com/LucasOl1337/TempHUD) provides CPU/GPU/RAM/motherboard sensors, SuperIO fan control (manual percentage or Auto/BIOS) and **STABILIZE** — fans locked to 50% up to 80 °C, a linear ramp to 100% at 92 °C and an immediate ceiling at 95 °C. The `hwtemp.exe` helper restores BIOS control if RamDog exits. Fans require admin; without them, the view still shows available readings. Linux reads `/sys/class/hwmon` and offers PWM/STABILIZE through compatible authenticated helpers that restore the prior state when they exit.
- **Startup.** Everything that starts with the PC, not just Task Manager's subset: `Run` and `RunOnce` (HKCU, HKLM and Wow64), the complete Startup folder (`.lnk`, `.vbs`, `.cmd`), scheduled boot/logon tasks, automatic services, UWP apps, Winlogon and Active Setup. The outer filter is **starts with the PC / does not start / broken**, with three clickable counters. Inside each block, startup phase runs from the surface inward: your programs, at sign-in, with the machine, before Windows. Each band shows entry and currently-running counts and collapses on click. The startup check answers “starts with the PC”; the **Now** column answers “has a process running”. The inner grouping can be changed to source type, phase, type or flat list. Linux reads systemd (user and system) and XDG autostart.
- **Screens.** The monitor map is drawn to scale: drag a window between monitors and drop it into a grid zone (halves, thirds, quadrants, primary+2, …). **Distribute** spreads everything on one monitor across the selected grid. **Scenes** save the arrangement as a fraction of the work area, not pixels, so presets survive resolution, scale and monitor changes; applying a scene moves open windows and launches missing ones when their windows appear. On Linux the backend is Hyprland (including Omarchy).
- **What is this, can I kill it?** The Windows details panel contains a catalog of 80 processes: what they do, why they are open and the risk of terminating them — 🟢 safe, 🟡 Windows restarts it, 🔴 it takes down the session.
- **Digital signature.** The signer comes from the certificate (`WinVerifyTrust`), not the file's `CompanyName`, which an impostor can fill with “Microsoft Corporation”. Verification is on demand for the selected process, outside the sampler. On Arch, the documented equivalent is pacman's SHA-256 comparison against the file on disk.
- **Mini mode.** The **Mini** button turns the app into a borderless HUD with CPU, RAM, GPU and disk in a 2×2 layout, each with its temperature, plus fan RPM and **STABILIZE**. It stays above other windows (toggle **top**), can be dragged by its background, minimized, and restored with a double-click. The mode is remembered between sessions.

## Usage

| Action | How |
|---|---|
| Terminate process | **✖** on the row, `Del`, right-click → Terminate, or the bottom panel |
| Terminate tree (process + children) | `Shift+Del`, `Shift`+**✖**, right-click → Terminate tree, or the bottom panel |
| Protect / unprotect | **🔒**/**🔓** on the row, right-click or the bottom panel. Protected processes are never terminated by RamDog |
| Manual category | right-click → Category, or the combo in the bottom panel (`auto` returns to automatic rules) |
| Views | **Processes**, **Tree** and **Categories** at the top of the sidebar; **Sweep**, **Startup**, **Drains**, **Thermal**, **Screens** and **Cleanup** under **Add-ons**. Startup/Drains/Screens/Thermal are also implemented on Linux (systemd, Hyprland and hwmon) |
| Group by app | **Group by app** in Processes and Categories; groups start collapsed, **▶**/**▼** reveal PIDs, **Expand all**/**Collapse** apply to the whole list, and **✖** in the header terminates every process in that app |
| Filter | search by name / PID / command; category chips (click toggles, double-click isolates); `hide below N MB` and the `RAM column shows` control on the right |
| Origin | *Origin* is the first living ancestor that is not a generic host (cmd, bash, node, …); the full chain is clickable in the bottom panel (**Go to parent**) |
| Launched by | when the parent chain has exited or contains only generic hosts, RamDog reads inherited environment variables and shows the originating agent (Claude Code + session + PID, Codex, Cursor Agent, Gemini CLI, Hermes, …), host (Maestri, VS Code, Cursor, Windows Terminal, …) and `npm run <script>` in the project |
| Refresh | **⏸ pause** and `every 0.5–5 s` in the footer beside the `sample X ms` value; `F5` forces a read |
| Mini mode | **Mini** in the upper right. In the HUD, **top** toggles always-on-top, **–** minimizes, **🗖** (or double-clicking the background) restores the full app, **🗙** closes; the interval button cycles 0.5 / 1 / 2 / 5 s; drag by the background |

Termination is immediate: there is no confirmation dialog. Protection comes from the **lock** (🔒 in the context menu) — a locked process survives `Del`, **✖** and tree termination. While the pointer is over the table, row order is **frozen** so **✖** never lands on a process that just moved.

**Startup** (Windows) lists what starts at boot and logon, with the source of each entry (`Run`, Startup folder, scheduled task, service, UWP, Winlogon, Active Setup), whether it is currently running and the executable's real path. Enable, disable and remove apply to the entry; the process itself remains terminable from List view. Machine entries (HKLM, services and tasks) require admin — without elevation, RamDog starts one elevated PowerShell action with **one UAC prompt per action**.

**Drains** (Windows) reads directly through the Service Control Manager and registry, without PowerShell. **Actions** use direct Win32 when RamDog is already elevated; otherwise they start one elevated PowerShell action with **one UAC prompt per action**. On macOS, the view exists only to explain that it is unavailable.

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
| Drains | Defender, services, Appx, Run | systemd services | systemd services | notice |
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

- Drains (Defender, services, Appx and Startup).
- Screens: `EnumDisplayMonitors`, `EnumWindows`, `SetWindowPos` and DWM (to subtract invisible window shadow). On macOS, the equivalent is the Accessibility API and requires explicit system permission; the view exists only to explain that.
- Startup: complete reading (HKLM, tasks and services) works without admin; enabling/disabling/removing machine entries requires elevation, requested through UAC when needed.
- Digital signature: `WinVerifyTrust` exists only on Windows. The field is hidden on macOS.
- Thermal: sensors and fans through `hwtemp.exe` (LibreHardwareMonitor). Without admin, Tctl/DIMM/fans are unavailable; NVIDIA GPU still reads. Only motherboard SuperIO fans are controlled — the GPU follows its own curve.
- CPU/RAM temperature: `hwtemp.exe` (LibreHardwareMonitor), elevated. Without helper/admin/sensor, the value is “—”.
- Top and per-process GPU: **NVIDIA** (`nvml.dll`). Without the driver, the value is “—”.
- `MsMpEng.exe` is kernel-protected: the Defender section reduces its work but does not terminate it. With Tamper Protection enabled, pausing real-time protection may have no effect.

**Linux only**

- Startup/Drains: systemd services and XDG autostart with startup presets. Screens: Hyprland (including Omarchy), map, arrangement and scenes. Icons come from desktop entries; SHA-256 integrity comes from the local pacman database, not Authenticode.
- Thermal: hwmon (CPU, GPU, DIMM when available), manual/automatic PWM and authenticated-helper STABILIZE; requires a driver that permits PWM writes.
- Top disk meter: `%util` and bytes/s from `/proc/diskstats` (whole disks; partitions, loop devices and zram excluded).
- With no display (plain SSH, no `WAYLAND_DISPLAY`/`DISPLAY`), the window cannot open.
- Runtime X11/Wayland libraries are required (`libxkbcommon`, `libwayland`, `libX11`, GL).
- Release binaries require glibc 2.39+. Current Omarchy/Arch meets this; older distributions can compile from source.

**macOS only**

- No Drains, Screens, CPU temperature or NVML. The top disk meter has no Task Manager-style idle percentage.
- Gatekeeper may block the binary the first time it opens.

## How it measures

**Windows:** the RAM column defaults to the working set; *Private Working Set* (`NtQuerySystemInformation`, the same number as Task Manager's Memory column) and Commit are available from the **RAM column shows** control. Top CPU = `GetSystemTimes`; per-process CPU = each process's `CycleTime` share of the same `GetSystemTimes` capacity, with a 1-second moving average. Without `CycleTime` (older Windows or a VM that returns zero), it falls back to kernel+user deltas. A process that exits between two samples is excluded from both sides of the calculation: its percentage disappears instead of being inherited by a process that remains. GPU = NVML + PDH `\GPU Engine(*)\Utilization Percentage` (maximum across the PID's engines). Top disk = PDH `% Idle Time` + bytes/s (`PdhAddEnglishCounterW`, English counter names). `hwtemp.exe` reads Tctl/Tdie and DIMM.

**Linux:** a dedicated `/proc` sampler — no threads shown as processes and no pinned `stat` file descriptors. CPU uses `utime+stime` against online cores with a 1-second moving average. The column shows equivalent cores (`1.6×`) when machine percentage would hide a process. Load (`/proc/loadavg`) and swap (`SwapTotal`/`SwapFree`) appear in the cards; the pressure banner and **Contention** chip surface emulator leftovers, `git-credential` loops and CPU-heavy processes with little RAM. The RAM column defaults to PSS (proportional set size): each shared page is split among the processes that map it, so sums across a group, category or the whole list match real usage. RSS comes from `statm`; USS/PSS from cached `smaps_rollup`; without smaps, private memory is `RSS − shared`. Per-process virtual memory and global commit (`Committed_AS`/`CommitLimit`) are distinct counters. Per-process disk uses `/proc/PID/io`; top disk uses `/proc/diskstats`. GPU uses nvidia-smi and DRM. [Details](../linux/README.md).

**macOS:** processes, RAM (RSS) and CPU through [sysinfo](https://crates.io/crates/sysinfo). Per-process disk is bytes read+written per second. No PDH, NVML or LibreHardwareMonitor.
