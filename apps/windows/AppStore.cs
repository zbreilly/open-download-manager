using System.Text.Json.Nodes;

namespace OpenDownloadManager;

/// <summary>What changed, for windows that keep themselves up to date.</summary>
public enum Change
{
    /// <summary>List membership, or a row's status, name, category or queue.</summary>
    List,
    Progress,
    Queues,
    Settings,
    Totals,
    /// <summary>Browser extension / bridge status.</summary>
    Ext,
    /// <summary>The power countdown started or was cancelled.</summary>
    Power,
}

public sealed record PowerCountdown(string Action, string Queue, DateTime Deadline);

/// <summary>App-wide state fed by engine events (UI thread only), and the actions windows share.</summary>
public sealed class AppStore
{
    public List<DownloadRow> Rows { get; } = [];
    private readonly Dictionary<string, DownloadRow> index = [];
    public List<JsonObject> Queues { get; private set; } = [];
    public JsonObject Settings { get; private set; } = new();
    public JsonObject Totals { get; private set; } = new();
    public JsonObject Ext { get; private set; } = new();
    public PowerCountdown? Power { get; private set; }
    public string? StartError { get; set; }
    public DateTime? RecentComplete { get; private set; }

    public event Action<Change>? Changed;
    /// <summary>Something went wrong: tell the user.</summary>
    public event Action<string>? Error;

    public void Emit(Change c) => Changed?.Invoke(c);
    public void Report(string message) => Error?.Invoke(message);

    public DownloadRow? Row(string id) => index.GetValueOrDefault(id);

    public JsonObject? Queue(string? id) => Queues.FirstOrDefault(q => J.S(q, "id") == id);

    public string QueueName(string? id) => id == null ? Fmt.Dash : J.Str(Queue(id), "name") ?? Fmt.Dash;

    public List<JsonObject> Categories => J.Objects(Settings, "categories").ToList();

    public bool Setting(string key, bool fallback) => J.B(Settings, key, fallback);

    public void Reload()
    {
        Queues = (TryCall("list_queues") as JsonArray)?.OfType<JsonObject>().Select(Clone).ToList() ?? [];
        Rows.Clear();
        index.Clear();
        foreach (var d in (TryCall("list_downloads") as JsonArray)?.OfType<JsonObject>() ?? []) Upsert(Clone(d));
        Settings = TryCall("get_settings") as JsonObject ?? new JsonObject();
        Totals = TryCall("totals") as JsonObject ?? new JsonObject();
        Ext = TryCall("extension_status") as JsonObject ?? new JsonObject();
    }

    private static JsonNode? TryCall(string method)
    {
        try { return Engine.Call(method); } catch { return null; }
    }

    private static JsonObject Clone(JsonObject o) => (JsonObject)o.DeepClone();

    private void Upsert(JsonObject d)
    {
        var id = J.S(d, "id");
        var queue = QueueName(J.Str(d, "queueId"));
        if (index.TryGetValue(id, out var row))
        {
            row.Apply(d, queue);
        }
        else
        {
            row = new DownloadRow(d, queue);
            Rows.Add(row);
            index[id] = row;
        }
    }

