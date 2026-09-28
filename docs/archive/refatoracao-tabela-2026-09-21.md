# Refatoração do modelo da tabela e cache

Data: 2026-09-21 (America/Sao_Paulo), validação concluída em 2026-09-22 UTC.

## Baseline e escopo

- HEAD inicial: `e4e8ac8db1b462904742a1071dae4f11ca1167bb`, branch `main`, checkout limpo.
- Políticas: `/home/lol/AGENTS.md`, nenhuma `AGENTS.md` adicional no repositório.
- Baseline real: `CARGO_BUILD_JOBS=2 cargo test --locked -- --test-threads=1`, **84 passed, 3 ignored, 0 failed**.
- Nenhum reset, clean, stash, troca de branch, subagente, push, deploy ou release.

## Fronteira implementada

| Arquivo | Antes | Depois | Responsabilidade |
|---|---:|---:|---|
| `src/app.rs` | 7.137 | 6.832 | Integração de snapshot/UI, construção de linhas, grupos e desenho |
| `src/app/table/mod.rs` | inexistente | 64 | Vocabulário `Row`, `SysRow`, `SortKey` e interface interna |
| `src/app/table/cache.rs` | inexistente | 114 | Estado e protocolo de invalidação, rebuild, hover e reconciliação |
| `src/app/table/query.rs` | inexistente | 216 | Consulta pura de snapshot, filtros, contagem, métrica e ordenação |
| `src/app/table/tests.rs` | inexistente | 511 | 22 testes contra os módulos de produção |

O benefício não é reduzir linhas totais. Sete campos interdependentes do cache antes expostos em `App` passam a ser privados de `RowCache`. A UI sinaliza `snapshot_changed`, `invalidate`, `clear` e `thaw`, consulta `action` e entrega o resultado de rebuild/reconciliação. O módulo não acessa egui, relógio, sampler nem ações do sistema.

`ProcessQuery` recebe referências ao snapshot já indexado, categorias, subárvores, métrica e filtros. O critério de disputa recebe a função de ranking do chamador, preservando a leitura de idade fora do núcleo puro. Ordenação e filtros não precisam mais construir `App`, carregar configuração, iniciar threads ou abrir janela para serem testados.

Continuam em `App`: construção List/Tree/Category, chave de cache, detecção geométrica de hover, expansão de ancestrais, agregação/atualização/ordenação de grupos, internacionalização e apresentação. As strings e os algoritmos de decisão existentes foram preservados. A integração de `after_kill` apenas substitui três atribuições do cache por `clear()`, sem modificar solicitação/execução de kill.

## Evidências de regressão

Os cinco testes antigos da decisão booleana foram substituídos por testes stateful equivalentes, mais 17 novos testes. A suite total passou de **84 para 101 aprovados**, mantendo os três ignorados.

- Cache inicial e vazio, repaint estável, hover sem dados novos, snapshot com e sem hover, saída do congelamento uma única vez.
- Mudança explícita de chave e invalidação vencem hover/snapshot pendente. `clear` e `thaw` preservam seus efeitos distintos.
- Snapshots sucessivos removem processos mortos/filtrados sem inserir novos nem reordenar sob o ponteiro.
- Contagem de hits permanece separada de ancestrais visíveis. Metadados da árvore e linhas sintéticas/categorias permanecem intactos na reconciliação.
- Grupos preservados, reduzidos para um processo ou removidos, incluindo sequência 2→1→0 e índice de grupo ausente.
- Limites inclusivos de RAM/CPU/GPU/VRAM, leituras GPU/VRAM ausentes, categoria desconhecida, busca normalizada e PID exato.
- Agrupamento adia corte de recursos para o agregado, mas continua aplicando categoria/busca.
- Todos os 12 critérios de ordenação em ambas as direções, desempate PID após inversão nos critérios numéricos e exceção histórica de Name, CPU de filhos encerrados, totais da árvore, GPU ausente e NaN.
- Private/WorkingSet/Commit e Proportional Linux, inclusive PSS indisponível.
- Fluxo integrado Query→Cache: snapshot com mudanças de consumo, hit novo aguardando descongelamento, remoção pelo filtro, rebuild ordenado e mudança explícita de busca.

