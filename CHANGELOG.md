# Changelog

As mudanças são registradas por versão. As notas descrevem funcionalidades disponíveis e suas limitações; testes de hardware não equivalem a cobertura de todos os drivers e desktops.

## [0.12.1] - 2026-09-22

Patch de manutenção interna da tabela, sem novas funções de interface ou ganho de desempenho medido. Preserva os recursos e os idiomas da v0.12.0.

Patch notes: [v0.12.1](docs/releases/v0.12.1.md).

### Manutenção

- Cache de linhas e consultas de processos separados da interface em módulos próprios, preservando filtros, ordenação e atualização sob o ponteiro.
- 22 testes do modelo da tabela cobrem transições do cache, reconciliação de grupos, busca, limites de recursos, métricas e os 12 critérios de ordenação. Substituem cinco testes anteriores e elevam a suíte de 84 para 101 testes aprovados no Linux, com três integrações opt-in ignoradas.

## [0.12.0] - 2026-09-22

O RamDog agora oferece a interface completa em inglês, preservando o português como padrão e todos os recursos das versões 0.11.x.

Patch notes: [v0.12.0](docs/releases/v0.12.0.md).

### Adicionado

- Seletor persistido **Português / English** nas Preferências. Configurações existentes continuam em português e mantêm locks, categorias, presets e os demais ajustes.
- [`README.en.md`](README.en.md) completo, com instalação, recursos, plataformas, limites e terminologia da interface.

### Melhorado

- Lista, Árvore, Categorias, Partida, Desperdício, Térmico, Telas e Limpeza, além de busca, filtros, confirmações, estados e erros, seguem o idioma escolhido.
- Recursos das versões 0.11.x também foram localizados: **Contention**, **Launched by**, contabilidade de CPU/RAM e tratamento de zumbis.
- Vitrine pública atualizada com capturas da interface atual, demonstração da Limpeza e vídeo narrado.

### Corrigido

- Partida e Desperdício explicam no idioma escolhido quando o barramento systemd de usuário não está disponível, sem expor a saída crua do `systemctl`.

## [0.11.1] - 2026-09-21

Finalizar deixa de bater em parede quando o pai não recolhe, e a última coluna responde "quem abriu isso" em vez de despejar argumentos.

### Adicionado

- Coluna **Quem abriu** no lugar de **Comando**: a cadeia de quem chamou quem, da raiz até o pai (`foot › bash › claude`, `Hermes (gateway) › python3`, `agent-bench · padrao › python3`). Compositor e `systemd` ficam de fora como raiz; unidade do systemd, agente e host deduzidos do ambiente entram na frente quando a cadeia não os mostra. Clique na célula seleciona o pai; o hover traz a cadeia com PIDs e a linha de comando inteira. Botão direito no título troca de volta para **Comando** (persistido).
- Painel de detalhes de um zumbi: diz que já morreu e não ocupa memória, quem está segurando, e dá os dois botões que existem de verdade: **Pedir ao pai para recolher** e **Finalizar pai: nome (PID)**. O menu de contexto ganha o mesmo item.

### Corrigido

- Finalizar um processo cujo pai não chama `wait` (script Python com `Popen` sem `wait`, ponte que abandona o filho) dizia `1 finalizado(s)` e a linha ficava para sempre; o clique seguinte caía num aviso de zumbi que só citava o Shift. Agora o RamDog espera o kernel recolher (até 400 ms), cutuca o pai com SIGCHLD e, se a linha ficar, diz na hora quem está segurando e que não há memória presa. Processo que já era zumbi na lista (árvore ou app agrupado) nem recebe sinal: vai direto para esse acerto.
- Zumbi reparentado depois que o pai original morreu era atribuído ao pai da amostra antiga; o pai agora é lido do kernel no clique.
- Linux: a última coluna da tabela não acompanhava a janela ao maximizar (o egui_extras congela a largura do "resto" quando a coluna é redimensionável). Agora ela acompanha; a borda da coluna Tempo continua arrastável.

## [0.11.0] - 2026-09-19

A conta do CPU fecha, a faixa Disputa aponta quem está atrapalhando o jogo ou a máquina, e a origem reconhece serviços do systemd.

Patch notes: [v0.11.0](docs/releases/v0.11.0.md).

### Melhorado

- A tabela reutiliza os cálculos entre amostras e aplica a ordenação pendente ao sair com o mouse, evitando recalcular todos os processos a cada evento visual.

### Adicionado

