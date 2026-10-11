// Where the readings come from. One real source, LibreHardwareMonitorLib over
// the PawnIO driver, with only the hardware groups docs/ARCHITECTURE.md §11
// names; and, in Debug builds only, a made-up one, so the pipe and the fan
// policy can be proved without elevation and without touching a sensor.

using LibreHardwareMonitor.Hardware;
using LibreHardwareMonitor.Hardware.Storage;
using Microsoft.Win32;

namespace NotchHw;

/// One sensor as the pipe tells it. `Kind` is the hardware it sits on: cpu,
/// gpu, motherboard, storage or controller. `Type` is temperature, fan,
/// control or power. `Ctl`: this fan's speed can be set from here. `Warn` and
/// `Crit`: the limits the hardware names for this temperature, where it does.
public sealed record Reading(string Id, string Hardware, string Kind, string Type, string Name, float? Value, float? Min, float? Max, bool Ctl, float? Warn = null, float? Crit = null);

/// What a drive says of its own health (SMART).
public sealed record Drive(string Name, string Letters, string Status, int? Life, int? Celsius);

public interface ISensorSource : IDisposable
{
    /// The PawnIO driver is on this PC.
    bool Driver { get; }
    /// The driver answers now. The hardware reads through it only if it did
    /// when the hardware was opened: `Reset` is how a driver that came later is taken up.
    bool Ready { get; }
    /// Reads the hardware. Slow: a second or two on some boards.
    void Update();
    /// What the last `Update` read.
    List<Reading> Read();
    List<Drive> Drives();
    /// The hardware is closed and opened again: after the machine woke up, and
    /// when the driver came while this runs.
    void Reset();
    /// The fans that can be set from here.
    int Controllable { get; }
    /// `percent` on every such fan, or `null` to hand them back to the motherboard.
    /// False when a write failed.
    bool SetFans(int? percent);
}

public static class Limits
{
    /// A drive's warning and critical temperatures are limits, not readings.
    /// The library (pinned in the project file) lists them as temperature
    /// sensors 10 and 11 of the drive: they leave the list and go with the
    /// drive's own temperature, sensor 0.
    public static List<Reading> Fold(List<Reading> all)
    {
        static bool Storage(Reading r) => r is { Kind: "storage", Type: "temperature" };
        float? Limit(string id) => all.FirstOrDefault(r => Storage(r) && r.Id == id)?.Value is { } v and > 0 ? v : null;
        return all.Where(r => !(Storage(r) && (r.Id.EndsWith("/temperature/10") || r.Id.EndsWith("/temperature/11"))))
            .Select(r => Storage(r) && r.Id.EndsWith("/temperature/0") ? r with { Warn = Limit(r.Id[..^1] + "10"), Crit = Limit(r.Id[..^1] + "11") } : r)
            .ToList();
    }
}

public sealed class LhmSource : ISensorSource
{
    private readonly Computer computer = new()
    {
        IsCpuEnabled = true,
        IsMotherboardEnabled = true,
        IsControllerEnabled = true,
        IsStorageEnabled = true,
        IsGpuEnabled = true,
    };

    public LhmSource() => computer.Open();

    public bool Driver
    {
        get
        {
            using var key = Registry.LocalMachine.OpenSubKey(@"SYSTEM\CurrentControlSet\Services\PawnIO");
            return key != null;
        }
    }

    public bool Ready => Native.DriverAnswers();

    private IEnumerable<IHardware> All()
    {
        static IEnumerable<IHardware> Tree(IHardware hw) => hw.SubHardware.SelectMany(Tree).Prepend(hw);
        return computer.Hardware.SelectMany(Tree);
    }

    private static string Kind(HardwareType type) => type switch
    {
        HardwareType.Cpu => "cpu",
        HardwareType.GpuNvidia or HardwareType.GpuAmd or HardwareType.GpuIntel => "gpu",
        HardwareType.Motherboard or HardwareType.SuperIO or HardwareType.EmbeddedController => "motherboard",
        HardwareType.Storage => "storage",
        HardwareType.Cooler => "controller",
        _ => "other",
    };

    /// The fan controls this process may write: the motherboard's and a
    /// controller's. A graphics card keeps its own.
    private IEnumerable<ISensor> Controls() =>
        All().Where(hw => Kind(hw.HardwareType) is "motherboard" or "controller")
            .SelectMany(hw => hw.Sensors)
            .Where(s => s.SensorType == SensorType.Control && s.Control != null);

    public void Update()
    {
        foreach (var hw in All()) hw.Update();
    }

    public List<Reading> Read()
    {
        var controls = Controls().Select(s => (s.Hardware.Identifier.ToString(), s.Index)).ToHashSet();
        var list = new List<Reading>();
        foreach (var hw in All())
        {
            var kind = Kind(hw.HardwareType);
            foreach (var s in hw.Sensors)
            {
                var type = s.SensorType switch
                {
                    SensorType.Temperature => "temperature",
                    SensorType.Fan => "fan",
                    SensorType.Control => "control",
                    SensorType.Power => "power",
                    _ => null,
                };
                // "Distance to TjMax" counts down as the chip heats up: not a temperature to judge by.
                if (type == null || (type == "temperature" && s.Name.Contains("Distance", StringComparison.OrdinalIgnoreCase))) continue;
                var ctl = type is "fan" or "control" && controls.Contains((hw.Identifier.ToString(), s.Index));
                list.Add(new Reading(s.Identifier.ToString(), hw.Name, kind, type, s.Name, s.Value, s.Min, s.Max, ctl));
            }
        }
        return Limits.Fold(list);
    }

