// hwtemp — ponte mínima pro RamDog ler temperatura de CPU e RAM.
//
// Por que existe: Tctl/Tdie (CPU) e DIMM #N (RAM) não têm API pública do Windows — só dá pra
// ler via acesso direto a hardware (MSR da CPU, Super I/O da placa-mãe pelo SMBus), que exige
// um driver de kernel assinado. Em vez de reimplementar esse driver em Rust, este helper usa a
// LibreHardwareMonitorLib (mesma lib do nosso outro app, TempHUD, já validada nesta
// máquina) e imprime uma linha JSON por leitura em stdout. O RamDog (Rust) só o inicia quando
// já está elevado — sem token de admin, Tctl vem 0 e nenhum sensor DIMM aparece (testado).
//
// Uso: hwtemp.exe <pid-do-processo-pai>
// Sai sozinho quando o processo pai morre — não fica órfão rodando em segundo plano.

using System.Text.Json;
using LibreHardwareMonitor.Hardware;

int parentPid = args.Length > 0 && int.TryParse(args[0], out var p) ? p : 0;
System.Diagnostics.Process? parent = null;
if (parentPid > 0)
{
    try { parent = System.Diagnostics.Process.GetProcessById(parentPid); } catch { }
}

var computer = new Computer { IsCpuEnabled = true, IsMotherboardEnabled = true };
computer.Open();
var visitor = new UpdateVisitor();

using var stdout = Console.OpenStandardOutput();
using var writer = new StreamWriter(stdout) { AutoFlush = true };

while (parent is null || !parent.HasExited)
{
    computer.Accept(visitor);

    float? cpuTemp = null;
    var dimm = new List<double>();

    foreach (var hw in computer.Hardware)
    {
        if (hw.HardwareType == HardwareType.Cpu)
        {
            var t = hw.Sensors.FirstOrDefault(s =>
                s.SensorType == SensorType.Temperature &&
                (s.Name.Contains("Tctl") || s.Name.Contains("Tdie") || s.Name.Contains("Package")));
            if (t?.Value is float v && v > 0)
            {
                cpuTemp = v;
            }
        }
        if (hw.HardwareType == HardwareType.Motherboard)
        {
            foreach (var sub in hw.SubHardware)
            {
                foreach (var s in sub.Sensors)
                {
                    if (s.SensorType == SensorType.Temperature && s.Name.StartsWith("DIMM") && s.Value is float dv && dv > 0)
                    {
                        dimm.Add(Math.Round(dv, 1));
                    }
                }
            }
        }
    }

    writer.WriteLine(JsonSerializer.Serialize(new { cpu_temp = cpuTemp, dimm }));
    Thread.Sleep(1500);
}

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