- Chip **GPU no CPU** na Disputa: navegador de agente que caiu em renderização por software (`swiftshader`) e queima núcleos desenhando enquanto a GPU real fica parada. Sobe acima de sobra no ranking.
- Linux/macOS: load 1/5/15 min no card de CPU e swap usado no card de Memória. Load acima do número de núcleos e swap ≥ 1 GB acendem o card.
- Faixa **Disputa** quando um jogo está aberto com GPU ociosa, load/swap altos, emulador Android sem janela, `git-credential`/`gh` em loop ou processo que come núcleo com pouca RAM. O botão **Ver disputa** ordena por isso e seleciona o primeiro.
- Chip **Disputa** na fileira de categorias e item no menu de contexto. Com a Disputa ligada a coluna CPU mostra os núcleos equivalentes em destaque (`14×`) e o % da máquina menor ao lado; desligada, volta ao % só.
- Linux: origem lida também da unidade do systemd (cgroup). Processo que veio de `uwsm app`, `systemd-run` ou de um serviço chega sem variáveis de ambiente e com o `systemd --user` como pai; agora aparece `Hermes (gateway)`, `agent-bench · projeto`, `Claude Desktop`, `desktop (navegador)` em vez de `systemd`. O nome cru da unidade fica no hover.

### Corrigido

- Linux: sem barramento de usuário do systemd, Partida e Desperdício avisam em português em vez de despejar o erro cru do `systemctl`.
- O medidor de CPU do topo dizia 28% e a lista, somada, não passava de 8%. Dois motivos: processo que nascia depois da amostra anterior entrava com 0,0% e só ganhava número na amostra seguinte, se ainda estivesse vivo (um `rg`, `git` ou `cc` de 3 s a 100% de um núcleo nunca aparecia); e filho que nascia e morria dentro da janela sumia sem deixar rastro. Agora processo novo entra com a taxa desde que nasceu (Linux e Windows), e no Linux o pai mostra `+N%` na coluna CPU com o que os filhos já encerrados gastaram (`cutime`/`cstime`, limitado ao que o medidor não explica). Quando ainda sobra diferença (interrupções, kernel sem PID), o card de CPU mostra o chip **fora da lista N%** com a conta no hover, em vez de deixar a soma não fechar em silêncio.
- Zumbi não "finalizava": o kernel aceita SIGTERM/SIGKILL num processo `Z` e o RamDog contava como `1 finalizado(s), ~0,0 MB` sem nada mudar. Agora o ✖ num zumbi pede ao pai para recolher (SIGCHLD) e diz quem segura; Shift+✖ finaliza o pai.
- Linux: o cliente Steam relançado por um atalho (`steam steam://rungameid/2357570`) era rotulado como o jogo (`Overwatch 2 · Steam / Proton`) horas depois de o jogo fechar. Agora é `Steam`.
- Linux: ícone do binário escolhido pelo primeiro `.desktop` que o `read_dir` devolvesse. `Counter-Strike 2.desktop` (`Exec=steam steam://rungameid/730`) dava o ícone do CS ao cliente Steam. O `.desktop` com o nome do programa ganha; atalho com URL perde.

## [0.10.0] - 2026-09-11

Interface nova, addon Limpeza, identidade da tarefa, GPU/VRAM na lista e encerramento que não mente. Inclui tudo da 0.9.1, que não chegou a ser publicada.

Patch notes: [v0.10.0](docs/releases/v0.10.0.md).

### Corrigido

- Linux: Overwatch/Proton deixa de aparecer como um `wine64` só. A chave do grupo é Steam appid / prefixo Wine / projeto / agente, não o path do runtime.
- Encerrar: ESRCH vira “já tinham saído”, não falha vermelha. Clique em PID morto avisa em vez de silenciar. F5 e o próprio kill descongelam a tabela. Grupo recolhido que cai de 2 para 1 PIDs mostra o sobrevivente.
- Coluna Comando de processos Wine deixa de cortar no primeiro `.exe` (não esconde mais `Overwatch.exe`).
- GPU `–` deixa de significar zero: ausência de leitura do driver é distinta de carga ~0%. A soma do grupo usa o máximo entre PIDs, não a soma das cargas.

### Adicionado