Também houve comparação automática contra o HEAD: seção `request_kill` até `after_kill`, apresentação de `SysRow` e `rows_key` byte-idênticas. Comparador preservado salvo a injeção de `steal_rank` e formatação. Nenhuma dependência ou arquivo Cargo alterado.

## Comandos executados

Todos os comandos Cargo de compilação/teste usaram `CARGO_BUILD_JOBS=2`.

| Comando | Resultado |
|---|---|
| `cargo test --locked -- --test-threads=1` baseline | 84 passed, 3 ignored |
| Mesmo comando após extração e regressões | 101 passed, 3 ignored, 0 failed |
| `cargo check --locked` | exit 0 |
| `cargo build --locked` | exit 0, binário debug real |
| `cargo fmt -- --check` | exit 0 |
| `git diff --check` | exit 0 |

Toolchain local: rustc 1.98.0, cargo 1.98.0, Linux x86_64. Warnings preexistentes de imports/dead code continuam, sem correções fora do escopo. Check/build reportaram 54 warnings. Houve uma falha intermediária de importação de `Category` ao criar o módulo, corrigida antes de todas as validações finais.

## Smoke da aplicação real

- Skill de coexistência e guia da bancada lidos antes da operação visual.
- `bench_ensure`: bancada exclusiva `refactor-ramdog`, workspace **9**, display **:83**, controle agente, input físico desativado, nenhum navegador iniciado.
- Binário: `target/debug/ramdog --smoke-test`, PID 2310321, propriedade conferida em cgroup `agent-bench@refactor-ramdog.service`.
- Execução exclusivamente por `agent-bench launch`, com `XDG_CONFIG_HOME` e `XDG_STATE_HOME` apontando para `target/refactor-smoke/{config,state}`. Isso também isolou `usage.json`.
- Modo oficial inspecionado antes da execução: alterna abas/métricas/mini e seleciona o próprio processo, sem cliques nem execução de kill/limpeza/startup. O modo suprime salvamento de configuração. Scans e amostragem continuam passivos.
- Captura real inspecionada em `target/refactor-smoke/smoke.png`, tabela e detalhes renderizados.
- Log: `target/refactor-smoke/state/RamDog/ramdog.log`, de `1790043758 iniciando` até `1790043848 SMOKE PASS: 90s, todas as abas, mini e métricas`, seguido de `saída: Ok(())`.
- Processo encerrou sozinho. Bancada própria encerrada em seguida, sem trabalho pendente. Desktop humano e demais bancadas não foram operados.

Logs de testes/check/build ficam em `target/refactor-{tests-final,check,build}.log`. Artefatos de validação permanecem locais e ignorados pelo Git.

## Limites e riscos restantes

- Apenas Linux nativo foi compilado/executado. Windows e macOS requerem a matriz CI existente, não executada nesta rodada. `rustup` não está instalado localmente. Nenhuma instalação/cross-toolchain foi tentada.
- O smoke oficial invalida a tabela a cada frame. Demonstra integração/renderização e ausência de panic nos ciclos, mas não comprova por si só cache-hit/hover. Esses contratos são exercitados pelos testes stateful reais.
- Sem benchmark de desempenho ou comparação visual pixel a pixel. Não há alegação de ganho de velocidade.
- Construção/ordenação dos grupos, agregados, ancestrais e cabeçalhos continuam em `App`, com comportamento histórico preservado. O estado congelado ainda conserva cabeçalhos/linhas sintéticas e não insere processos novos até reconstruir.
- Kill, ações privilegiadas, limpeza, startup, permissões e mudança de idioma não foram acionados. Os três testes ignorados do projeto permanecem ignorados.
