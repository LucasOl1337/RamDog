# Contributing to RamDog

Thanks for helping. Issues and pull requests are welcome in **English or Portuguese**.

## Ways to help

- **Hardware and desktop reports.** RamDog reads sensors, fans, GPUs and windows through drivers and compositors that vary a lot. A report that says "on my board / GPU / compositor this reading is missing or wrong", with the output of `ramdog --diagnose`, is one of the most useful things you can send.
- **Bugs.** Use the bug report form. Include your OS, desktop, RamDog version (Preferences, or the release you installed), what you did and what you expected.
- **Process identification.** If an app is split into several groups, grouped with the wrong thing, or put in the wrong category, open an issue with the process name, executable path and command line (redact anything private).
- **Pull requests.** Small, focused changes are easiest to review. For a new view or a larger change, open an issue first so we can agree on the approach.

## Development setup

You need Rust (stable) and the native libraries eframe uses.

```sh
# Arch / Omarchy
sudo pacman -S --needed base-devel rust pkgconf libxkbcommon wayland libx11 \
  libxcursor libxi libxrandr mesa

# Debian / Ubuntu
sudo apt install build-essential pkg-config libxkbcommon-dev libwayland-dev \
  libx11-dev libxcursor-dev libxi-dev libxrandr-dev libgl1-mesa-dev

git clone https://github.com/LucasOl1337/RamDog.git
cd RamDog
cargo run
```

To try a build without touching your real settings, point it at a scratch config:

```sh
XDG_CONFIG_HOME=/tmp/ramdog-dev/config XDG_STATE_HOME=/tmp/ramdog-dev/state cargo run
```

## Before you open a pull request

```sh
cargo fmt -- --check
cargo test --locked -- --test-threads=1
python3 tests/test_installer.py     # if you touched install.sh or linux/ramdog-launch
```

CI builds and tests Linux x86_64/aarch64, macOS (Apple Silicon and Intel) and Windows x64 on every push to `main`. If your change is platform-specific, keep it behind the matching `cfg` and make sure the other targets still compile.

## Project conventions

- **Both languages in the UI.** Every user-visible string goes through `locale.text("português", "English")`. Add both; if you are not comfortable writing one of them, say so in the PR and a maintainer will fill it in.
- **Never invent a number.** When a reading is unavailable, show `—` and explain why in the tooltip. Partial sums are marked `≥`.
- **Destructive actions stay safe.** Anything that kills, removes or changes the system must respect locks and the protected-process list, and ask for confirmation when it affects more than the row you clicked. Privileged work goes through the existing helpers (`--clean-helper`, `--fan-helper`, UAC on Windows), never by running the GUI as root.
- **Comments explain why.** Much of the codebase is commented in Portuguese; new comments can be in English or Portuguese. Describe the reason or the bug a line prevents, not what the code already says.
- **Tests next to the logic.** Pure logic (identity, grouping, table queries, config migration) has unit tests in the same module or in `src/app/table/tests.rs`. Add one when you change behavior there.

## Where things live

| Path | What it is |
|---|---|
| `src/app.rs`, `src/app/table/` | UI, row building, grouping, table cache and queries |
| `src/procs*.rs`, `src/sampler.rs` | Process sampling per OS (`/proc` on Linux) |
| `src/identity.rs`, `src/categories.rs` | App identity, grouping keys and categories |
| `src/*_linux.rs` | Linux backends: GPU, fans, startup, screens, cleanup, desktop |
| `src/sweep.rs` | Sweep classification |
| `src/config.rs` | Settings and their migrations |
| `hwtemp/` | Windows thermal helper (.NET, LibreHardwareMonitor) |
| `linux/` | `ramdog-launch` and the Linux guide |
| `docs/` | Reference, release notes, site and media |

## Releases

Maintainers publish versions with `./release vX.Y.Z`; see [docs/RELEASING.md](docs/RELEASING.md). Please do not bump the version in pull requests.

By contributing, you agree that your contribution is licensed under the [MIT License](LICENSE).
