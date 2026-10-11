// The helper's state and its protocol (docs/ARCHITECTURE.md §11): one line of
// JSON in, one line out, `{ "v": 1, "op": … }`. Everything that arrives is
// untrusted: it is parsed strictly, an unknown op or field is refused, and no
// request can make this process do anything but read sensors and set fans
// inside FanPolicy. Nothing here runs a program, opens a file or takes a path.
//
// Ops: hello, snapshot, subscribe { ms }, reopen, fan.apply { mode, level, curve },
// fan.heartbeat, fan.restore, quit. An optional `id` comes back in the reply.
// While subscribed the helper also says `{ "ev": "snapshot", … }` by itself.
//
// `reopen` closes the hardware and opens it again, in this same process: how a
// driver installed while the helper runs is taken up without a second Windows
// prompt. The helper also does it by itself, at the next reading after the
// driver starts to answer.

using System.Security.Principal;
using System.Text.Json;

namespace NotchHw;

public sealed class Engine(ISensorSource source, bool fake)
{
    public const string Version = "0.3.0";
    /// The longest request there is fits several times over.
    public const int MaxLine = 4096;
    /// With no client and the fans on automatic for this long, the helper exits.
    public const long IdleMs = 30_000;
    /// Sensors are read at most this often, whoever asks.
    private const long MinReadMs = 1000;
    /// Opening the hardware again takes seconds and hands the fans back
    /// meanwhile: on request, not more often than this.
    private const long ReopenMinMs = 5000;
    private const int MaxSensors = 512;

    private static readonly bool Elevated = new WindowsPrincipal(WindowsIdentity.GetCurrent()).IsInRole(WindowsBuiltInRole.Administrator);

    private readonly object gate = new();

    private List<Reading> readings = [];
    private List<Drive> drives = [];
    private long readAt;
    /// The hardware was opened with the driver answering: it reads through it.
    private bool inUse = source.Ready;
    /// Readings since the hardware was last opened: a sensor still silent after a few is not coming.
    private int reads;
    private long reopenedAt;

    private string mode = "auto";
    private int level;
    private List<CurvePoint> curve = [];
    /// What the fans were last told, in percent; null while the motherboard has them.
    private int? applied;
    /// Why `applied` is not what was asked, or why automatic control came back.
    private string? reason;
    private long heartbeatAt;
    private int failures;
    private bool overriding;
    private int? curveLevel;

    private int subscribeMs;
    private long pushedAt;
    private Action<byte[]>? push;
    private bool connected;
    private long idleSince = Environment.TickCount64;
    private bool down;

    /// Set from the system's thread when the machine wakes; acted on by the next tick.
    public volatile bool Resumed;
    /// A client asked the helper to end.
    public volatile bool Quit;

    // ── The connection ──

    /// A client passed the peer check (`send` reaches it), or the one there was is gone.
    public void Connected(Action<byte[]>? send)
    {
        lock (gate)
        {
            connected = send != null;
            push = send;
            subscribeMs = 0;
            idleSince = Environment.TickCount64;
        }
    }

    /// Nobody is connected, no fan is held, and it has been so for `IdleMs`.
    public bool Idle
    {
        get
        {
            lock (gate) return !connected && mode == "auto" && Environment.TickCount64 - idleSince >= IdleMs;
        }
    }

    // ── Once a second ──

    public void Tick()
    {
        byte[]? said = null;
        Action<byte[]>? to = null;
        lock (gate)
        {
            if (down) return;
            var now = Environment.TickCount64;
            if (Resumed)
            {
                // The hardware lost what it was told while the machine slept, and the
                // time asleep is not a missed heartbeat.
                Resumed = false;
                Reopen("resume");
                heartbeatAt = now;
            }
            if (mode != "auto")
            {
                Refresh(now, true);
                if (now - heartbeatAt > FanPolicy.HeartbeatMs) Restore("heartbeat");
                else Steer();
            }
            // The tick is a second long: a period is a whole number of them.
            if (subscribeMs > 0 && push != null && now - pushedAt >= subscribeMs - 200)
            {
                Refresh(now, false);
                pushedAt = now;
                said = Snapshot(null, null);
                to = push;
            }
        }
        to?.Invoke(said!);
    }

    /// The fans go back to the motherboard, whatever happens next. Safe to call twice.
    public void Shutdown()
    {
        lock (gate)
        {
            if (down) return;
            down = true;
            if (mode != "auto") Restore("exit");
            try { source.Dispose(); } catch (Exception e) { Program.Log($"close: {e.Message}"); }
        }
    }

    // ── Sensors ──

