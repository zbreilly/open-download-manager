using System.IO.Pipes;
using System.Text;
using System.Text.Json.Nodes;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;

namespace OpenDownloadManager;

public static class Program
{
    [STAThread]
    private static int Main(string[] args)
    {
        // one instance per user: a second launch (an odm:// link, the
        // shortcut again) hands its arguments to the first and exits
        if (!SingleInstance.Acquire(args)) return 0;
        WinRT.ComWrappersSupport.InitializeComWrappers();
        Application.Start(_ =>
        {
            var context = new DispatcherQueueSynchronizationContext(DispatcherQueue.GetForCurrentThread());
            SynchronizationContext.SetSynchronizationContext(context);
            _ = new App(args);
        });
        return 0;
    }
}

public static class SingleInstance
{
    private static Mutex? mutex;
    private static string PipeName => "OpenDownloadManager.Open." + Environment.UserName;

    /// <summary>True when this process is the app's one instance. False when a running one got <paramref name="args"/>.</summary>
    public static bool Acquire(string[] args)
    {
        mutex = new Mutex(true, @"Local\OpenDownloadManager.Instance", out var first);
        if (first)
        {
            new Thread(Listen) { IsBackground = true, Name = "odm-instance" }.Start();
            return true;
        }
        try
        {
            using var pipe = new NamedPipeClientStream(".", PipeName, PipeDirection.Out, PipeOptions.CurrentUserOnly);
            pipe.Connect(3000);
            var bytes = Encoding.UTF8.GetBytes(string.Join("\n", args));
            pipe.Write(bytes, 0, bytes.Length);
        }
        catch
        {
            // the first instance is starting up or going away: nothing to hand over to
        }
        return false;
    }

    private static void Listen()
    {
        while (true)
        {
            try
            {
                using var pipe = new NamedPipeServerStream(PipeName, PipeDirection.In, 1, PipeTransmissionMode.Byte, PipeOptions.CurrentUserOnly);
                pipe.WaitForConnection();
                using var reader = new StreamReader(pipe, Encoding.UTF8);
                var args = reader.ReadToEnd().Split('\n', StringSplitOptions.RemoveEmptyEntries);
                App.UI?.TryEnqueue(() => App.HandleArgs(args, laterLaunch: true));
            }
            catch
            {
                Thread.Sleep(500);
            }
        }
    }
}

/// <summary>
/// Command-line arguments and links handed to the app (<c>odm://add?url=…</c>, plain http(s) links,
/// <c>--minimized</c>). A link from a web page only carries its URL and is marked source "web", so
/// the Add window doesn't contact it or start by itself.
/// </summary>
public static class Incoming
{
    /// <summary>The Add-window parameters for one argument, an empty object for "show the app", or null to ignore it.</summary>
    public static JsonObject? Parse(string arg)
    {
        if (!Uri.TryCreate(arg.Trim(), UriKind.Absolute, out var u)) return null;
        switch (u.Scheme.ToLowerInvariant())
        {
            case "odm":
                if (u.Host.Length > 0 && !u.Host.Equals("add", StringComparison.OrdinalIgnoreCase)) return new JsonObject();
                foreach (var pair in u.Query.TrimStart('?').Split('&', StringSplitOptions.RemoveEmptyEntries))
                {
                    var kv = pair.Split('=', 2);
                    if (kv.Length == 2 && kv[0] == "url")
                    {
                        var target = Uri.UnescapeDataString(kv[1].Replace('+', ' '));
                        return Fmt.IsHttp(target) ? J.Obj(("url", target), ("source", "web")) : null;
                    }
                }
                return null;
            case "http":
            case "https":
                return J.Obj(("url", u.AbsoluteUri), ("source", "web"));
            default:
                return null;
        }
    }
}
