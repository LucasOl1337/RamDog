//! Catálogo do que cada processo do Windows faz, por que está aberto e se dá para matar.
//!
//! Existe porque a pergunta que o Gerenciador de Tarefas nunca responde é a única que
//! importa: "não sei o que diabos isso faz, posso fechar?". Sem essa resposta o usuário
//! ou mata algo essencial ou deixa lixo rodando por medo.
//!
//! O texto é curto de propósito — cabe numa linha do painel de detalhes. Nada aqui é
//! consultado por amostra; é uma tabela estática, custo zero em tempo de execução.

/// O que acontece se o processo for encerrado.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Risk {
    /// Encerrar é seguro: no máximo você perde o que estava fazendo nele.
    Safe,
    /// O Windows reabre sozinho em segundos — matar quase nunca resolve nada.
    Respawns,
    /// Derruba a sessão ou o sistema inteiro (tela azul / logoff forçado).
    Fatal,
}

impl Risk {
    pub fn label(self) -> &'static str {
        match self {
            Risk::Safe => "seguro encerrar",
            Risk::Respawns => "reabre sozinho",
            Risk::Fatal => "NÃO encerrar",
        }
    }

    pub fn dot(self) -> &'static str {
        match self {
            Risk::Safe => "🟢",
            Risk::Respawns => "🟡",
            Risk::Fatal => "🔴",
        }
    }

    pub fn color(self) -> egui::Color32 {
        match self {
            Risk::Safe => egui::Color32::from_rgb(90, 220, 130),
            Risk::Respawns => egui::Color32::from_rgb(230, 190, 80),
            Risk::Fatal => egui::Color32::from_rgb(235, 90, 90),
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            Risk::Safe => "Encerrar não quebra o Windows. Você só perde o trabalho não salvo desse programa.",
            Risk::Respawns => "O Windows reinicia este processo automaticamente. Matar libera a RAM por alguns segundos e ele volta.",
            Risk::Fatal => "Processo crítico: encerrar causa tela azul (CRITICAL_PROCESS_DIED) ou derruba sua sessão na hora.",
        }
    }
}

pub struct Known {
    /// O que o processo faz, em uma frase.
    pub what: &'static str,
    /// Por que ele está aberto agora — a pergunta que ninguém responde.
    pub why: &'static str,
    pub risk: Risk,
}

const fn k(what: &'static str, why: &'static str, risk: Risk) -> Known {
    Known { what, why, risk }
}

