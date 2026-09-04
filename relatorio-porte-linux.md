# Relatório — porte Linux (OMART)

Data: 2026-09-04  
Repo: `C:\Projetos\RamDog` · `https://github.com/LucasOl1337/RamDog.git` · `main`  
Máquina deste turno: Windows. **A máquina Linux do OMART não está neste PC.**

## HamDog vs RamDog

Não há `HamDog` / `hamdog` em lugar nenhum deste repo. O app no disco é **RamDog** (crate `ramdog`, README, ícones, `hwtemp`). Ordem do grok: tratar HamDog = RamDog até o capitão corrigir. Não parei.

Commit ao GitHub: **não enviado** (não autorizado neste turno).

## O que o código já tinha (antes deste porte)

O crate já compilava para Unix:

- `eframe` com features `wayland` e `x11`
- `sysinfo` + `libc` em `cfg(not(windows))`
- `procs_unix.rs` / `metrics_unix.rs`
- stubs de Partida / Desperdício / Telas
- config em `$XDG_CONFIG_HOME/RamDog` no Linux
- `xdg-open` para URL e pasta

O que **não** funcionava de verdade no Linux:

- `install.sh` recusava qualquer SO que não fosse Darwin
- README só falava Windows e macOS
- `fallback_rule` tratava `session == 0` como “serviço do Windows”. No Unix o campo vinha sempre 0 → **todo processo caía em Sistema**
- `is_critical` não protegia PID 1, `kthreadd`, kernel threads, compositor
- Térmico / disco no topo / mensagens da UI falavam “macOS” e desligavam o Linux
- Sem leitura de hwmon, sem `/proc/diskstats`

## O que este porte mudou (código no repo)

| Área | Mudança |
|---|---|
| Classificação | `session == 0` só no Windows. Nomes Linux em Sistema/Dev/Browser. Steam via `/steam/` |
| Lock | PID 1 e 2, kernel threads, `systemd`/`init`, dbus, compositor (GNOME/KWin/Xorg), display manager |
| Processos | `session_id` e contagem de tasks (sysinfo) no Unix |
| Térmico | Linux lê `/sys/class/hwmon` (temp + RPM). Sem escrita de PWM, sem ESTABILIZAR |
| Disco (topo) | Linux: `%util` + bytes/s de `/proc/diskstats` (discos inteiros; partição/loop/zram de fora) |
| Origem | Kitty, Konsole, GNOME Terminal, Tilix, Ghostty nas env vars |
| Fichas | systemd, kthreadd, dbus, compositor, pipewire, NetworkManager, sshd |
| UI | Textos de Térmico / sudo / Partida / Desperdício / Telas distinguem Linux de macOS |
| Instalar | `install.sh` aceita Linux x86_64 e aarch64; cai em `cargo build` se o release não existir |
| CI | `.github/workflows/linux.yml` (Ubuntu 24.04, x86_64) — **não disparado** (sem push) |
| Docs | README: badge Linux, apt deps, limites honestos, config XDG |

Windows: Partida, Desperdício, Telas, `hwtemp.exe`, NVML, assinatura, ícones — **não mexidos na lógica**. Manifesto de elevação continua só em `release` no Windows.

## O que deve rodar no Linux (por leitura do código, não por teste)

Com Rust + libs X11/Wayland/GL, display gráfico (`WAYLAND_DISPLAY` ou `DISPLAY`):

- Janela (eframe glow)
- Lista / árvore / categorias / kill / lock / origem por env
- CPU e RAM no topo (sysinfo)
- Disco no topo se `/proc/diskstats` existir
- Térmico em leitura se o kernel exportar hwmon (`coretemp` / `k10temp` / `zenpower` / GPU / DIMM)

Kill: `SIGKILL`. Sem root, processos de outro uid falham com “acesso negado (sudo?)”.

## O que quebra ou fica de fora no Linux

- **Partida, Desperdício, Telas:** stubs. Sem systemd-units, sem autostart, sem mover janela (X11/Wayland).
- **GPU no topo e por processo:** sem NVML. Mostra “–”.
- **Fans / ESTABILIZAR:** de propósito. Escrever PWM sem o fallback da BIOS do `hwtemp.exe` é perigoso; não implementei.
- **Ícone do executável:** `None` (não há SHGetFileInfo).
- **Assinatura digital:** “só no Windows”.
- **Pools do kernel na lista:** sem equivalente barato; linhas de kernel somem (já era assim no Unix).
- **SSH sem display:** a janela não abre.
- **Release:** `RamDog-linux-*.tar.gz` ainda não existe no GitHub; `install.sh` cai em compile local.
- **aarch64:** script e README citam; CI só gera x86_64.
- Kernel threads com nome fora da lista (ex.: driver específico) podem não ficar em lock — o PID 1/2 e os prefixos clássicos (`kworker`, `ksoftirqd`, …) sim.

## O que não foi verificado

Não invento resultado.

| Checagem | Estado |
|---|---|
| Rodar o app no Linux do OMART | **Não.** Essa máquina não está neste PC. |
| `cargo build` / `cargo check` para `x86_64-unknown-linux-gnu` neste PC | **Não.** Sem target Linux ligado aqui; regra do operador também não autorizou typecheck/build neste turno. |
| Recompilar e abrir o RamDog no Windows depois das edições | **Não** neste turno. As mudanças de Windows são `cfg!(windows)` em classificação e textos; a amostragem Win32 não foi reescrita. |
| Workflow `linux.yml` no GitHub Actions | **Não.** Sem push/tag. |
| hwmon nesta máquina | **Não.** Este PC é Windows; `/sys/class/hwmon` não existe aqui. |
| Wayland vs X11 de verdade | **Não.** Só o fato de o `Cargo.toml` já pedir as duas features. |

## Como validar no OMART (quando a máquina existir)

1. Instalar `build-essential pkg-config libxkbcommon-dev libwayland-dev libx11-dev libxcursor-dev libxi-dev libxrandr-dev libgl1-mesa-dev` e rustup.
2. `cd /caminho/RamDog && cargo build --release`
3. Numa sessão gráfica: `./target/release/ramdog`
4. Conferir: lista de processos **não** toda em Sistema; PID 1 com lock; ✖ em processo próprio; Térmico se hwmon existir; disco no topo; Partida/Desperdício/Telas só explicam a ausência.
5. Sem display: esperar falha de janela, não lista headless.

## Estado

**Parcial.** Código do porte está no repo. Relatório honesto. Falta prova em Linux real (OMART) e prova de compile no target GNU. Sem commit.
