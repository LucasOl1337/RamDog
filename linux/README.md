# RamDog on Linux

[Português](README.pt-BR.md)

Since v0.9.0 RamDog runs natively on Linux, without Wine. The Linux build has its own backends for processes, GPU, services, windows, sensors and cleanup; Windows keeps its own. See the [changelog](../CHANGELOG.md) for what changed in each version.

## Omarchy

RamDog's reference Linux desktop is [Omarchy](https://omarchy.org/) (Arch + Hyprland + UWSM + Quickshell).

- Hyprland, UWSM and Quickshell are critical processes: RamDog never terminates them.
- Screens and "focused app" use `hyprctl` and the Hyprland 0.55+ Lua API (explicit `enable`/`disable` for floating, never a *toggle*).
- Startup and Drains talk to the user and system systemd instances; the launcher runs RamDog as `ramdog.service`.
- NVIDIA (`nvidia-smi`) and AMD/Intel (DRM/hwmon) GPUs on the same machine; the DRM scan does not double-count the NVIDIA path.
- Icons come from `.desktop` files; executable integrity is checked with SHA-256 against the local pacman mtree.
- eframe's explicit VSync is off: with NVIDIA EGL/Wayland, hiding the window used to stall the event loop.

Reference validation used Omarchy/Hyprland with an NVIDIA RTX 4070 Ti SUPER and integrated AMD graphics. Other compositors do not get Screens; everything else does not depend on Hyprland.

## Features

- **Processes, tree, categories, origin, kill and session protection.** The sampler reads `/proc` directly: threads never show up as processes, `/proc/*/stat` descriptors are not kept open, and CPU is a share of the whole machine with a 1-second average even under restricted affinity. Load (`/proc/loadavg`) and swap (`/proc/meminfo`) appear in the cards; the **Contention** banner points at emulator leftovers, credential-helper loops and CPU-heavy processes with little RAM, the slice a memory-sorted list hides.
- **Memory.** RSS, USS and PSS mean different things. The RAM column defaults to **PSS**, which splits shared pages among the processes that map them, so app groups, categories and the top chips add up to what an app really uses. USS/PSS come from `smaps_rollup`, read with a per-cycle budget and a 5-second cache; without that reading, private memory falls back to `RSS − shared`. Processes whose PSS cannot be read (root or other users) show `—`, and partial sums show `≥`. Per-process virtual memory is not commit; global commit uses `Committed_AS`/`CommitLimit`. Open file descriptors appear in the details panel with the same budget/cache.
- **Group by app.** Processes of the same app become one row in Processes and Categories, with RAM, CPU, GPU and disk summed. Known agent families (Claude, Codex, Grok, Hermes, Cursor, Gemini, OpenCode, Maestri, ChatGPT) are recognized by executable, by process name (an interpreter renamed to `hermes`) or by inherited environment; Python/Node apps are grouped by project, and everything else by executable path.
- **GPU.** Card selector, load, VRAM, temperature, power and fan when exposed. NVIDIA uses `nvidia-smi` (global query and pmon); AMD/Intel use DRM/hwmon. The per-PID DRM scan only opens `/dev/dri` and is skipped without an AMD/Intel card. Per-PID load and memory appear when the driver allows it; `—` does not mean 0%. External collection runs in a worker with a 1.5-second timeout and drops stale samples.
- **Startup.** User and system systemd services, sockets and timers, XDG autostart, enable/disable and presets. Static units cannot be enabled; essential ones are protected. Enabling at startup does not start the service now.
- **Drains.** State, RAM and start/stop/restart/enable/disable for Linux services. Windows-specific integrations (Defender, registry, UWP) are replaced by Linux service management, not emulated.
- **Screens.** Monitor map, window selection, moving and resizing across monitors, grids, return to tiling, and scenes that can reopen missing programs. Hyprland 0.55+ Lua API, with classic dispatchers on older versions. Other compositors and X11 do not have this backend. Scenes do not restore documents or in-app sessions.
- **Thermal.** hwmon sensors, RPM and manual/Auto/STABILIZE PWM control on a compatible controller. The GUI never runs as root: `sudo -n` when already authorized, otherwise `pkexec`, elevates only the helper. STABILIZE only drives connected fans with a positive RPM (CPU and case); the pump and empty headers stay with the BIOS. Manual control is limited to 30–100%, with thermal protection and restoration of the previous modes/PWM when RamDog exits. RamDog must stay open to keep the curve; when closed, the BIOS takes over. GPU fans are not controlled.
- **Sweep** (Linux only). Groups each app with its processes and classifies it by the CPU, disk, GPU and focus RamDog observed. **Can close** is pre-selected and only gets leftovers with evidence (a hidden Android emulator; a process launched by an agent session that already ended, idle for 10 minutes and without a window). **Maybe**: no window and idle for 30 minutes, or a window without focus for 2 hours. **In use**: recent activity or focus, systemd services, or helpers of an app in use. Select or clear by group or row, close with confirmation (children included); protected and system processes never appear. **Keep always** remembers the app identity, not the runtime name. Observations are stored in `sweep.json` next to the config and survive restarts of up to 15 minutes; right after RamDog opens, almost everything shows as in use.
- **Cleanup** (Linux only). RAM and disk on one screen. On top, `/proc/meminfo` (used, kernel cache, swap) and **Drop kernel cache** (`sync` + `drop_caches` + `compact_memory`, through `pkexec`). In the middle, zombies: they cannot receive signals, so each one points at its parent with **terminate parent**. Open-but-unused apps live in Sweep. At the bottom, disk: every `~/.cache` (`XDG_CACHE_HOME`) subfolder above 1 MB, trash (`XDG_DATA_HOME/Trash`), pacman cache (`paccache -rk1` + `-ruk0`), journal (`--vacuum-size=64M`), coredumps and orphans (`pacman -Qtdq` → `-Rns`). Everything asks for inline confirmation; system operations run as `ramdog --clean-helper <op>` as root through `pkexec`, and the helper never receives a path or package name as an argument (it recomputes them). Symlinks are never followed, neither for size nor removal.
- **Icons and integrity.** Icons from `.desktop` files and icon themes; foreground usage from Hyprland. Arch executable integrity is a SHA-256 comparison with the local pacman mtree; this is not a digital signature and does not authenticate the local database.

