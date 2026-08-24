// hwtemp — motor térmico do RamDog: sensores + controle de fans via LibreHardwareMonitorLib.
//
// Por que existe: Tctl/Tdie (CPU), DIMM #N (RAM) e controle de fan SuperIO não têm API pública
// do Windows — só dá pra ler/escrever via acesso direto a hardware (MSR da CPU, Super I/O da
// placa-mãe pelo SMBus), que exige um driver de kernel assinado. Em vez de reimplementar esse
// driver em Rust, este helper usa a LibreHardwareMonitorLib (mesma lib do TempHUD, já validada
// nesta máquina) e conversa com o RamDog por pipes. Herda o token do RamDog (asInvoker).
// Sem admin, Tctl/DIMM vêm vazios e nenhum controle de fan aparece — o RamDog mostra "–".
//
// Protocolo:
//   stdout — uma linha JSON por tick (1s): { cpu_temp, dimm, sensors, fans, stab }
//   stdin  — comandos em texto, um por linha:
//     "set <pct> <nome do fan>"  — % manual (nome pode ter espaço, por isso vem por último)
//     "auto <nome do fan>"       — devolve esse fan à BIOS
//     "stab on" / "stab off"     — liga/desliga a curva ESTABILIZAR
//     "quit"                     — encerra devolvendo tudo à BIOS
//
// Segurança: a curva mora AQUI, não no RamDog. Se o RamDog travar ou morrer (pai encerrado ou
// EOF no stdin), o helper devolve todo fan tocado à BIOS (SetDefault) antes de sair — fans
// nunca ficam órfãos presos num % fixo.
//
// Curva ESTABILIZAR (portada do TempHUD, medida no 9800X3D + MSI B650M): 50% até 80°C,
// rampa linear até 100% aos 92°C, ≥95°C = 100% na hora; EMA α=0.35, sobe/desce no máx 3%/s.
//
// Uso: hwtemp.exe <pid-do-processo-pai>

using System.Collections.Concurrent;
using System.Globalization;
using System.Text.Json;
using LibreHardwareMonitor.Hardware;

const float HoldPct = 50;
const float RampFromC = 80;
const float RampToC = 92;
const float KillC = 95;
const float SlewPct = 3;
const float EmaAlpha = 0.35f;

int parentPid = args.Length > 0 && int.TryParse(args[0], out var p) ? p : 0;
System.Diagnostics.Process? parent = null;
if (parentPid > 0)
{
    try { parent = System.Diagnostics.Process.GetProcessById(parentPid); } catch { }
}

var computer = new Computer
{
    IsCpuEnabled = true,
    IsGpuEnabled = true,
    IsMemoryEnabled = true,
    IsMotherboardEnabled = true,
};
computer.Open();
var visitor = new UpdateVisitor();

using var stdout = Console.OpenStandardOutput();
using var writer = new StreamWriter(stdout) { AutoFlush = true };

// stdin em thread própria: EOF = RamDog morreu ou fechou o pipe → mesmo caminho do "quit".
var cmds = new ConcurrentQueue<string>();
var stdinThread = new Thread(() =>
{
    string? line;
    while ((line = Console.ReadLine()) is not null)
        cmds.Enqueue(line.Trim());
    cmds.Enqueue("quit");
}) { IsBackground = true };
stdinThread.Start();

IEnumerable<IHardware> AllHardware() =>
    computer.Hardware.SelectMany(h => new[] { h }.Concat(h.SubHardware));

computer.Accept(visitor);
var controls = AllHardware()
    .SelectMany(h => h.Sensors)
    .Where(s => s.SensorType == SensorType.Control && s.Control is not null
                && s.Hardware.HardwareType == HardwareType.SuperIO)
    .ToList();

var touched = new HashSet<ISensor>();
var manualPct = new Dictionary<ISensor, float>();
var guarded = new HashSet<ISensor>();
bool stab = false;
bool cpuHot = false;
float tempEma = float.NaN;
float heldPct = HoldPct;
bool quit = false;

ISensor? FindControl(string name) => controls.FirstOrDefault(c => c.Name == name);

float DesiredPct(float cpuC)
{
    if (cpuC >= KillC) return 100;
    if (cpuC <= RampFromC) return HoldPct;
    float u = (cpuC - RampFromC) / (RampToC - RampFromC);
    return HoldPct + (100 - HoldPct) * Math.Clamp(u, 0, 1);
}

float TickStabilize(float rawC)
{
    tempEma = float.IsNaN(tempEma) ? rawC : tempEma * (1 - EmaAlpha) + rawC * EmaAlpha;
    float desired = DesiredPct(tempEma);
    if (desired >= 100 && tempEma >= KillC)
    {
        heldPct = 100;
        return heldPct;
    }
    heldPct = Math.Clamp(heldPct + Math.Clamp(desired - heldPct, -SlewPct, SlewPct), HoldPct, 100);
    return heldPct;
}

void ApplyCommand(string cmd)
{
    if (cmd == "quit") { quit = true; return; }
    if (cmd == "stab on")
    {
        stab = true;
        tempEma = float.NaN;
        heldPct = HoldPct;
        return;
    }
    if (cmd == "stab off")
    {
        stab = false;
        tempEma = float.NaN;
        heldPct = HoldPct;
        manualPct.Clear();
        guarded.Clear();
        foreach (var c in controls) c.Control?.SetDefault(); // tudo de volta à BIOS
        return;
    }
    if (cmd.StartsWith("auto "))
    {
        var c = FindControl(cmd[5..]);
        if (c is null) return;
        manualPct.Remove(c);
        guarded.Remove(c);
        c.Control?.SetDefault();
        return;
    }
    if (cmd.StartsWith("set "))
    {
        int sp = cmd.IndexOf(' ', 4);
        if (sp < 0) return;
        if (!float.TryParse(cmd[4..sp], NumberStyles.Float, CultureInfo.InvariantCulture, out float pct)) return;
        var c = FindControl(cmd[(sp + 1)..]);
        if (c is null) return;
        pct = Math.Clamp(pct, 0, 100);
        manualPct[c] = pct;
        guarded.Remove(c);
        c.Control?.SetSoftware(pct);
        touched.Add(c);
    }
}

