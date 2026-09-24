# RamDog no Linux

A partir da v0.9.0, estes recursos são distribuídos nativamente para Linux, sem Wine. A **v0.9.1** troca o sampler para `/proc` direto e corrige o float do Hyprland 0.55+ — é a versão que aguenta sessão longa. Windows mantém seus próprios backends. Consulte as [patch notes v0.9.1](../docs/releases/v0.9.1.md), as da [v0.9.0](../docs/releases/v0.9.0.md) e o [changelog](../CHANGELOG.md).

## Omarchy

O desktop Linux de referência do RamDog é o [Omarchy](https://omarchy.org/) (Arch + Hyprland + UWSM + Quickshell).

- Hyprland, UWSM e Quickshell são processos críticos: o RamDog não os encerra.
- Telas e “em primeiro plano” usam `hyprctl` e a API Lua 0.55+ (`enable`/`disable` no float, não *toggle*).
- Partida e Desperdício falam com systemd de usuário e de sistema; o launcher sobe `ramdog.service`.
- GPU NVIDIA (`nvidia-smi`) e AMD/Intel (DRM/hwmon) na mesma máquina; a varredura DRM não duplica o caminho NVIDIA.
- Ícones pelos `.desktop`; SHA-256 contra o mtree local do pacman.
- O VSync explícito do eframe fica desligado: com NVIDIA EGL/Wayland, ocultar a janela travava o event loop.

A validação de referência usou Omarchy/Hyprland com NVIDIA RTX 4070 Ti SUPER e AMD integrada. Outros compositores ficam sem Telas; o resto do app não depende do Hyprland.

## Recursos

- Processos, árvore, categorias, origem, encerramento e proteção da sessão. O sampler lê `/proc` direto: threads não entram na lista, descritores de `/proc/*/stat` não ficam abertos, CPU é % da máquina com média de 1 s mesmo com afinidade restrita. Load (`/proc/loadavg`) e swap (`/proc/meminfo`) entram nos cards; a faixa Disputa aponta sobra de emulador, loop de credencial e CPU sem RAM — o recorte que a lista por memória esconde. RSS, USS e PSS têm significados distintos; USS/PSS vêm de `smaps_rollup` com teto por ciclo e cache de 5 s. Sem essa leitura, o privado usa `RSS − shared`. Falta de permissão em PSS aparece como `—` e agregações parciais como `≥`. Virtual por processo não é commit; o commit global usa Committed_AS/CommitLimit. Descritores de arquivo na ficha, com o mesmo orçamento/cache.
- GPU: seletor entre placas, carga, VRAM, temperatura, potência e fan quando expostos. NVIDIA usa `nvidia-smi` (consulta global e pmon); AMD/Intel usam DRM/hwmon. A varredura DRM por PID só abre `/dev/dri` e é pulada quando não há placa AMD/Intel. Carga e memória por PID aparecem quando o driver permite; `—` não significa 0%. Coleta externa em worker com timeout (1,5 s) e descarte de amostra obsoleta.
- Partida: serviços, sockets e timers systemd de usuário/sistema, autostart XDG, habilitação e presets. Unidades estáticas não oferecem habilitação; essenciais ficam protegidas. Habilitar na partida não inicia automaticamente o serviço agora.
- Desperdício: estado, RAM e ações iniciar/parar/reiniciar/habilitar/desabilitar dos serviços Linux. As integrações Windows específicas (Defender, registro, UWP) são substituídas por gestão de serviços Linux, não emuladas.
- Telas: mapa e seleção de janelas, movimento e redimensionamento entre monitores, grades, retorno ao tiling e cenários com abertura opcional dos programas ausentes. API Lua do Hyprland 0.55+ (`enable`/`disable` no float) e dispatch clássico nas versões anteriores. Outros compositores/X11 não oferecem este backend. Cenários não recuperam documentos ou sessões internas dos apps.
- Térmico: sensores hwmon, RPM e controle PWM manual/Auto/ESTABILIZAR em controlador compatível. A interface normal não precisa rodar como root; `pkexec` autentica somente o helper. ESTABILIZAR controla apenas fans conectados com RPM positivo (CPU e gabinete); bomba e headers vazios ficam na BIOS. Manual limitado a 30–100%, proteção térmica e restauração dos modos/PWM anteriores quando o pai termina. O app precisa permanecer aberto para manter a curva; fechado, a BIOS assume. Não controla fans da GPU.
- Limpeza (só Linux): RAM e disco numa tela. Em cima, `/proc/meminfo` (uso, cache do kernel, swap) e "Soltar cache do kernel" (`sync` + `drop_caches` + `compact_memory`, via `pkexec`). No meio, os apps acima de um corte de RAM agrupados por identidade, com sobras (emulador sem janela) e zombies primeiro e quem tem janela no fim; marca e encerra de uma vez, com o mesmo lock e proteção da lista. Zombie não recebe sinal: a linha aponta o pai. Embaixo, o disco: cada subpasta de `~/.cache` (`XDG_CACHE_HOME`) acima de 1 MB, lixeira (`XDG_DATA_HOME/Trash`), cache do pacman (`paccache -rk1` + `-ruk0`), journal (`--vacuum-size=64M`), coredumps e órfãos (`pacman -Qtdq` → `-Rns`). Tudo pede confirmação inline; o que é do sistema roda em `ramdog --clean-helper <op>` como root via `pkexec`, sem receber caminho nem nome de pacote por argumento (o helper recalcula). Symlinks nunca são seguidos, nem no tamanho nem na remoção.
- Ícones via arquivos `.desktop` e temas; uso em primeiro plano via Hyprland. Integridade de executáveis Arch via hash SHA-256 do mtree local do pacman; essa checagem não equivale a uma assinatura digital ou a verificar a autenticidade do banco local.

## Compilar e executar

Requer Rust, dependências nativas do eframe/Wayland/X11, coreutils (`timeout`), systemd e polkit para ações administrativas. `nvidia-smi` vem com o driver NVIDIA; `hyprctl` com Hyprland; `rsvg-convert` é usado para ícones SVG. Sensores dependem dos drivers do kernel.

```sh
cargo test --locked -- --test-threads=1
cargo build --locked --release
install -Dm755 target/release/ramdog ~/.local/bin/ramdog
install -Dm755 linux/ramdog-launch ~/.local/bin/ramdog-launch
~/.local/bin/ramdog-launch
```

Os binários distribuídos requerem glibc 2.39+ (Ubuntu 24.04, Omarchy/Arch atual ou distribuição compatível). Para sistemas anteriores, compile do código. Os pacotes incluem `ramdog` e `ramdog-launch`; mantenha ambos no mesmo diretório. O instalador verifica SHA-256 e respeita `RAMDOG_HOME`, `RAMDOG_VERSION` e `RAMDOG_NO_LAUNCH=1`.

No Omarchy / Arch, as libs de compilação:

```sh
sudo pacman -S --needed base-devel rust pkgconf libxkbcommon wayland libx11 \
  libxcursor libxi libxrandr mesa
```

O launcher usa uma unidade de usuário transitória `ramdog.service`, desacoplada do terminal que abriu o app. Um segundo lançamento foca a janela existente. Não reinicia automaticamente após falha: o resultado da execução fica disponível no journal. Sem systemd de usuário disponível, o launcher executa o binário diretamente. O executável também pode ser aberto diretamente.

```sh
ramdog --diagnose                  # inventário somente leitura em JSON
journalctl --user -u ramdog.service
cat ~/.local/state/RamDog/ramdog.log
```

O VSync explícito do eframe fica desativado no Linux: com NVIDIA EGL/Wayland, a versão anterior bloqueava o event loop ao ocultar a janela. A coleta e os pedidos de repaint continuam controlando a atualização.

O log também respeita XDG_STATE_HOME, gira ao atingir 2 MiB e registra início, retorno e panic/backtrace. SIGKILL não pode executar um handler; nesse caso é necessário consultar o journal. Não execute a GUI inteira com sudo para obter controles térmicos.

## Compatibilidade de sensores e PWM

As leituras e controles dependem do driver hwmon do kernel. O instalador do RamDog não instala módulos, DKMS, regras de acesso ou configurações específicas de placa-mãe. A presença de sensores/RPM não garante que o controlador exponha PWM gravável. Sem esse suporte, use as leituras disponíveis e o controle da BIOS.

O helper restaura o modo anterior quando detecta o término do processo principal e trata SIGTERM/SIGINT. SIGKILL do próprio helper ou perda de energia não permitem executar a restauração. Não execute vários controladores PWM sobre o mesmo hardware ao mesmo tempo.

## Validação

A suíte contém testes de memória/threads reais, proteção de PIDs, discos/hotplug, NVIDIA, XDG e geometria. Dois testes ignorados por padrão exercitam alterações temporárias reais:

```sh
cargo test linux_integration::user_service_lifecycle -- --ignored
RAMDOG_TEST_WINDOW_PID=<PID-de-uma-janela-RamDog-de-teste> cargo test linux_integration::window_move_resize_restore -- --ignored
```

O teste de janela restaura a posição e o estado anterior. O de serviço cria e remove sua própria unidade. `ramdog --smoke-test` percorre abas, mini e métricas por 90 s, fecha normalmente e não salva alterações de configuração.

Referências: [NVIDIA SMI](https://docs.nvidia.com/deploy/nvidia-smi/index.html), [DRM fdinfo](https://docs.kernel.org/gpu/drm-usage-stats.html), [hwmon](https://docs.kernel.org/hwmon/sysfs-interface.html), [Hyprland dispatchers](https://wiki.hypr.land/configuring/core/dispatchers/).

A validação manual de referência usou Omarchy/Hyprland com NVIDIA RTX 4070 Ti SUPER e AMD integrada: smoke de 90 s, janela de teste exclusiva (incluindo dois encaixes seguidos para confirmar que o float não vira toggle), serviço temporário e restauração térmica. A v0.9.1 foi a primeira a aguentar sessão longa nesta máquina sem vazar descritores nem inflar o CPU do próprio app. A release também executa testes e builds em runners Linux x86_64/aarch64. Resultados em outros drivers, placas e compositores podem diferir.