    /// <summary>An engine event, already on the UI thread.</summary>
    public void Handle(string name, string json)
    {
        JsonNode? v;
        try { v = JsonNode.Parse(json); } catch { return; }
        switch (name)
        {
            case "progress_batch":
                var statusChanged = false;
                foreach (var p in (v as JsonArray)?.OfType<JsonObject>() ?? [])
                    if (Row(J.S(p, "id")) is DownloadRow r) statusChanged |= r.Progress(p);
                Emit(statusChanged ? Change.List : Change.Progress);
                break;
            case "download_added":
            case "download_changed":
                if (v is JsonObject d) { Upsert(d); Emit(Change.List); }
                break;
            case "download_removed":
                if (v is JsonValue idv && idv.TryGetValue<string>(out var id) && index.Remove(id, out var gone))
                {
                    Rows.Remove(gone);
                    Emit(Change.List);
                }
                break;
            case "totals":
                if (v is JsonObject t)
                {
                    Totals = t;
                    foreach (var r in Rows)
                        if (r.Status is "downloading" or "connecting") r.SampleSpeed();
                    SystemServices.KeepAwake(J.N(t, "active") > 0);
                    Emit(Change.Totals);
                }
                break;
            case "queues_changed":
                if (v is JsonArray qs)
                {
                    Queues = qs.OfType<JsonObject>().Select(Clone).ToList();
                    foreach (var r in Rows) r.SetQueueName(QueueName(r.QueueId));
                    Emit(Change.Queues);
                }
                break;
            case "settings_changed":
                if (v is JsonObject s) { Settings = s; Emit(Change.Settings); }
                break;
            case "extension_status":
                if (v is JsonObject e)
                {
                    // the event carries connection state only; keep the bridge error and port
                    e["bridgeError"] = Ext["bridgeError"]?.DeepClone();
                    e["port"] = Ext["port"]?.DeepClone();
                    if (!J.Same(e, Ext)) { Ext = e; Emit(Change.Ext); }
                }
                break;
            case "bridge_error":
                Ext["bridgeError"] = J.OrNull(J.Str(v, "error"));
                Emit(Change.Ext);
                break;
            case "download_completed":
                if (v is JsonObject done) Completed(done);
                break;
            case "download_failed":
                if (v is JsonObject failed && Setting("notifyFailure", true))
                    SystemServices.Notify($"Download failed: {J.S(failed, "fileName")}", J.Str(failed, "error") ?? "Unknown error");
                break;
            case "queue_finished":
                if (v is JsonObject q) QueueFinished(q);
                break;
            case "show_add":
                if (v is JsonObject add) App.OpenAdd(add);
                break;
            case "show_links":
                if (v is JsonObject links) App.OpenGrabber(links);
                break;
        }
    }

    private void Completed(JsonObject d)
    {
        RecentComplete = DateTime.Now;
        var path = System.IO.Path.Combine(J.S(d, "saveDir"), J.S(d, "fileName"));
        switch (J.S(d, "whenFinished"))
        {
            case "openFile": SystemServices.Open(path); break;
            case "openFolder": SystemServices.Reveal(path); break;
            case "none": break;
            default:
                if (Setting("notifyComplete", true)) SystemServices.Notify("Download complete", J.S(d, "fileName"));
                break;
        }
        if (Setting("showCompleteDialog", false)) App.OpenProgress(J.S(d, "id"));
        Emit(Change.Totals);
    }

    private void QueueFinished(JsonObject q)
    {
        var name = J.S(q, "name");
        if (Setting("notifyQueueFinished", true)) SystemServices.Notify("Queue finished", $"{name} has no more files to download");
        var action = J.S(q, "onFinish");
        if (action is not ("sleep" or "shutdown" or "exit")) return;
        Power = new PowerCountdown(action, name, DateTime.Now.AddSeconds(60));
        App.ShowMain();
        Emit(Change.Power);
    }

    public void CancelPower()
    {
        Power = null;
        Emit(Change.Power);
    }

    /// <summary>The countdown ran out.</summary>
    public void RunPower()
    {
        if (Power is not PowerCountdown p) return;
        Power = null;
        Emit(Change.Power);
        switch (p.Action)
        {
            case "exit": App.Quit(); break;
            case "sleep": SystemServices.Sleep(); break;
            case "shutdown":
                Engine.Shutdown();
                SystemServices.ShutDown();
                break;
        }
    }

    // ------------------------------------------------------------------ actions

    /// <summary>Fire an engine call off the UI thread; errors are shown to the user.</summary>
    public async void Act(string method, JsonObject? args = null)
    {
        try { await Engine.CallAsync(method, args); }
        catch (Exception e) { Report(e.Message); }
    }

    public void Resume(IEnumerable<string> ids) => Act("resume", J.Obj(("ids", J.List(ids))));
    public void Pause(IEnumerable<string> ids) => Act("pause", J.Obj(("ids", J.List(ids))));
    public void Delete(IEnumerable<string> ids, bool files) => Act("delete", J.Obj(("ids", J.List(ids)), ("deleteFiles", files)));
    public void SetGlobalLimit(ulong? bps) => Act("set_global_limit", J.Obj(("bps", bps is ulong b ? JsonValue.Create(b) : null)));

