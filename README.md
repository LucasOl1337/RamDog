<p align="center">
  <img src="assets/ramdog-256.png" width="128" height="128" alt="RamDog">
</p>

<h1 align="center">RamDog</h1>

<p align="center">
  Gerenciador de processos para Windows e macOS: origem, categorias, kill de árvore.
</p>

<p align="center">
  <img alt="Windows x64" src="https://img.shields.io/badge/Windows-x64-0B3A4A?logo=windows&logoColor=4FC3F7">
  <img alt="macOS" src="https://img.shields.io/badge/macOS-arm64%20%7C%20x86_64-111111?logo=apple&logoColor=white">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-000000?logo=rust&logoColor=white">
  <img alt="MIT" src="https://img.shields.io/badge/license-MIT-69F0AE">
</p>

<img src="docs/screenshot-main.png" alt="Janela do RamDog em visão Lista: medidores de CPU, RAM, GPU e Disco no topo, chips de categoria (IA/Agentes, Dev, Navegador, Jogos, Pessoal, Sistema, Outros) e tabela de processos ordenada por RAM — Memory Compression, vmmemWSL, Brave, Claude e o restante." width="100%">

## Por que existe

O Gerenciador de Tarefas não mostra a cadeia de origem — quem lançou o processo. Não classifica IA, Dev ou Navegador. Não mata a árvore inteira com lock no que não pode cair. Não trata os ralos do Windows.

Este app existe por isso.

## O que faz

- **Origem / lançado por.** Coluna Origem = primeiro ancestral vivo que não seja host genérico (`cmd`, `bash`, `node`…). Quando a cadeia de pais morreu, o RamDog lê o ambiente herdado e mostra em roxo o agente (Claude Code + sessão + PID, Codex, Cursor Agent, Gemini CLI…) e o host (Maestri, VS Code, Cursor, Windows Terminal…), além de `npm run <script>` no projeto.
- **Categorias.** IA / Agentes, Dev, Navegador, Jogos, Pessoal, Sistema, Outros — regra automática, com override manual por processo.
- **Kill, árvore e lock.** Finaliza o processo ou a árvore (processo + filhos). Lock impede que o RamDog encerre o protegido. Processos críticos do SO (`System`/`csrss`/`dwm` no Windows; `kernel_task`/`launchd`/`WindowServer` no macOS) são sempre protegidos.
- **Visões.** Lista (plana), Árvore (pai → filhos, RAM da subárvore), Categorias (agrupado). **Ralos** só no Windows (Defender, serviços, Appx, inicialização).
- **Medidores.** CPU e RAM no topo nos dois SOs. GPU NVIDIA (NVML) e % de disco estilo Task Manager só no Windows.

## Instalar

**Windows x64** (PowerShell):

```powershell
irm https://raw.githubusercontent.com/LucasOl1337/RamDog/main/install.ps1 | iex
```

Depois: `ramdog` no terminal, ou o atalho **RamDog** na área de trabalho (UAC = temperatura da CPU).

**macOS** (Apple Silicon ou Intel):

```bash
curl -sSfL https://raw.githubusercontent.com/LucasOl1337/RamDog/main/install.sh | sh
```