/// Ficha do processo pelo nome do executável (minúsculo, com ou sem `.exe`).
pub fn lookup(name_lower: &str) -> Option<Known> {
    let b = name_lower.strip_suffix(".exe").unwrap_or(name_lower);
    Some(match b {
        // ── Núcleo da sessão: matar qualquer um destes derruba o Windows ──────────────
        "system" => k(
            "O próprio kernel do Windows e os drivers, agrupados num processo fictício.",
            "Existe desde o boot. Não é um programa: é o Windows.",
            Risk::Fatal,
        ),
        "registry" => k(
            "Guarda o Registro do Windows carregado em memória.",
            "Sempre aberto — todo o sistema lê configuração daqui.",
            Risk::Fatal,
        ),
        "memory compression" => k(
            "Comprime páginas de RAM pouco usadas em vez de mandá-las para o disco.",
            "Cresce quando a RAM aperta. É economia, não desperdício.",
            Risk::Fatal,
        ),
        "secure system" | "lsaiso" => k(
            "Isolamento por virtualização (VBS/Credential Guard) — protege credenciais do resto do sistema.",
            "Ligado porque a Segurança Baseada em Virtualização está ativa nesta máquina.",
            Risk::Fatal,
        ),
        "smss" => k(
            "Gerenciador de Sessões: o primeiro processo de modo usuário do boot.",
            "Cria cada sessão e depois sai — por isso costuma aparecer como pai já encerrado.",
            Risk::Fatal,
        ),
        "csrss" => k(
            "Subsistema de tempo de execução do Win32: console, criação e término de processos.",
            "Um por sessão, desde o login. Sempre haverá pelo menos dois.",
            Risk::Fatal,
        ),
        "wininit" => k(
            "Inicialização da sessão 0: sobe services.exe, lsass.exe e o gerenciador de sessão.",
            "É o avô de todo serviço do Windows. Ele mesmo usa poucos MB — o número grande na árvore é a soma dos filhos.",
            Risk::Fatal,
        ),
        "winlogon" => k(
            "Cuida do login, do bloqueio de tela e do Ctrl+Alt+Del.",
            "Um por sessão interativa, desde que você ligou o PC.",
            Risk::Fatal,
        ),
        "services" => k(
            "Gerenciador de Controle de Serviços: inicia, para e supervisiona todo serviço do Windows.",
            "Pai de quase todo svchost.exe — daí a RAM enorme na visão de árvore.",
            Risk::Fatal,
        ),
        "lsass" => k(
            "Autoridade de Segurança Local: valida senhas, tokens e políticas de segurança.",
            "Sempre aberto. É também o alvo favorito de roubo de credenciais.",
            Risk::Fatal,
        ),
        "fontdrvhost" => k(
            "Hospeda o driver de fontes fora do kernel, isolado por segurança.",
            "Sobe junto com a sessão gráfica.",
            Risk::Fatal,
        ),
        "dwm" => k(
            "Gerenciador de Janelas: compõe tudo o que você vê na tela, com transparência e sombras.",
            "Sem ele não há área de trabalho. A RAM dele é quase toda buffer de vídeo.",
            Risk::Fatal,
        ),
        "logonui" => k(
            "Desenha a tela de login e a de bloqueio.",
            "Aparece quando a sessão está trancada.",
            Risk::Fatal,
        ),

        // ── Reabrem sozinhos ─────────────────────────────────────────────────────────
        "explorer" => k(
            "A área de trabalho, a barra de tarefas e as janelas de pasta.",
            "Também hospeda ícones da bandeja e extensões de shell de outros programas — por isso incha com o tempo.",
            Risk::Respawns,
        ),
        "svchost" => k(
            "Casca genérica que hospeda serviços do Windows: sozinho o nome não diz nada.",
            "O que importa é qual serviço está dentro — o RamDog mostra isso na ficha.",
            Risk::Respawns,
        ),
        "sihost" => k(
            "Infraestrutura do shell: menu de contexto, notificações, ações da barra de tarefas.",
            "Um por sessão de usuário.",
            Risk::Respawns,
        ),
        "ctfmon" => k(
            "Entrada de texto: teclado virtual, idiomas, reconhecimento de escrita.",
            "Sobe assim que qualquer campo de texto existe.",
            Risk::Respawns,
        ),
        "runtimebroker" => k(
            "Fiscaliza as permissões dos aplicativos da Store (câmera, microfone, arquivos).",
            "Um por app moderno aberto. Muitos ao mesmo tempo é normal.",
            Risk::Respawns,
        ),
        "wmiprvse" => k(
            "Provedor WMI: responde consultas de inventário e monitoramento sobre a máquina.",
            "Sobe sob demanda e some sozinho depois de alguns minutos ocioso. Antivírus e o próprio RamDog fazem essas consultas.",
            Risk::Respawns,
        ),
        "searchhost" | "searchapp" => k(
            "A busca do menu Iniciar e a caixa de pesquisa da barra de tarefas.",
            "Fica pré-carregado para abrir instantâneo quando você aperta a tecla Windows.",
            Risk::Respawns,
        ),
        "searchindexer" => k(
            "Indexa arquivos e e-mails para a busca responder rápido.",
            "Trabalha em rajadas depois de mexer em muitos arquivos.",
            Risk::Respawns,
        ),
        "startmenuexperiencehost" => k(
            "Desenha o menu Iniciar.",
            "Pré-carregado por velocidade, mesmo com o menu fechado.",
            Risk::Respawns,
        ),
        "shellexperiencehost" => k(
            "Central de notificações, relógio e partes visuais da barra de tarefas.",
            "Sempre presente na sessão gráfica.",
            Risk::Respawns,
        ),
        "textinputhost" => k(
            "Teclado virtual, painel de emoji e sugestões de texto.",
            "Pré-carregado para o painel de emoji (Win+.) abrir sem atraso.",
            Risk::Respawns,
        ),
        "applicationframehost" => k(
            "Fornece a moldura da janela para aplicativos da Store.",
            "Um por app moderno aberto.",
            Risk::Respawns,
        ),
        "dllhost" => k(
            "Hospeda componentes COM que não têm processo próprio, como miniaturas de arquivos.",
            "Aparece e some conforme o Explorer precisa gerar previews.",
            Risk::Respawns,
        ),
        "taskhostw" => k(
            "Executa as tarefas agendadas que são bibliotecas em vez de programas.",
            "Sobe quando o Agendador de Tarefas dispara algo.",
            Risk::Respawns,
        ),
        "spoolsv" => k(
            "Fila de impressão.",
            "Sempre aberto, mesmo sem impressora instalada.",
            Risk::Respawns,
        ),
        "audiodg" => k(
            "Isola os efeitos de áudio dos drivers fora do serviço de som.",
            "Sobe quando algo toca som.",
            Risk::Respawns,
        ),
        "conhost" | "openconsole" => k(
            "Janela de console clássica para programas de linha de comando.",
            "Um por programa de terminal antigo em execução.",
            Risk::Respawns,
        ),
        "systemsettings" => k(
            "O aplicativo Configurações do Windows.",
            "Fica em segundo plano suspenso depois de fechado.",
            Risk::Respawns,
        ),
        "appactions" => k(
            "Ações de aplicativo sugeridas pelo Windows (compartilhar, abrir com).",
            "Componente do shell, sobe sob demanda.",
            Risk::Respawns,
        ),
        "widgets" | "widgetservice" => k(
            "Painel de widgets (clima, notícias) da barra de tarefas.",
            "Pode ser desligado nas configurações da barra de tarefas.",
            Risk::Respawns,
        ),
        "phoneexperiencehost" => k(
            "Aplicativo Vincular ao Celular.",
            "Sobe sozinho se você já vinculou um telefone.",
            Risk::Respawns,
        ),
        "wudfhost" => k(
            "Hospeda drivers de modo usuário (impressoras, biométricos, periféricos USB).",
            "Um por classe de dispositivo conectado.",
            Risk::Respawns,
        ),
        // ── Defender e segurança ─────────────────────────────────────────────────────
        "msmpeng" => k(
            "Motor do Microsoft Defender: varre arquivos em tempo real.",
            "Sempre aberto. A RAM sobe durante builds e downloads grandes — excluir pastas de projeto reduz muito.",
            Risk::Respawns,
        ),
        "nissrv" => k(
            "Inspeção de rede do Defender.",
            "Complemento do MsMpEng.",
            Risk::Respawns,
        ),
        "securityhealthservice" | "securityhealthsystray" => k(
            "Central de Segurança do Windows: o ícone do escudo e o painel de status.",
            "Serviço permanente do sistema.",
            Risk::Respawns,
        ),
        "mpdefendercoreservice" => k(
            "Serviço central do Defender, separado do motor de varredura.",
            "Parte da proteção em tempo real.",
            Risk::Respawns,
        ),

        // ── Atualização e nuvem ──────────────────────────────────────────────────────
        "onedrive" => k(
            "Sincroniza pastas com o OneDrive.",
            "Inicia com o Windows. Pode ser desativado se você não usa a nuvem da Microsoft.",
            Risk::Safe,
        ),
        "usocoreworker" | "mousocoreworker" => k(
            "Orquestrador do Windows Update.",
            "Sobe para checar, baixar ou preparar atualizações. Some depois.",
            Risk::Respawns,
        ),
        "tiworker" | "trustedinstaller" => k(
            "Instalador de módulos do Windows: aplica atualizações e componentes.",
            "Come CPU em rajadas depois de uma atualização. É temporário.",
            Risk::Respawns,
        ),
        "compattelrunner" => k(
            "Telemetria de compatibilidade de aplicativos.",
            "Roda por tarefa agendada, em segundo plano.",
            Risk::Safe,
        ),

        // ── Programas comuns ─────────────────────────────────────────────────────────
        "chrome" | "msedge" | "firefox" | "brave" | "opera" | "vivaldi" => k(
            "Navegador. Cada aba, extensão e site tem seu próprio processo, por isolamento.",
            "A soma dos filhos é o consumo real — um processo sozinho não diz nada.",
            Risk::Safe,
        ),
        "code" | "cursor" | "devenv" | "rider64" | "idea64" | "pycharm64" => k(
            "Editor de código. Servidores de linguagem e extensões rodam em processos separados.",
            "Os filhos costumam pesar mais que a janela em si.",
            Risk::Safe,
        ),
        "node" => k(
            "Runtime JavaScript: servidor de desenvolvimento, ferramenta de build ou agente.",
            "Olhe a linha de comando na ficha para saber qual projeto o abriu.",
            Risk::Safe,
        ),
        "python" | "python3" | "pythonw" => k(
            "Interpretador Python: script, servidor ou ferramenta.",
            "A linha de comando na ficha diz qual script está rodando.",
            Risk::Safe,
        ),
        "rustc" | "cargo" | "rust-analyzer" => k(
            "Ferramenta da toolchain Rust: compilação ou análise de código no editor.",
            "Aparece durante build ou com um projeto Rust aberto no editor.",
            Risk::Safe,
        ),
        "steam" | "steamwebhelper" => k(
            "Cliente Steam. O steamwebhelper desenha a interface, que é um navegador embutido.",
            "Inicia com o Windows por padrão; dá para desligar nas opções da Steam.",
            Risk::Safe,
        ),
        "discord" | "spotify" | "slack" | "teams" | "ms-teams" | "whatsapp" | "telegram" => k(
            "Aplicativo de desktop construído sobre um navegador embutido (Electron).",
            "Costuma iniciar junto com o Windows e ficar na bandeja.",
            Risk::Safe,
        ),
        "nvcontainer" | "nvdisplay.container" => k(
            "Contêiner de serviços da NVIDIA: telemetria, overlay e controle do driver.",
            "Instalado junto com o driver de vídeo.",
            Risk::Respawns,
        ),
        _ => return None,
    })
}