    private void Refresh(long now, bool force)
    {
        if (!force && readAt != 0 && now - readAt < MinReadMs) return;
        try
        {
            // The driver came while this runs (installed a moment ago): the hardware
            // is opened again, with it. Gone, it is said to be gone.
            var ready = source.Ready;
            if (ready && !inUse) Reopen("driver");
            inUse = ready;
            source.Update();
            readings = source.Read();
            drives = source.Drives();
            if (reads < int.MaxValue) reads++;
        }
        catch (Exception e)
        {
            // Nothing read is nothing known: the fan rules below treat it so.
            Program.Log($"read: {e.Message}");
            readings = [];
            drives = [];
        }
        readAt = now;
    }

    /// The hardware is closed and opened again. What was read before is of the
    /// hardware as it was: nothing is known until the next reading.
    private void Reopen(string why)
    {
        try { source.Reset(); } catch (Exception e) { Program.Log($"reopen: {e.Message}"); }
        inUse = source.Ready;
        reads = 0;
        readAt = 0;
        readings = [];
        drives = [];
        // Closing gave the fans back to the motherboard: what is asked of them is told again.
        if (mode != "auto") applied = null;
        Program.Log($"reopened: {why}, driver in use: {inUse}");
    }

    private IEnumerable<(string Name, double Value)> Temperatures(params string[] kinds) =>
        readings.Where(r => r.Type == "temperature" && kinds.Contains(r.Kind) && r.Value is { } v && FanPolicy.ValidTemperature(v))
            .Select(r => (r.Name, (double)r.Value!.Value));

    /// The processor's temperature as one number. What counts is where a sensor
    /// sits (the processor) and what it measures (a temperature), not what it is
    /// called: of those that read, the package where the chip names one, the die
    /// on AMD, else the hottest of them all, which is the hottest core.
    private double? CpuTemperature()
    {
        var cpu = Temperatures("cpu").ToList();
        if (cpu.Count == 0) return null;
        foreach (var name in (string[])["Package", "Tctl", "Tdie"])
        {
            var named = cpu.Where(t => t.Name.Contains(name, StringComparison.OrdinalIgnoreCase)).ToList();
            if (named.Count > 0) return named.Max(t => t.Value);
        }
        return cpu.Max(t => t.Value);
    }

    /// The processor's power as one number: the package where the chip names
    /// one, else the largest it reports (the whole is never less than a part).
    /// Zero is no reading: it is what the library says of a counter it could
    /// not read, which is every counter while the driver is not in use.
    private double? CpuPower()
    {
        var power = readings.Where(r => r is { Type: "power", Kind: "cpu", Value: not null } && float.IsFinite(r.Value.Value) && r.Value > 0).ToList();
        var package = power.FirstOrDefault(r => r.Name.Contains("Package", StringComparison.OrdinalIgnoreCase)) ?? power.MaxBy(r => r.Value);
        return package?.Value;
    }

    // ── Fans ──

    /// Works out what the fans should be told now, and tells them if it changed.
    private void Steer()
    {
        var hot = Temperatures("cpu", "gpu").Select(t => t.Value).DefaultIfEmpty(double.NaN).Max();
        if (double.IsNaN(hot))
        {
            // Blind is no state to hold a fan in.
            if (++failures >= FanPolicy.TemperatureFailureLimit) Restore("temperature");
            return;
        }
        failures = 0;
        int asked;
        if (mode == "manual") asked = level;
        else asked = (curveLevel = FanPolicy.CurveLevel(curve, CpuTemperature() ?? hot, curveLevel)).Value;
        var target = FanPolicy.Clamp(asked);
        reason = asked < FanPolicy.Floor ? "floor" : null;
        overriding = FanPolicy.Overriding(hot, overriding);
        if (overriding)
        {
            target = FanPolicy.Max;
            reason = "thermal";
        }
        if (applied == target) return;
        // debt: the write is not read back from the chip (upstream gives up after three mismatches).
        if (source.SetFans(target)) applied = target;
        else Restore("control");
    }

    private void Restore(string? why)
    {
        try { source.SetFans(null); } catch (Exception e) { Program.Log($"restore: {e.Message}"); }
        mode = "auto";
        applied = null;
        reason = why;
        failures = 0;
        overriding = false;
        curveLevel = null;
    }

    // ── Requests ──