Baixa o tar do [release](https://github.com/LucasOl1337/RamDog/releases) (`RamDog-macos-aarch64.tar.gz` ou `…-x86_64.tar.gz`) para `~/.local/bin/ramdog` e abre. Se o Gatekeeper bloquear: Ajustes → Privacidade e segurança → Abrir mesmo assim.

No Mac: lista, categorias, origem, árvore, kill. Sem ralos, sem temp de CPU, sem GPU NVML. Sem binário no release, o script cai no `cargo build` (precisa [rustup](https://rustup.rs) + git).

[Release](https://github.com/LucasOl1337/RamDog/releases) com zip (Windows) e tar (macOS), se preferir baixar na mão.

Do código:

```bash
git clone https://github.com/LucasOl1337/RamDog.git
cd RamDog
cargo build --release
```

Windows, helper de temperatura (opcional, [SDK .NET 8](https://dotnet.microsoft.com/download/dotnet/8.0)):

```bash
dotnet publish hwtemp -c Release -o target/release --no-self-contained
```

## Uso

| Ação | Como |
|---|---|
| Finalizar processo | ✖ na linha, `Del`, botão direito → Finalizar, ou painel inferior |
| Finalizar árvore (processo + filhos) | `Shift+Del`, `Shift`+✖, botão direito → Finalizar árvore, ou painel inferior |
| Proteger / desproteger | 🔒/🔓 na linha, botão direito, ou painel inferior. Protegidos nunca são finalizados pelo RamDog |
| Categoria manual | botão direito → Categoria, ou combo no painel inferior (`auto` volta à regra automática) |
| Visões | **Lista**, **Árvore**, **Categorias**. **Ralos** só no Windows |
| Filtro | busca por nome / PID / comando; chips de categoria (clique alterna, duplo clique isola); `min. MB` |
| Origem | coluna *Origem* = primeiro ancestral vivo que não seja host genérico (cmd, bash, node...); cadeia completa clicável no painel inferior (`Ir para o pai`) |
| Lançado por | quando a cadeia de pais morreu (ou só tem hosts genéricos), o RamDog lê as variáveis de ambiente herdadas do processo e mostra em roxo quem o originou: agente (Claude Code + sessão + PID, Codex, Cursor Agent, Gemini CLI...) e host (Maestri, VS Code, Cursor, Windows Terminal...), além de `npm run <script>` em `<projeto>` |
| Atualização | 0,5–5 s, **Pausar**, `F5` força |

Enquanto o mouse está sobre a tabela a ordem das linhas fica **congelada** (status "ordem congelada"), para o clique em ✖ nunca cair numa linha que acabou de trocar de lugar. Com **Confirmar kill** ligado, todo encerramento pede confirmação (`Enter` confirma, `Esc` cancela) e lista os processos afetados.

Aba **Ralos** (Windows): leitura direta (SCM + registro, sem PowerShell). **Ação** usa Win32 direto quando o RamDog já está elevado; senão dispara um PowerShell elevado — **um prompt de UAC por ação**. No macOS a aba existe só para dizer isso.

| Seção | O que dá para fazer | Reversível? |
|---|---|---|
| **Microsoft Defender** | Excluir pastas de projeto/agentes da varredura em tempo real (`Add-MpPreference -ExclusionPath`); limitar a CPU da varredura agendada para 5/10/20% (`-ScanAvgCPULoadFactor`); pausar/reativar a proteção em tempo real | Sim, tudo |
| **Serviços dispensáveis** | **Parar** (só agora) ou **Desativar** (não inicia mais) — WSearch, SysMain, DiagTrack, DoSvc, WerSvc, MapsBroker, PhoneSvc, Xbox*, lfsvc, RemoteRegistry, Fax. `wuauserv` é só *parar* (o Windows o religa sozinho) | Sim, botão **Reativar** |
| **Apps de sistema (Appx)** | Remover pacotes de sistema que você não usa | Reinstalável pela Store |
| **Inicialização** | Ligar/desligar entradas do `Run` (HKCU e HKLM) e **remover** de vez; **Finalizar** o processo se já estiver rodando | Ligar/desligar sim; remover, não |

## Limites

**Os dois SOs**

- Processos de outros usuários: no Windows, **Reabrir como admin**; no macOS, `sudo ramdog` se precisar. A UI nunca inventa número — GPU/temp/disco ausentes aparecem "–".
- Configuração: Windows `%APPDATA%\RamDog\config.json`; macOS `~/Library/Application Support/RamDog/config.json`.

**Só Windows**

- Ralos (Defender, serviços, Appx, inicialização).
- Temperatura de CPU/RAM: helper `hwtemp.exe` (LibreHardwareMonitor), só elevado. Sem helper/admin/sensor, "–".
- GPU no topo e na tabela: **NVIDIA** (`nvml.dll`). Sem driver, "–".
- `MsMpEng.exe` é processo protegido pelo kernel: a seção Defender só reduz o trabalho dele, não mata. Com Tamper Protection ligada, pausar tempo real não pega.

**Só macOS**

- Sem ralos, sem temp de CPU, sem NVML. Disco no topo não tem % idle estilo Task Manager.
- Gatekeeper pode barrar o binário no primeiro open.

## Como mede

**Windows:** RAM = *Private Working Set* (`NtQuerySystemInformation`, mesma coluna Memória do Gerenciador de Tarefas). CPU no topo = `GetSystemTimes`. GPU = NVML + PDH `\GPU Engine(*)\Utilization Percentage` (máximo entre engines do PID). Disco no topo = PDH `% Idle Time` + bytes/s (`PdhAddEnglishCounterW`, nomes em inglês). `hwtemp.exe` lê Tctl/Tdie e DIMM.

**macOS:** processos, RAM (RSS) e CPU via [sysinfo](https://crates.io/crates/sysinfo). Disco por processo = bytes lidos+escritos/s. Sem PDH, sem NVML, sem LibreHardwareMonitor.

## Licença

[MIT](LICENSE).

## Também

[**TempHUD**](https://github.com/LucasOl1337/TempHUD) — overlay térmico para Windows.
