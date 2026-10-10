using System.ComponentModel;
using System.Text.Json.Nodes;
using Microsoft.UI;
using Microsoft.UI.Xaml.Media;

namespace OpenDownloadManager;

/// <summary>One download as the list shows it. The record itself is the engine's JSON.</summary>
public sealed class DownloadRow : INotifyPropertyChanged
{
    public event PropertyChangedEventHandler? PropertyChanged;

    public JsonObject Data { get; private set; }
    public string QueueName { get; private set; } = Fmt.Dash;

    public DownloadRow(JsonObject data, string queueName)
    {
        Data = data;
        QueueName = queueName;
    }

    public string Id => J.S(Data, "id");
    public string Name => J.S(Data, "fileName");
    public string Url => J.S(Data, "url");
    public string SaveDir => J.S(Data, "saveDir");
    public string Status => J.S(Data, "status");
    public string Category => J.S(Data, "category");
    public string? QueueId => J.Str(Data, "queueId");
    public string AddedAt => J.S(Data, "addedAt");
    public ulong Received => J.N(Data, "receivedBytes");
    public ulong? Total => J.Num(Data, "totalBytes");
    public ulong SpeedBps => J.N(Data, "speedBps");
    public ulong? EtaSecs => J.Num(Data, "etaSecs");
    public bool IsActive => Fmt.IsActive(Status);
    public bool IsComplete => Status == "complete";
    public string FullPath => System.IO.Path.Combine(SaveDir, Name);
    public string FileExtension => Fmt.Extension(Name);

    // what the cells show
    public string SizeText => Fmt.Bytes(Total);
    public double Percent => Fmt.Percent(Received, Total, IsComplete);
    public string PercentText => $"{Fmt.Percent(Received, Total, IsComplete)}%";
    public string SpeedText => IsActive ? Fmt.Speed(SpeedBps) : Fmt.Dash;
    public string EtaText => Status == "downloading" ? Fmt.Eta(EtaSecs) : Fmt.Dash;
    public string StatusText => Fmt.StatusLabel(Status, J.Str(Data, "scheduledAt"));
    public string AddedText => Fmt.Added(AddedAt);
    public Brush StatusBrush => Palette.Status(Status);
    public string Glyph => Palette.FileGlyph(Category);

    // sort keys
    public ulong SpeedSort => IsActive ? SpeedBps : 0;
    public ulong EtaSort => Status == "downloading" ? EtaSecs ?? ulong.MaxValue : ulong.MaxValue;

    /// <summary>Replace the whole record, keeping the speed history collected here when the new one has none.</summary>
    public void Apply(JsonObject data, string queueName)
    {
        if (J.Arr(data, "speedHistory").Count == 0 && Data["speedHistory"] is JsonArray history)
            data["speedHistory"] = history.DeepClone();
        Data = data;
        QueueName = queueName;
        Changed();
    }

    /// <summary>Merge a progress item. True when the status changed.</summary>
    public bool Progress(JsonObject p)
    {
        var statusChanged = J.S(p, "status") != Status;
        foreach (var key in new[] { "receivedBytes", "speedBps", "etaSecs", "activeConnections", "segments", "status" })
            if (p.ContainsKey(key)) Data[key] = p[key]?.DeepClone();
        if (p["totalBytes"] is JsonNode total) Data["totalBytes"] = total.DeepClone();
        Changed();
        return statusChanged;
    }

    /// <summary>One more sample for the 60-second speed graph.</summary>
    public void SampleSpeed()
    {
        if (Data["speedHistory"] is not JsonArray h) Data["speedHistory"] = h = new JsonArray();
        h.Add(SpeedBps);
        while (h.Count > 60) h.RemoveAt(0);
    }

    public void SetQueueName(string name)
    {
        if (QueueName == name) return;
        QueueName = name;
        Changed();
    }

    // an empty name tells bindings that every property may have changed
    private void Changed() => PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(string.Empty));
}

/// <summary>Status colors and glyphs (Segoe Fluent Icons).</summary>
public static class Palette
{
    private static readonly Dictionary<string, SolidColorBrush> cache = [];

    private static SolidColorBrush Brush(byte r, byte g, byte b)
    {
        var key = $"{r},{g},{b}";
        if (!cache.TryGetValue(key, out var brush)) cache[key] = brush = new SolidColorBrush(ColorHelper.FromArgb(255, r, g, b));
        return brush;
    }

    public static SolidColorBrush Status(string status) => status switch
    {
        "downloading" or "connecting" or "assembling" => Brush(0x3B, 0x8E, 0xEA),
        "paused" => Brush(0xD6, 0x8A, 0x00),
        "error" => Brush(0xE0, 0x43, 0x43),
        "complete" => Brush(0x2E, 0xA0, 0x5A),
        _ => Brush(0x8A, 0x8A, 0x8A),
    };

    public static string FileGlyph(string category) => category switch
    {
        "video" => "\uE714",
        "music" => "\uE8D6",
        "documents" => "\uE8A5",
        "compressed" => "\uF012",
        "programs" => "\uECAA",
        _ => "\uE7C3",
    };

    public static string CategoryGlyph(string icon) => icon switch
    {
        "cat-video" => "\uE714",
        "cat-music" => "\uE8D6",
        "cat-documents" => "\uE8A5",
        "cat-compressed" => "\uF012",
        "cat-programs" => "\uECAA",
        "cat-images" => "\uEB9F",
        "file-disc" => "\uE958",
        _ => "\uE8B7",
    };
}
