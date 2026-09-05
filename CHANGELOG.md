# Changelog

As mudanças são registradas por versão. As notas descrevem funcionalidades disponíveis e suas limitações; testes de hardware não equivalem a cobertura de todos os drivers e desktops.

## [0.9.0] - 2026-09-05

### Adicionado

- Linux: Partida com serviços, sockets e timers systemd de usuário/sistema, autostart XDG e presets de inicialização.
- Linux: Desperdício com estado e consumo dos serviços e ações para iniciar, parar, reiniciar, habilitar e desabilitar.
- Linux: GPU NVIDIA via `nvidia-smi`, AMD/Intel via DRM/hwmon, seleção entre placas e métricas globais e por processo quando o driver permite.
- Linux: Telas com mapa de monitores, movimento, redimensionamento, grades, retorno ao tiling e cenários no Hyprland, incluindo API Lua 0.55+.
- Linux: sensores, RPM e PWM manual/Auto/ESTABILIZAR em controladores hwmon compatíveis. Helper autenticado por polkit, faixa manual de 30–100%, proteção térmica e restauração do estado anterior ao término do app.
- Linux: memória proporcional (PSS), memória privada (USS), descritores de arquivo, ícones de aplicativos, uso em primeiro plano via Hyprland e comparação SHA-256 com a base local do pacman.
- Linux: diagnóstico JSON com `--diagnose`, smoke gráfico opt-in, logs persistentes com rotação e launcher via systemd de usuário.
- Distribuição: pacotes Linux x86_64/aarch64, macOS Apple Silicon/Intel e Windows x64, com testes e compilação em GitHub Actions, checksums e notas versionadas. Comando `./release vX.Y.Z` para publicar versões futuras.

### Corrigido

- Threads Linux deixaram de aparecer como processos independentes e de multiplicar a RAM nas somas. A contagem inclui a thread principal.
- Memória privada deixou de ser uma cópia do RSS. Espaço virtual por processo foi separado do commit global (`Committed_AS`/`CommitLimit`), que pode exceder o limite de overcommit.
- Métricas inacessíveis aparecem como `—`; somas de memória incompletas recebem `≥`.
- Ocultar uma janela sob NVIDIA EGL/Wayland podia bloquear o loop de eventos. O VSync explícito foi desativado no Linux para corrigir o cenário reproduzido.
- Utilização de disco considera o dispositivo mais ocupado, com tratamento de reset e hotplug, em vez de somar percentuais de vários discos.
- Hyprland, UWSM e componentes essenciais da sessão passaram a ter proteção contra encerramento.
- Sensores ACPI genéricos deixaram de ser classificados automaticamente como CPU.
- Nome do pacote Windows alinhado ao instalador: `RamDog-windows-x64.zip`. Pacotes Linux incluem o launcher, que respeita o destino de instalação e permite execução direta sem systemd de usuário.

### Alterado

- Coletas externas Linux executam em segundo plano com timeout; métricas GPU vencidas são descartadas.
- Partida no Windows separa o que sobe com o PC, o que não sobe e entradas quebradas, com agrupamento por fase e estado de execução separado.
- Instalação Unix verifica SHA-256 e aceita versão fixa, destino próprio e instalação sem abrir o aplicativo.

### Limitações

- Telas e uso em primeiro plano no Linux dependem de Hyprland; outros desktops continuam com os recursos independentes do compositor.
- Gestão de serviços exige systemd; ações administrativas pedem autenticação.
- PWM exige driver compatível, não controla fans da GPU e não instala drivers de kernel. A restauração depende de o helper continuar executando; SIGKILL do helper ou perda de energia não permitem limpeza.
- Integridade via pacman compara o arquivo com o banco local; não é assinatura digital nem valida a autenticidade desse banco.
- Binários Linux: glibc 2.39+. Windows: runtime .NET 8 para o helper térmico. macOS mantém seu conjunto limitado de recursos.

Patch notes: [v0.9.0](docs/releases/v0.9.0.md).

## [0.8.0] - 2026-08-28

- Windows: pedido de elevação embutido no executável release, por qualquer forma de abertura, usando `highestAvailable`.
- Contas sem elevação continuam abrindo o app com funcionalidades limitadas. Testes debug não recebem o manifesto de elevação.

O histórico anterior está nas [releases do GitHub](https://github.com/LucasOl1337/RamDog/releases).

[0.9.0]: https://github.com/LucasOl1337/RamDog/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/LucasOl1337/RamDog/releases/tag/v0.8.0