    public void ToggleLimit() =>
        SetGlobalLimit(J.B(Settings, "globalLimitEnabled") ? null : J.N(Settings, "globalLimitBps"));

    /// <summary>Put an extension in a category, and move the listed downloads of that type along.</summary>
    public void AssignExtension(string ext, string category)
    {
        Act("assign_extensions", J.Obj(("category", category), ("extensions", J.List([ext]))));
        var same = Rows.Where(r => r.FileExtension == ext && r.Category != category).Select(r => r.Id).ToList();
        if (same.Count > 0) Act("set_category", J.Obj(("ids", J.List(same)), ("category", category)));
    }

    public void OpenFile(DownloadRow r)
    {
        if (File.Exists(r.FullPath)) SystemServices.Open(r.FullPath);
        else Report("The file is no longer there. It may have been moved or deleted.");
    }

    /// <summary>Show the finished file, else its part file, else the folder.</summary>
    public void Reveal(DownloadRow r)
    {
        var part = r.FullPath + ".odmpart";
        if (File.Exists(r.FullPath)) SystemServices.Reveal(r.FullPath);
        else if (File.Exists(part)) SystemServices.Reveal(part);
        else SystemServices.Open(r.SaveDir);
    }

    /// <summary>
    /// Save settings. When the engine refuses them (e.g. the new bridge port is taken) the error
    /// is shown and the engine's copy comes back.
    /// </summary>
    public async void SaveSettings(JsonObject wanted)
    {
        var s = Clone(wanted);
        if (J.B(Settings, "launchAtLogin") != J.B(s, "launchAtLogin"))
        {
            // keep the setting equal to what the system actually did
            if (SystemServices.SetAutostart(J.B(s, "launchAtLogin")) is string problem) Report(problem);
            s["launchAtLogin"] = SystemServices.AutostartEnabled;
        }
        Settings = s;
        Emit(Change.Settings);
        try
        {
            await Engine.CallAsync("save_settings", J.Obj(("settings", Clone(s))));
        }
        catch (Exception e)
        {
            Report(e.Message);
            if (TryCall("get_settings") is JsonObject real)
            {
                Settings = real;
                Emit(Change.Settings);
            }
        }
    }

    /// <summary>The Run key can be changed outside the app: follow it.</summary>
    public void SyncAutostart()
    {
        var real = SystemServices.AutostartEnabled;
        if (Settings.Count == 0 || J.B(Settings, "launchAtLogin") == real) return;
        Settings["launchAtLogin"] = real;
        Act("save_settings", J.Obj(("settings", Clone(Settings))));
    }

    /// <summary>
    /// Three-way merge of settings: start from <paramref name="latest"/> and apply every field whose
    /// value in <paramref name="edited"/> differs from <paramref name="based"/>.
    /// </summary>
    public static JsonObject Merge(JsonObject based, JsonObject edited, JsonObject latest)
    {
        if (J.Same(edited, based)) return Clone(latest);
        if (J.Same(latest, based)) return Clone(edited);
        var result = Clone(latest);
        foreach (var key in based.Select(p => p.Key).Union(edited.Select(p => p.Key)).ToList())
        {
            if (J.Same(based[key], edited[key])) continue;
            if (edited.ContainsKey(key)) result[key] = edited[key]?.DeepClone();
            else result.Remove(key);
        }
        return result;
    }

    /// <summary>Tray icon state, by the brand priority rules.</summary>
    public string TrayState
    {
        get
        {
            var recent = RecentComplete is DateTime t && (DateTime.Now - t).TotalSeconds < 5;
            var active = J.N(Totals, "active");
            if (J.N(Totals, "failed") > 0 && active == 0 && !recent) return "error";
            if (active > 0 && J.B(Totals, "limitOn")) return "limited";
            if (active > 0) return "downloading";
            if (recent) return "complete";
            if (J.N(Totals, "paused") > 0) return "paused";
            return "idle";
        }
    }
}