    public List<Drive> Drives()
    {
        var list = new List<Drive>();
        foreach (var device in All().OfType<StorageDevice>())
        {
            var storage = device.Storage;
            if (storage == null) continue;
            var letters = string.Concat((storage.Partitions ?? []).Select(p => p.DriveLetter).OfType<char>().Where(char.IsAsciiLetter).Select(char.ToUpperInvariant).Distinct());
            var smart = storage.Smart;
            list.Add(new Drive(storage.Model ?? device.Name, letters, smart?.DiskStatus.ToString().ToLowerInvariant() ?? "unknown", smart?.Life, smart?.Temperature));
        }
        return list;
    }

    // Every group is closed and made again: each one opens the driver anew.
    public void Reset() => computer.Reset();

    public int Controllable => Controls().Count();

    public bool SetFans(int? percent)
    {
        var ok = true;
        foreach (var sensor in Controls())
        {
            try
            {
                var control = sensor.Control;
                if (percent is { } p) control.SetSoftware(Math.Clamp(p, control.MinSoftwareValue, control.MaxSoftwareValue));
                else control.SetDefault();
            }
            catch (Exception)
            {
                ok = false;
            }
        }
        return ok;
    }

    // Closing gives every fan this process touched back to the motherboard.
    public void Dispose() => computer.Close();
}

#if DEBUG
/// Made-up hardware. The processor's temperature is whatever number the file
/// holds (45 without one); the fan this source "sets" is written beside it, in
/// `<file>.fan`, as `auto` or the percentage. The driver "is installed" while
/// `<file>.driver` exists. What there is to read is decided when the hardware
/// is opened, as with the real one: with `bare` in `<file>.set` it is a laptop
/// read without the driver (a processor that lists sensors and reads none, a
/// drive with its limits, no fan). Never in a Release build.
public sealed class FakeSource : ISensorSource
{
    private readonly string file;
    private int? fan;
    private double cpu = 45;
    private bool bare;

    public FakeSource(string file)
    {
        this.file = file;
        Reset();
    }

    public bool Driver => File.Exists(file + ".driver");
    public bool Ready => Driver;
    public int Controllable => bare ? 0 : 1;
    public void Dispose() { }

    public void Reset()
    {
        try
        {
            bare = File.ReadAllText(file + ".set").Trim() == "bare";
        }
        catch (IOException)
        {
            bare = false;
        }
    }

    public void Update()
    {
        try
        {
            if (double.TryParse(File.ReadAllText(file).Trim(), System.Globalization.CultureInfo.InvariantCulture, out var v)) cpu = v;
        }
        catch (IOException) { }
    }

    public List<Reading> Read() => bare
        ? Limits.Fold(
        [
            // As the library lists a processor it cannot read: the sensors are there, the readings are not, and the power is a zero.
            new("/fakecpu/0/temperature/0", "Made-up processor", "cpu", "temperature", "CPU Package", null, null, null, false),
            new("/fakecpu/0/temperature/1", "Made-up processor", "cpu", "temperature", "P-Core #1", null, null, null, false),
            new("/fakecpu/0/power/0", "Made-up processor", "cpu", "power", "CPU Package", 0, 0, 0, false),
            new("/fakenvme/0/temperature/0", "Made-up NVMe", "storage", "temperature", "Composite Temperature", (float)cpu + 13, 30, 60, false),
            new("/fakenvme/0/temperature/10", "Made-up NVMe", "storage", "temperature", "Warning Temperature", 83, 83, 83, false),
            new("/fakenvme/0/temperature/11", "Made-up NVMe", "storage", "temperature", "Critical Temperature", 87, 87, 87, false),
        ])
        :
        [
            new("/fakecpu/0/temperature/0", "Made-up processor", "cpu", "temperature", "CPU Package", (float)cpu, 30, 95, false),
            new("/fakecpu/0/temperature/1", "Made-up processor", "cpu", "temperature", "CPU Core #1", (float)cpu - 3, 28, 92, false),
            new("/fakecpu/0/power/0", "Made-up processor", "cpu", "power", "CPU Package", 35.5f, 4, 90, false),
            new("/fakegpu/0/temperature/0", "Made-up graphics", "gpu", "temperature", "GPU Core", 50, 35, 70, false),
            new("/fakeboard/temperature/0", "Made-up board", "motherboard", "temperature", "Motherboard", 38, 30, 45, false),
            new("/fakeboard/fan/0", "Made-up board", "motherboard", "fan", "Fan #1", 300 + 15 * (fan ?? 40), 0, 1800, true),
            new("/fakeboard/fan/1", "Made-up board", "motherboard", "fan", "Fan #2", 700, 0, 1500, false),
            new("/fakeboard/control/0", "Made-up board", "motherboard", "control", "Fan #1", fan ?? 40, 0, 100, true),
        ];

    public List<Drive> Drives() => [bare ? new("Made-up NVMe", "C", "good", 97, (int)cpu + 13) : new("Made-up SSD 1TB", "C", "good", 97, 38)];

    public bool SetFans(int? percent)
    {
        fan = percent;
        try
        {
            File.WriteAllText(file + ".fan", percent?.ToString() ?? "auto");
        }
        catch (IOException) { }
        return true;
    }
}
#endif