static string HwLabel(IHardware hw) => hw.HardwareType switch
{
    HardwareType.Cpu => "CPU",
    HardwareType.GpuNvidia => "GPU",
    HardwareType.GpuAmd => "iGPU",
    HardwareType.GpuIntel => "GPU",
    HardwareType.SuperIO => "Placa-mãe",
    HardwareType.Memory => "RAM",
    _ => hw.Name,
};

static bool Include(ISensor s) => s.SensorType switch
{
    SensorType.Temperature => !s.Name.Contains("Limit") && !s.Name.Contains("Resolution"),
    SensorType.Fan => s.Hardware.HardwareType != HardwareType.SuperIO, // RPM SuperIO vai junto do fan
    SensorType.Load => s.Name is "CPU Total" or "GPU Core" or "Memory" or "GPU Memory",
    _ => false,
};

while (!quit && (parent is null || !parent.HasExited))
{
    computer.Accept(visitor);

    float? cpuTemp = null;
    var dimm = new List<double>();
    var sensors = new List<object>();
    var fans = new List<object>();

    foreach (var hw in AllHardware())
    {
        if (hw.HardwareType == HardwareType.Cpu && cpuTemp is null)
        {
            var t = hw.Sensors
                .Where(s => s.SensorType == SensorType.Temperature && s.Value is float v && v > 0)
                .OrderBy(s =>
                    s.Name.Contains("Tctl") ? 0 :
                    s.Name.Contains("Tdie") ? 1 :
                    s.Name.Contains("Package") ? 2 : 3)
                .FirstOrDefault();
            if (t?.Value is float v) cpuTemp = v;
        }
        foreach (var s in hw.Sensors)
        {
            if (s.SensorType == SensorType.Temperature && s.Name.StartsWith("DIMM") && s.Value is float dv && dv > 0)
                dimm.Add(Math.Round(dv, 1));
            if (Include(s) && s.Value is float sv)
            {
                // Sem admin, Tctl existe mas lê 0 — 0°C é "sem leitura", não um dado.
                if (s.SensorType == SensorType.Temperature && sv <= 0) continue;
                string kind = s.SensorType switch
                {
                    SensorType.Temperature => "temp",
                    SensorType.Fan => "rpm",
                    _ => "load",
                };
                // Memória vem como dois hardwares ("Total Memory" e "Virtual Memory"), cada um
                // com um sensor chamado só "Memory" — o nome do hardware é que distingue.
                string name = hw.HardwareType == HardwareType.Memory ? hw.Name : s.Name;
                sensors.Add(new { hw = HwLabel(hw), name, kind, value = Math.Round(sv, 1) });
            }
        }
    }

    float tRaw = cpuTemp ?? 0;
    if (stab)
    {
        float pct = TickStabilize(tRaw);
        foreach (var c in controls)
        {
            c.Control?.SetSoftware(pct);
            touched.Add(c);
        }
    }
    else
    {
        // histérese da proteção térmica do modo manual: liga em 80°C, desliga abaixo de 72°C
        cpuHot = cpuHot ? tRaw > 72 : tRaw >= 80;
        foreach (var (c, pct) in manualPct)
        {
            if (cpuHot && !guarded.Contains(c))
            {
                guarded.Add(c);
                c.Control?.SetSoftware(100);
            }
            else if (!cpuHot && guarded.Contains(c))
            {
                guarded.Remove(c);
                c.Control?.SetSoftware(pct);
            }
        }
    }

    foreach (var c in controls)
    {
        var rpm = c.Hardware.Sensors.FirstOrDefault(f => f.SensorType == SensorType.Fan && f.Name == c.Name);
        fans.Add(new
        {
            name = c.Name,
            pct = c.Value is float cv ? Math.Round(cv, 0) : (double?)null,
            rpm = rpm?.Value is float rv ? Math.Round(rv, 0) : (double?)null,
            auto = !stab && !manualPct.ContainsKey(c),
            guard = guarded.Contains(c),
        });
    }

    writer.WriteLine(JsonSerializer.Serialize(new
    {
        cpu_temp = cpuTemp,
        dimm,
        sensors,
        fans,
        stab = new { on = stab, held = stab ? Math.Round(heldPct, 0) : HoldPct },
    }));

    // 10×100ms em vez de um sleep de 1s: comando do usuário (slider, ESTABILIZAR) aplica
    // em ≤100ms, a emissão continua 1x/s.
    for (int i = 0; i < 10 && !quit; i++)
    {
        while (cmds.TryDequeue(out var cmd)) ApplyCommand(cmd);
        Thread.Sleep(100);
    }
}

foreach (var c in touched) c.Control?.SetDefault(); // devolve fans à BIOS, sempre
computer.Close();

class UpdateVisitor : IVisitor
{
    public void VisitComputer(IComputer computer) => computer.Traverse(this);
    public void VisitHardware(IHardware hardware)
    {
        hardware.Update();
        foreach (var sub in hardware.SubHardware) sub.Accept(this);
    }
    public void VisitSensor(ISensor sensor) { }
    public void VisitParameter(IParameter parameter) { }
}
