// What a client may ask of the fans, and what the helper does about it whatever
// the client says. Every number that reaches a fan passes through here, inside
// the elevated process: the app on the other side of the pipe is not trusted
// to have checked anything.
//
// The curve rules and the curve arithmetic are ported from the upstream file
// Services/FanControl/FanControlSupport.swift. The floor and the thermal
// override are the two additions docs/ARCHITECTURE.md §11 asks for, because a
// motherboard keeps the last speed it was given if this process is killed.

namespace NotchHw;

public readonly record struct CurvePoint(int Temperature, int Level);

public static class FanPolicy
{
    /// The lowest speed ever written, in percent. A request under it is raised to it.
    public const int Floor = 20;
    public const int Max = 100;
    public const int Step = 5;

    /// At or over this, on any processor or graphics sensor, the fans run flat out.
    public const double OverrideAt = 85;
    /// The override lets go under this: no flapping around the limit.
    public const double OverrideClear = 80;

    /// Without a heartbeat for this long the motherboard gets its fans back.
    public const long HeartbeatMs = 10_000;
    /// Readings in a row with no usable temperature before the same happens.
    public const int TemperatureFailureLimit = 3;

    public const int MinCurveTemperature = 20;
    public const int MaxCurveTemperature = 110;
    public const int MinCurvePoints = 2;
    public const int MaxCurvePoints = 8;
    /// Only when slowing down: the fans do not hunt around a point of the curve.
    public const double CurveHysteresis = 2.0;

    public static bool ValidLevel(int level) => level is >= 0 and <= Max;

    public static int Clamp(int level) => Math.Clamp(level, Floor, Max);

    /// A reading a fan decision may rest on.
    public static bool ValidTemperature(double value) => double.IsFinite(value) && value >= 1 && value < 125;

    /// Two to eight points, temperatures rising, speeds never falling.
    public static bool ValidCurve(IReadOnlyList<CurvePoint> points)
    {
        if (points.Count is < MinCurvePoints or > MaxCurvePoints) return false;
        for (var i = 0; i < points.Count; i++)
        {
            var p = points[i];
            if (p.Temperature is < MinCurveTemperature or > MaxCurveTemperature || !ValidLevel(p.Level)) return false;
            if (i > 0 && (p.Temperature <= points[i - 1].Temperature || p.Level < points[i - 1].Level)) return false;
        }
        return true;
    }

    /// The speed the curve asks for at `temperature`: a straight line between
    /// two points, rounded up to the next step.
    public static int Interpolated(IReadOnlyList<CurvePoint> points, double temperature)
    {
        var first = points[0];
        var last = points[^1];
        if (temperature <= first.Temperature) return first.Level;
        if (temperature >= last.Temperature) return last.Level;
        for (var i = 1; i < points.Count; i++)
        {
            var upper = points[i];
            if (temperature > upper.Temperature) continue;
            var lower = points[i - 1];
            var progress = (temperature - lower.Temperature) / (upper.Temperature - lower.Temperature);
            var raw = lower.Level + (upper.Level - lower.Level) * progress;
            var stepped = (int)Math.Ceiling(raw / Step - 1e-9) * Step;
            return Math.Clamp(stepped, 0, Max);
        }
        return last.Level;
    }

    /// The curve's speed with its hysteresis: going up is at once, going down
    /// waits until two degrees warmer would also ask for less.
    public static int CurveLevel(IReadOnlyList<CurvePoint> points, double temperature, int? previous)
    {
        var requested = Interpolated(points, temperature);
        if (previous is not { } before || requested >= before) return requested;
        var held = Interpolated(points, temperature + CurveHysteresis);
        return Math.Min(before, Math.Max(requested, held));
    }

    /// Whether the override holds now, given whether it held a moment ago.
    public static bool Overriding(double hottest, bool was) => hottest >= OverrideAt || (was && hottest >= OverrideClear);
}