    /// One request line in, one reply line out. Never throws.
    public byte[] Handle(ReadOnlySpan<byte> line)
    {
        string? op = null;
        long? id = null;
        try
        {
            using var doc = JsonDocument.Parse(line.ToArray(), new JsonDocumentOptions { MaxDepth = 4 });
            var root = doc.RootElement;
            if (root.ValueKind != JsonValueKind.Object) return Error(null, null, "malformed");
            // First who is asking for what, so that a refusal can say so.
            if (root.TryGetProperty("id", out var idGiven) && Whole(idGiven) is { } n and >= 0) id = n;
            if (root.TryGetProperty("op", out var opGiven) && opGiven.ValueKind == JsonValueKind.String) op = opGiven.GetString();

            var version = false;
            JsonElement? ms = null, modeGiven = null, levelGiven = null, curveGiven = null;
            var seen = new HashSet<string>();
            foreach (var property in root.EnumerateObject())
            {
                if (!seen.Add(property.Name)) return Error(op, id, "malformed");
                switch (property.Name)
                {
                    case "v": version = Whole(property.Value) == 1; break;
                    case "op": if (op == null) return Error(null, id, "malformed"); break;
                    case "id": if (id == null) return Error(op, null, "bad field"); break;
                    case "ms": ms = property.Value; break;
                    case "mode": modeGiven = property.Value; break;
                    case "level": levelGiven = property.Value; break;
                    case "curve": curveGiven = property.Value; break;
                    default: return Error(op, id, "unknown field");
                }
            }
            if (!version) return Error(op, id, "version");
            if (op == null) return Error(null, id, "malformed");
            if (ms != null && op != "subscribe") return Error(op, id, "unknown field");
            if ((modeGiven ?? levelGiven ?? curveGiven) != null && op != "fan.apply") return Error(op, id, "unknown field");

            lock (gate)
            {
                if (down) return Error(op, id, "closing");
                var now = Environment.TickCount64;
                switch (op)
                {
                    case "hello":
                        return Json(w =>
                        {
                            Head(w, op, id);
                            w.WriteString("version", Version);
                            w.WriteNumber("pid", Environment.ProcessId);
                            w.WriteBoolean("elevated", Elevated);
                            w.WriteBoolean("driver", source.Driver);
                            w.WriteBoolean("inUse", inUse);
                            w.WriteBoolean("fake", fake);
                        });
                    case "snapshot":
                        Refresh(now, false);
                        return Snapshot(op, id);
                    case "subscribe":
                        if (ms is not { } period || Whole(period) is not { } wanted || wanted < 0) return Error(op, id, "bad field");
                        // Zero ends it. Anything else is at least a second: reading the hardware is slow.
                        subscribeMs = wanted == 0 ? 0 : (int)Math.Clamp(wanted, 1000, 60_000);
                        pushedAt = now;
                        return Json(w =>
                        {
                            Head(w, op, id);
                            w.WriteNumber("ms", subscribeMs);
                        });
                    case "reopen":
                        if (reopenedAt != 0 && now - reopenedAt < ReopenMinMs) return Error(op, id, "too soon");
                        reopenedAt = now;
                        Reopen("request");
                        return Json(w =>
                        {
                            Head(w, op, id);
                            w.WriteBoolean("inUse", inUse);
                        });
                    case "fan.apply":
                        return Apply(op, id, now, modeGiven, levelGiven, curveGiven);
                    case "fan.heartbeat":
                        heartbeatAt = now;
                        return FanReply(op, id);
                    case "fan.restore":
                        if (mode != "auto") Restore(null);
                        return FanReply(op, id);
                    case "quit":
                        if (mode != "auto") Restore("exit");
                        Quit = true;
                        return Json(w => Head(w, op, id));
                    default:
                        return Error(op, id, "unknown op");
                }
            }
        }
        catch (JsonException)
        {
            return Error(op, id, "malformed");
        }
        catch (Exception e)
        {
            // A request must never take the helper down with a fan in its hand.
            Program.Log($"request: {e}");
            return Error(op, id, "failed");
        }
    }

    private byte[] Apply(string op, long? id, long now, JsonElement? modeGiven, JsonElement? levelGiven, JsonElement? curveGiven)
    {
        if (modeGiven is not { ValueKind: JsonValueKind.String } m) return Error(op, id, "bad field");
        var wanted = m.GetString();
        var points = new List<CurvePoint>();
        long asked = 0;
        switch (wanted)
        {
            case "auto":
                break;
            case "manual":
                if (levelGiven is not { } l || Whole(l) is not { } percent || !FanPolicy.ValidLevel((int)Math.Clamp(percent, -1, 101))) return Error(op, id, "bad field");
                asked = percent;
                break;
            case "curve":
                if (curveGiven is not { ValueKind: JsonValueKind.Array } list || list.GetArrayLength() > FanPolicy.MaxCurvePoints) return Error(op, id, "bad field");
                foreach (var point in list.EnumerateArray())
                {
                    if (point.ValueKind != JsonValueKind.Object || point.EnumerateObject().Count() != 2) return Error(op, id, "bad field");
                    if (!point.TryGetProperty("t", out var t) || !point.TryGetProperty("level", out var p)) return Error(op, id, "bad field");
                    if (Whole(t) is not { } temperature || Whole(p) is not { } speed || temperature is < 0 or > 1000 || speed is < 0 or > 1000) return Error(op, id, "bad field");
                    points.Add(new CurvePoint((int)temperature, (int)speed));
                }
                if (!FanPolicy.ValidCurve(points)) return Error(op, id, "bad field");
                break;
            default:
                return Error(op, id, "bad field");
        }
        if (wanted == "auto")
        {
            if (mode != "auto") Restore(null);
            reason = null;
            return FanReply(op, id);
        }
        if (source.Controllable == 0) return Error(op, id, "no controllable fan");
        mode = wanted!;
        level = (int)asked;
        curve = points;
        curveLevel = null;
        failures = 0;
        reason = null;
        heartbeatAt = now;
        Refresh(now, true);
        Steer();
        return FanReply(op, id);
    }

