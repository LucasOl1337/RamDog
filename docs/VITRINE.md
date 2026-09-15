# Produção da vitrine — 15 de setembro de 2026

A página `index.html` usa capturas do aplicativo real no Linux, gravadas numa instância exclusiva do RamDog dentro da bancada X11 `ramdog-promo` (1600 × 1000). Os clientes das janelas foram recortados para 1520 × 900. A demonstração é a **v0.10.0** (interface nova, addon Limpeza). O botão de download aponta para a release estável publicada; esta PR não cria release nova nem liga GitHub Pages.

A instância da bancada usou `XDG_CONFIG_HOME` e `XDG_STATE_HOME` próprios em `/tmp/ramdog-vitrine-x1`. O RamDog da sessão humana (`DISPLAY=:0`) não foi tocado. Só a árvore de demonstração `ramdog-demo` / `ramdog-worker` foi encerrada; os filhos com lock sobreviveram, como o app anuncia.

## Evidência e medidas

- Vídeo H.264/AAC: 1920 × 1080 no master (`~/Videos/ramdog-promo-1080p.mp4`) e 1280 × 720 na página; 30 fps; duração **85,800 s** (86 s na página). Conferido por ffprobe; decodificação completa sem erro. Folha de contato dos segmentos inspecionada.
- Narração: OmniVoice Studio local, perfil `04b04ba5` (Lucas Oliveira, OFICIAL #1, Studio Clean v2), resolvido em `GET http://localhost:3900/api/mcp/default` no momento da geração e presente em `/v1/audio/voices`. Watermark invisível desligado. 48 kHz estéreo, loudnorm I=-16 TP=-1.5 LRA=11. Sem edge-tts.
- Binário Linux x86_64 da revisão demonstrada, `cargo build --locked --release`: **7.699.736 bytes (7,34 MiB)**. Tamanho em disco, não uso de RAM.
- Encerramento de demonstração: pai `ramdog-demo` finalizado pelo RamDog; dois `ramdog-worker` com lock permaneceram. Removidos ao encerrar a bancada.
- As métricas nas telas são leituras da máquina durante trabalho concorrente. Não são benchmarks nem memória economizada.
- Partida e Desperdício listam o inventário acessível. O aviso “Sessão de usuário do systemd indisponível” é o isolamento da bancada (sem barramento de usuário); serviços de sistema e autostart XDG continuam listados. Telas mostra o limite real: o backend Linux precisa de Hyprland e não opera no X11 da bancada.
- Página conferida em 1440 px e 390 px por captura headless: imagens carregadas, âncoras válidas.

## Fontes das afirmações

Código, [README](../README.md), [documentação Linux](../linux/README.md), [changelog](../CHANGELOG.md), [patch notes v0.10.0](releases/v0.10.0.md) e [licença MIT](../LICENSE).

Fontes externas consultadas em 15/09/2026:

- [Microsoft — Task Manager](https://learn.microsoft.com/en-us/troubleshoot/windows-server/support-tools/support-tools-task-manager)
- [Microsoft — configuração de aplicativos de inicialização](https://support.microsoft.com/en-us/windows/experience/startup-boot/configure-startup-applications-in-windows)
- [htop — manual](https://github.com/htop-dev/htop/blob/main/htop.1.in)
- [htop — projeto](https://htop.dev/)

Os números de CPU/RAM e a disponibilidade de sensores variam entre máquinas. `—` significa indisponível; o lock protege contra as ações do próprio RamDog.
