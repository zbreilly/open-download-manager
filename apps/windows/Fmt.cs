using System.Globalization;
using System.Text.Json.Nodes;

namespace OpenDownloadManager;

/// <summary>Text formatting, by the same rules as the other front ends (base-1024 sizes, integer percent, short ETA).</summary>
public static class Fmt
{
    private const double KB = 1024, MB = KB * 1024, GB = MB * 1024;
    public const string Dash = "—";
    private static readonly CultureInfo Inv = CultureInfo.InvariantCulture;

    public static string Bytes(ulong? n)
    {
        if (n is not ulong v) return Dash;
        double d = v;
        if (d < MB) return (d / KB).ToString("0.0", Inv) + " KB";
        if (d < GB) return (d / MB).ToString("0.0", Inv) + " MB";
        return (d / GB).ToString("0.00", Inv) + " GB";
    }

    public static string Speed(ulong bps) => bps == 0 ? Dash : Bytes(bps) + "/s";

    /// <summary>0..99 while unfinished, 100 only when complete.</summary>
    public static int Percent(ulong received, ulong? total, bool complete)
    {
        if (complete) return 100;
        if (total is not ulong t || t == 0) return 0;
        return (int)Math.Clamp(received / (double)t * 100.0, 0, 99);
    }

    public static string Eta(ulong? secs)
    {
        if (secs is not ulong s || s >= ulong.MaxValue / 2) return Dash;
        if (s < 60) return $"{s}s";
        if (s < 3600) return $"{s / 60}m {s % 60:00}s";
        if (s < 86400) return $"{s / 3600}h {s % 3600 / 60:00}m";
        return "> 1 day";
    }

    private static DateTime? Local(string? iso) =>
        DateTimeOffset.TryParse(iso, Inv, DateTimeStyles.RoundtripKind, out var d) ? d.LocalDateTime : null;

    /// <summary>"Today 14:02", "Yesterday 22:41", "Oct 6", "Oct 6, 2025".</summary>
    public static string Added(string? iso)
    {
        if (Local(iso) is not DateTime d) return Dash;
        var now = DateTime.Now;
        var days = (now.Date - d.Date).Days;
        var hm = d.ToString("HH:mm", Inv);
        if (days == 0) return $"Today {hm}";
        if (days == 1) return $"Yesterday {hm}";
        return d.ToString(d.Year == now.Year ? "MMM d" : "MMM d, yyyy", Inv);
    }

    public static string HHmm(string? iso) => Local(iso)?.ToString("HH:mm", Inv) ?? "";

    public static bool IsActive(string status) => status is "connecting" or "downloading" or "assembling";

    public static string StatusLabel(string status, string? scheduledAt) => status switch
    {
        "downloading" => "Downloading",
        "paused" => "Paused",
        "queued" => scheduledAt != null ? $"Starts {HHmm(scheduledAt)}" : "Queued",
        "error" => "Failed",
        "complete" => "Complete",
        "connecting" => "Connecting",
        "assembling" => "Finishing",
        _ => status,
    };

    /// <summary>Bytes per second as MB/s with at most one decimal ("25", "2.5").</summary>
    public static string Mbps(ulong bps) => (Math.Round(bps / MB * 10) / 10).ToString("0.#", Inv);

    public const double MaxLimitMB = 1_000_000;
    public const string LimitError = "Enter a speed between 0 and 1,000,000 MB/s.";

    /// <summary>
    /// Parse a "MB/s" text field. Null for anything that is not a finite number above 0 and at
    /// most <see cref="MaxLimitMB"/>, so callers show an error instead of silently turning a limit off.
    /// </summary>
    public static ulong? LimitBps(string text)
    {
        if (!double.TryParse(text.Trim().Replace(',', '.'), NumberStyles.Float, Inv, out var v)) return null;
        if (!double.IsFinite(v) || v <= 0 || v > MaxLimitMB) return null;
        return Math.Max(1, (ulong)Math.Round(v * MB));
    }

    public static string Host(string url) => Uri.TryCreate(url, UriKind.Absolute, out var u) ? u.Host : "";

    /// <summary>A full http(s) address with a host.</summary>
    public static bool IsHttp(string? s) =>
        Uri.TryCreate(s?.Trim(), UriKind.Absolute, out var u) && u.Scheme is "http" or "https" && u.Host.Length > 0;

    /// <summary>Lowercase extension of a file name ("" when there is none or it is longer than 10 characters).</summary>
    public static string Extension(string name)
    {
        var dot = name.LastIndexOf('.');
        if (dot <= 0 || dot == name.Length - 1) return "";
        var e = name[(dot + 1)..];
        return e.Length <= 10 && !e.Contains('/') && !e.Contains('\\') ? e.ToLowerInvariant() : "";
    }

    /// <summary>The last path segment of a link, percent-decoded.</summary>
    public static string UrlFileName(string url)
    {
        if (!Uri.TryCreate(url, UriKind.Absolute, out var u)) return "";
        var last = u.AbsolutePath.Split('/').LastOrDefault() ?? "";
        return Uri.UnescapeDataString(last);
    }

    /// <summary>"2 active · 1 paused · 3 queued" for the status bar.</summary>
    public static string TotalsSummary(JsonNode? t)
    {
        var parts = new List<string> { $"{J.N(t, "active")} active" };
        foreach (var k in new[] { "paused", "queued", "scheduled", "failed" })
            if (J.N(t, k) > 0) parts.Add($"{J.N(t, k)} {k}");
        return string.Join(" · ", parts);
    }

    /// <summary>When a queue's schedule next starts it ("today 01:00", "tomorrow 01:00", "Friday 01:00"), or null.</summary>
    public static string? NextRun(JsonNode? q)
    {
        if (!J.B(q, "startEnabled")) return null;
        var parts = J.S(q, "startTime").Split(':');
        if (parts.Length != 2 || !int.TryParse(parts[0], out var h) || !int.TryParse(parts[1], out var m)) return null;
        if (h is < 0 or > 23 || m is < 0 or > 59) return null;
        var days = J.Arr(q, "days").Select(d => d is JsonValue v && v.TryGetValue<bool>(out var on) ? on : true).ToList();
        var now = DateTime.Now;
        for (var add = 0; add < 8; add++)
        {
            var t = now.Date.AddDays(add).AddHours(h).AddMinutes(m);
            var wd = ((int)t.DayOfWeek + 6) % 7; // Monday = 0
            if (t > now && (wd >= days.Count || days[wd]))
            {
                var hm = t.ToString("HH:mm", Inv);
                return add == 0 ? $"today {hm}" : add == 1 ? $"tomorrow {hm}" : $"{t.ToString("dddd", Inv)} {hm}";
            }
        }
        return null;
    }

    /// <summary>"mkv, .WEBM;zip" → mkv, webm, zip.</summary>
    public static List<string> ParseExtensions(string text)
    {
        var list = new List<string>();
        foreach (var raw in text.Split([' ', '\t', '\r', '\n', ',', ';'], StringSplitOptions.RemoveEmptyEntries))
        {
            var e = raw.Trim('.').ToLowerInvariant();
            if (e.Length > 0 && !list.Contains(e)) list.Add(e);
        }
        return list;
    }
}