- Interface nova, no espírito do libadwaita: barra lateral com visões e addons (sai a fileira de abas e o bloco de botões do topo), cabeçalho com busca e agrupamento, quatro cards de recurso com gráfico dos últimos 90 s, tabela de linhas duplas (ícone ou inicial, nome, "PID · categoria · origem" e chip de estado), filtros de corte e métrica de RAM numa janela de Preferências. Paleta em cinzas neutros (1e1e1e / 242424 / 2b2b2b) com o azul do GNOME; no Linux usa Adwaita Sans/Mono quando instaladas. As colunas Cat., PID, Estado e Origem viraram texto na linha; ordenar por elas fica no menu de contexto. Partida, Desperdício, Térmico, Telas e Limpeza usam o mesmo vocabulário (botões em pílula, linhas em caixa, badges de estado) via `src/kit.rs`.
- Linux: addon **Limpeza**. RAM (`/proc/meminfo`, soltar cache do kernel), apps acima de um corte de RAM para marcar e encerrar de uma vez (sobras e zombies primeiro, janela aberta por último, mesmo lock da lista) e disco: subpastas de `~/.cache`, lixeira, cache do pacman, journal, coredumps e pacotes órfãos, cada um com confirmação inline. O que é do sistema passa por `pkexec ramdog --clean-helper <op>`; o helper só conhece operações fixas e recalcula os alvos, sem receber caminho por argumento.
- Colunas VRAM e Estado (em foco / janela / fundo / leftover / zombie). Leftover só com evidência (zombie, emulador `-qt-hide-window`).
- Filtros “ocultar abaixo de” para CPU, GPU e VRAM, independentes da RAM. Com Agrupar por app, o corte vale no total do app.
- Origem Steam/Proton, projeto (venv) e emulador Android nomeado pelo AVD.

### Alterado

- Largura mínima da janela completa passa de 900 para 1000 px por causa da barra lateral. O modo Mini não muda.

## [0.9.1] - 2026-09-08

A sessão longa no Linux — em particular no [Omarchy](https://omarchy.org/) com Hyprland — deixa de degradar. O sampler lê `/proc` direto, o agrupamento de agentes cabe numa linha e o Telas no Hyprland 0.55+ deixa a janela no estado pedido.

Patch notes: [v0.9.1](docs/releases/v0.9.1.md). Publicada junto com a 0.10.0.

### Corrigido

- Linux: o sampler não indexa mais cada thread como processo nem mantém `/proc/*/stat` aberto. Isso vazava milhares de descritores, queimava CPU e fazia a lista/RAM/CPU mentirem depois de algumas horas.
- Linux: descritores de arquivo voltam a aparecer na ficha do processo; a coluna CPU usa a mesma média móvel de 1 s do Windows, em % da máquina (100% = todos os núcleos), mesmo quando o próprio RamDog tem afinidade restrita.
- Linux: USS/PSS deixam de ser relidos em todos os processos a cada amostra (teto por ciclo, cache de 5 s, fila que prioriza o que nunca foi tentado). Sem smaps, o privado cai na estimativa `RSS − shared` de `statm`, não em zero. Tentativas com erro respeitam o intervalo do cache; processos sem permissão não bloqueiam os demais; leituras ausentes ou vencidas não aparecem como zero.
- Linux: a varredura DRM por processo lê só descritores `/dev/dri`, ignora NVIDIA (já coberta pelo `nvidia-smi pmon`) e não percorre o `fdinfo` inteiro de cada PID. Sem placa AMD/Intel, essa varredura nem começa. O worker de GPU espera 1,5 s entre coletas.
- Hyprland 0.55+: `setfloating` / `settiled` usam `enable` / `disable`. Ações que o compositor não reconhece viravam *toggle*, então o segundo encaixe podia devolver a janela ao tiling.
- Mini: ao voltar para a janela inteira, o teto de tamanho do modo mini é removido (`MaxInnerSize` infinito). No X11, só `Resizable` não bastava.

### Alterado

- Lista: famílias reconhecidas (Claude, Codex, Grok, ChatGPT, Cursor, Gemini, Hermes, Maestri, OpenCode) viram uma linha só, inclusive com instalações versionadas e o `node` lançado pelo agente. O grupo começa recolhido; clica para ver os PIDs. `node`/`python` de venvs diferentes continuam separados.
- Agrupamento: executáveis fora das famílias reconhecidas ficam separados por caminho, preservando maiúsculas/minúsculas no Unix; o botão de encerrar um grupo não mistura binários homônimos de projetos diferentes.

### Documentação

- Vitrine no GitHub Pages com vídeo narrado e legendado, oito capturas reais, comparação documentada e instruções por sistema.
- README, changelog e patch notes passam a tratar o Omarchy como alvo de primeira classe no Linux (Hyprland, UWSM, Quickshell, pacman, NVIDIA/AMD).

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

[0.10.0]: https://github.com/LucasOl1337/RamDog/compare/v0.9.0...v0.10.0
[0.9.1]: https://github.com/LucasOl1337/RamDog/compare/v0.9.0...v0.10.0
[0.9.0]: https://github.com/LucasOl1337/RamDog/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/LucasOl1337/RamDog/releases/tag/v0.8.0
