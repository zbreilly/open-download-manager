using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace OpenDownloadManager;

/// <summary>An error reported by the engine.</summary>
public sealed class EngineException(string message) : Exception(message);

/// <summary>
/// Thin wrapper over the Rust engine's C interface (odm.h, odm_ffi.dll).
/// Every call is a method name plus JSON arguments, and returns JSON.
/// </summary>
public static class Engine
{
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate void EventCallback(IntPtr name, IntPtr json, IntPtr ctx);

    [DllImport("odm_ffi", CallingConvention = CallingConvention.Cdecl)]
    private static extern int odm_start(byte[] dataDir, EventCallback? cb, IntPtr ctx);

    [DllImport("odm_ffi", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr odm_call(byte[] method, byte[] args);

    [DllImport("odm_ffi", CallingConvention = CallingConvention.Cdecl)]
    private static extern void odm_free(IntPtr s);

    [DllImport("odm_ffi", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr odm_last_error();

    [DllImport("odm_ffi", CallingConvention = CallingConvention.Cdecl)]
    private static extern void odm_shutdown();

    // kept alive for as long as the engine may call it
    private static EventCallback? callback;

    /// <summary>Raised on the engine's "odm-events" thread: (name, JSON).</summary>
    public static event Action<string, string>? Event;

    private static byte[] Utf8Z(string s) => Encoding.UTF8.GetBytes(s + "\0");

    /// <summary>Open the database in <paramref name="dataDir"/> and start the engine and the browser bridge.</summary>
    public static void Start(string dataDir)
    {
        callback = (name, json, _) =>
        {
            try
            {
                var n = Marshal.PtrToStringUTF8(name);
                var j = Marshal.PtrToStringUTF8(json);
                if (n != null && j != null) Event?.Invoke(n, j);
            }
            catch
            {
                // nothing may propagate into the engine
            }
        };
        var rc = odm_start(Utf8Z(dataDir), callback, IntPtr.Zero);
        if (rc != 0)
        {
            var msg = Marshal.PtrToStringUTF8(odm_last_error());
            throw new EngineException(string.IsNullOrEmpty(msg) ? $"engine failed to start ({rc})" : msg);
        }
    }

    /// <summary>Stop the bridge and downloads, waiting up to 5 s for them to flush. Runs once.</summary>
    public static void Shutdown()
    {
        try { odm_shutdown(); } catch (DllNotFoundException) { }
    }

    /// <summary>Call an engine method. Blocking methods (probe_url, grab, refresh_link) belong on <see cref="CallAsync"/>.</summary>
    public static JsonNode? Call(string method, JsonObject? args = null)
    {
        var ptr = odm_call(Utf8Z(method), Utf8Z((args ?? new JsonObject()).ToJsonString()));
        if (ptr == IntPtr.Zero) throw new EngineException("engine returned nothing");
        string text;
        try { text = Marshal.PtrToStringUTF8(ptr) ?? ""; }
        finally { odm_free(ptr); }
        JsonNode? reply;
        try { reply = JsonNode.Parse(text); }
        catch (JsonException e) { throw new EngineException($"could not read engine reply for {method}: {e.Message}"); }
        if (reply is JsonObject o && o["err"] is JsonNode err) throw new EngineException(err.ToString());
        // detach so the result can be put into another tree
        return (reply as JsonObject)?["ok"]?.DeepClone();
    }

    /// <summary>Call off the UI thread.</summary>
    public static Task<JsonNode?> CallAsync(string method, JsonObject? args = null)
    {
        var copy = args?.DeepClone() as JsonObject;
        return Task.Run(() => Call(method, copy));
    }

    /// <summary>Call and return the error text instead of throwing (null when it worked).</summary>
    public static string? Try(string method, JsonObject? args = null)
    {
        try { Call(method, args); return null; }
        catch (Exception e) { return e.Message; }
    }
}

/// <summary>Tolerant readers for the engine's JSON (missing or null fields give defaults).</summary>
public static class J
{
    public static string S(JsonNode? o, string key) => Str(o, key) ?? "";

    public static string? Str(JsonNode? o, string key)
    {
        if (o is JsonObject obj && obj[key] is JsonValue v && v.TryGetValue<string>(out var s)) return s;
        return null;
    }

    public static bool B(JsonNode? o, string key, bool fallback = false)
    {
        if (o is JsonObject obj && obj[key] is JsonValue v && v.TryGetValue<bool>(out var b)) return b;
        return fallback;
    }

    public static ulong? Num(JsonNode? o, string key)
    {
        if (o is not JsonObject obj || obj[key] is not JsonValue v) return null;
        if (v.TryGetValue<ulong>(out var u)) return u;
        if (v.TryGetValue<long>(out var l)) return l < 0 ? 0 : (ulong)l;
        if (v.TryGetValue<int>(out var i)) return i < 0 ? 0 : (ulong)i;
        if (v.TryGetValue<double>(out var d) && double.IsFinite(d)) return d <= 0 ? 0 : d >= 1.8e19 ? ulong.MaxValue : (ulong)d;
        return null;
    }

    public static ulong N(JsonNode? o, string key) => Num(o, key) ?? 0;

    public static JsonArray Arr(JsonNode? o, string key) => (o as JsonObject)?[key] as JsonArray ?? new JsonArray();

    public static IEnumerable<JsonObject> Objects(JsonNode? o, string key) => Arr(o, key).OfType<JsonObject>();

    public static List<string> Strings(JsonNode? o, string key) =>
        Arr(o, key).OfType<JsonValue>().Select(v => v.TryGetValue<string>(out var s) ? s : null).OfType<string>().ToList();

    public static JsonObject Obj(params (string Key, JsonNode? Value)[] items)
    {
        var o = new JsonObject();
        foreach (var (k, v) in items) o[k] = v;
        return o;
    }

    public static JsonArray List(IEnumerable<string> items)
    {
        var a = new JsonArray();
        foreach (var i in items) a.Add(i);
        return a;
    }

    /// <summary>JSON null for an empty string.</summary>
    public static JsonNode? OrNull(string? s) => string.IsNullOrWhiteSpace(s) ? null : JsonValue.Create(s.Trim());

    public static bool Same(JsonNode? a, JsonNode? b) => JsonNode.DeepEquals(a, b);
}