    /// A JSON number that is a whole number, or null.
    private static long? Whole(JsonElement e) => e.ValueKind == JsonValueKind.Number && e.TryGetInt64(out var n) ? n : null;

    // ── Replies ──

    private static byte[] Json(Action<Utf8JsonWriter> body)
    {
        using var stream = new MemoryStream();
        using (var w = new Utf8JsonWriter(stream))
        {
            w.WriteStartObject();
            w.WriteNumber("v", 1);
            body(w);
            w.WriteEndObject();
        }
        stream.WriteByte((byte)'\n');
        return stream.ToArray();
    }

    private static void Head(Utf8JsonWriter w, string? op, long? id)
    {
        w.WriteBoolean("ok", true);
        w.WriteString("op", op);
        if (id is { } n) w.WriteNumber("id", n);
    }

    public static byte[] Error(string? op, long? id, string error) => Json(w =>
    {
        w.WriteBoolean("ok", false);
        // What was asked is said back only when it is one of ours: nothing a client sent is echoed.
        if (op is "hello" or "snapshot" or "subscribe" or "reopen" or "fan.apply" or "fan.heartbeat" or "fan.restore" or "quit") w.WriteString("op", op);
        if (id is { } n) w.WriteNumber("id", n);
        w.WriteString("error", error);
    });

    private byte[] FanReply(string op, long? id) => Json(w =>
    {
        Head(w, op, id);
        Fan(w);
    });

    private void Fan(Utf8JsonWriter w)
    {
        w.WriteStartObject("fan");
        w.WriteString("mode", mode);
        w.WriteNumber("level", level);
        Number(w, "applied", applied);
        w.WriteString("reason", reason);
        w.WriteNumber("controllable", source.Controllable);
        w.WriteEndObject();
    }

    private static void Number(Utf8JsonWriter w, string name, double? value)
    {
        if (value is { } v && double.IsFinite(v)) w.WriteNumber(name, Math.Round(v, 2));
        else w.WriteNull(name);
    }

    private static string Short(string? text) => text == null ? "" : text.Length <= 96 ? text : text[..96];

    /// The last reading: the reply to `snapshot` (with `op`), or what a subscriber is told.
    private byte[] Snapshot(string? op, long? id) => Json(w =>
    {
        if (op == null) w.WriteString("ev", "snapshot");
        else Head(w, op, id);
        w.WriteNumber("uptimeMs", readAt);
        w.WriteBoolean("inUse", inUse);
        w.WriteNumber("reads", reads);
        Number(w, "cpuTemp", CpuTemperature());
        Number(w, "cpuPower", CpuPower());
        w.WriteStartArray("sensors");
        foreach (var r in readings.Take(MaxSensors))
        {
            w.WriteStartObject();
            w.WriteString("id", Short(r.Id));
            w.WriteString("hardware", Short(r.Hardware));
            w.WriteString("kind", r.Kind);
            w.WriteString("type", r.Type);
            w.WriteString("name", Short(r.Name));
            Number(w, "value", r.Value);
            Number(w, "min", r.Min);
            Number(w, "max", r.Max);
            w.WriteNumber("readAtUptimeMs", readAt);
            w.WriteBoolean("ctl", r.Ctl);
            if (r.Warn != null) Number(w, "warn", r.Warn);
            if (r.Crit != null) Number(w, "crit", r.Crit);
            w.WriteEndObject();
        }
        w.WriteEndArray();
        w.WriteStartArray("drives");
        foreach (var d in drives.Take(64))
        {
            w.WriteStartObject();
            w.WriteString("name", Short(d.Name));
            w.WriteString("letters", Short(d.Letters));
            w.WriteString("status", d.Status);
            Number(w, "life", d.Life);
            Number(w, "celsius", d.Celsius);
            w.WriteEndObject();
        }
        w.WriteEndArray();
        Fan(w);
    });
}