## Build and run

Requires Rust, the eframe/Wayland/X11 native libraries, coreutils (`timeout`), systemd, and polkit for administrative actions. `nvidia-smi` ships with the NVIDIA driver, `hyprctl` with Hyprland, and `rsvg-convert` is used for SVG icons. Sensors depend on kernel drivers.

```sh
cargo test --locked -- --test-threads=1
cargo build --locked --release
install -Dm755 target/release/ramdog ~/.local/bin/ramdog
install -Dm755 linux/ramdog-launch ~/.local/bin/ramdog-launch
~/.local/bin/ramdog-launch
```

Release binaries require glibc 2.39+ (Ubuntu 24.04, current Omarchy/Arch or a compatible distribution); on older systems, build from source. Packages contain `ramdog`, `ramdog-launch` and the icon; keep both executables in the same directory. The installer verifies SHA-256, adds an app launcher entry and honors `RAMDOG_HOME`, `RAMDOG_VERSION`, `RAMDOG_NO_LAUNCH=1` and `RAMDOG_NO_DESKTOP=1`.

Build dependencies on Omarchy / Arch:

```sh
sudo pacman -S --needed base-devel rust pkgconf libxkbcommon wayland libx11 \
  libxcursor libxi libxrandr mesa
```

On Debian/Ubuntu:

```sh
sudo apt install build-essential pkg-config libxkbcommon-dev libwayland-dev \
  libx11-dev libxcursor-dev libxi-dev libxrandr-dev libgl1-mesa-dev
```

The launcher runs RamDog in a transient user unit, `ramdog.service`, detached from the terminal that opened it. Launching again focuses the existing window. It does not restart after a crash; the result stays in the journal. Without a user systemd instance, the launcher runs the binary directly, and the binary can always be run directly.

```sh
ramdog --diagnose                  # read-only JSON inventory
journalctl --user -u ramdog.service
cat ~/.local/state/RamDog/ramdog.log
```

The log honors `XDG_STATE_HOME`, rotates at 2 MiB and records start, exit and panic/backtrace. SIGKILL cannot run a handler; check the journal in that case. Do not run the whole GUI with sudo to get thermal controls.

## Sensor and PWM compatibility

Readings and controls depend on the kernel hwmon driver. RamDog does not install kernel modules, DKMS packages, access rules or board-specific configuration. Sensors and RPM readings do not guarantee that the controller exposes writable PWM; without it, use the available readings and the BIOS fan control.

The helper restores the previous mode when it sees the main process exit and handles SIGTERM/SIGINT. SIGKILL of the helper itself or a power loss cannot run the restore. Do not run several PWM controllers on the same hardware at the same time.

## Validation

The suite covers real memory/thread reads, PID protection, disks/hotplug, NVIDIA, XDG and geometry. Two tests are ignored by default because they make real temporary changes:

```sh
cargo test linux_integration::user_service_lifecycle -- --ignored
RAMDOG_TEST_WINDOW_PID=<PID-of-a-test-RamDog-window> cargo test linux_integration::window_move_resize_restore -- --ignored
```

The window test restores the previous position and state. The service test creates and removes its own unit. `ramdog --smoke-test` cycles through views, mini mode and metrics for 90 seconds, exits normally and does not save configuration changes.

References: [NVIDIA SMI](https://docs.nvidia.com/deploy/nvidia-smi/index.html), [DRM fdinfo](https://docs.kernel.org/gpu/drm-usage-stats.html), [hwmon](https://docs.kernel.org/hwmon/sysfs-interface.html), [Hyprland dispatchers](https://wiki.hypr.land/configuring/core/dispatchers/).

Manual reference validation used Omarchy/Hyprland with an NVIDIA RTX 4070 Ti SUPER and integrated AMD graphics: the 90-second smoke test, an exclusive test window (including two consecutive snaps to confirm floating is not toggled), a temporary service and thermal restoration. Releases also run tests and builds on Linux x86_64/aarch64 runners. Other drivers, boards and compositors may behave differently.
