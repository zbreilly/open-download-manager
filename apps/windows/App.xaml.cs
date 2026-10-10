using System.Text.Json.Nodes;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Windows.ApplicationModel.DataTransfer;

namespace OpenDownloadManager;

public partial class App : Application
{
    public static AppStore Store { get; } = new();
    public static DispatcherQueue UI { get; private set; } = null!;
    public static MainWindow? Main { get; private set; }
    public static Tray? Tray { get; private set; }

    private static readonly Dictionary<string, ProgressWindow> progress = [];
    private static readonly List<Window> windows = [];
    private static SchedulerWindow? scheduler;
    private static GrabberWindow? grabber;
    private static SettingsWindow? settings;
    private static string[] launchArgs = [];
    private static bool quitting;

    public App(string[] args)
    {
        launchArgs = args;
        InitializeComponent();
        UnhandledException += (_, e) =>
        {
            // a failed click handler must not take the downloads down with it
            e.Handled = true;
            Store.Report(e.Message);
        };
    }

    protected override void OnLaunched(LaunchActivatedEventArgs e)
    {
        UI = DispatcherQueue.GetForCurrentThread();
        Engine.Event += (name, json) => UI.TryEnqueue(() => Store.Handle(name, json));
        var dataDir = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), "app.opendm.desktop");
        try
        {
            Engine.Start(dataDir);
            Store.Reload();
            Store.SyncAutostart();
        }
        catch (Exception ex)
        {
            Store.StartError = ex.Message;
        }
        SystemServices.StartNotifications();
        SystemServices.KeepAwake(J.N(Store.Totals, "active") > 0);

        Main = new MainWindow();
        Tray = new Tray(Win.Handle(Main), TrayCommand);
        Store.Changed += c =>
        {
            if (c == Change.Settings) foreach (var w in windows.ToList()) Win.ApplyTheme(w);
            if (c is Change.Totals or Change.Settings) UpdateTray();
        };
        UpdateTray();
        WatchClipboard();

        var minimized = launchArgs.Contains("--minimized") || launchArgs.Contains("--hidden");
        if (!(minimized && Tray.Visible)) ShowMain();
        HandleArgs(launchArgs, laterLaunch: false);
    }

    // ------------------------------------------------------------------ windows

    /// <summary>Every window registers here so theme changes reach it.</summary>
    public static void Register(Window w)
    {
        windows.Add(w);
        w.Closed += (_, _) => windows.Remove(w);
    }

    public static void ShowMain()
    {
        if (Main != null) Win.Show(Main);
    }

    /// <summary>Open an Add window. <paramref name="p"/> is the engine's show_add payload, {batch: true}, or {url, source: "web"}.</summary>
    public static void OpenAdd(JsonObject? p = null) => Win.Show(new AddWindow(p ?? new JsonObject()));

    public static void OpenProgress(string id)
    {
        if (progress.TryGetValue(id, out var open))
        {
            Win.Show(open);
            return;
        }
        if (Store.Row(id) == null) return;
        var w = new ProgressWindow(id);
        progress[id] = w;
        w.Closed += (_, _) => progress.Remove(id);
        Win.Show(w);
    }

    public static void OpenScheduler()
    {
        if (scheduler == null)
        {
            scheduler = new SchedulerWindow();
            scheduler.Closed += (_, _) => scheduler = null;
        }
        Win.Show(scheduler);
    }

    /// <summary>Show the grabber. With <paramref name="links"/> (a show_links payload) it lists those instead of scanning a page.</summary>
    public static void OpenGrabber(JsonObject? links = null)
    {
        if (grabber == null)
        {
            grabber = new GrabberWindow();
            grabber.Closed += (_, _) => grabber = null;
        }
        if (links != null) grabber.TakeList(links);
        Win.Show(grabber);
    }

    public static void OpenSettings()
    {
        if (settings == null)
        {
            settings = new SettingsWindow();
            settings.Closed += (_, _) => settings = null;
        }
        Win.Show(settings);
    }

    /// <summary>Arguments of this launch, or of a later one handed over by the second instance.</summary>
    public static void HandleArgs(string[] args, bool laterLaunch)
    {
        var any = false;
        foreach (var a in args.Where(a => !a.StartsWith('-')))
        {
            if (Incoming.Parse(a) is not JsonObject p) continue;
            any = true;
            if (p.Count == 0) ShowMain(); else OpenAdd(p);
        }
        if (laterLaunch && !any) ShowMain();
    }

    /// <summary>
    /// The main window's close button: stay in the notification area when there is an icon there
    /// and the user wants that, otherwise quit. True when the window should only be hidden.
    /// </summary>
    public static bool HideInsteadOfClosing => !quitting && Store.Setting("minimizeToTray", true) && Tray?.Visible == true;

    public static void Quit()
    {
        if (quitting) return;
        quitting = true;
        foreach (var w in windows.ToList())
        {
            try { w.AppWindow.Hide(); } catch { }
        }
        // stops downloads and saves state (waits up to 5 s)
        Engine.Shutdown();
        SystemServices.KeepAwake(false);
        SystemServices.StopNotifications();
        Tray?.Dispose();
        Current.Exit();
    }

    // --------------------------------------------------------------------- tray

    private static void TrayCommand(Tray.Command c)
    {
        switch (c)
        {
            case Tray.Command.Show: ShowMain(); break;
            case Tray.Command.Add: OpenAdd(); break;
            case Tray.Command.Grabber: OpenGrabber(); break;
            case Tray.Command.StartMainQueue: Store.Act("start_queue", J.Obj(("id", "main"))); break;
            case Tray.Command.PauseAll: Store.Act("pause_all"); break;
            case Tray.Command.ToggleLimiter: Store.ToggleLimit(); break;
            case Tray.Command.Scheduler: OpenScheduler(); break;
            case Tray.Command.Settings: OpenSettings(); break;
            case Tray.Command.Quit: Quit(); break;
        }
    }

    private static void UpdateTray()
    {
        if (Tray == null) return;
        var active = J.N(Store.Totals, "active");
        var line = active > 0 ? $"{active} active · {Fmt.Speed(J.N(Store.Totals, "speedBps"))}" : "No active downloads";
        Tray.StatusLine = line;
        Tray.LimiterOn = J.B(Store.Settings, "globalLimitEnabled");
        var showSpeed = Store.Setting("traySpeed", false) && active > 0;
        Tray.Set(Store.TrayState, showSpeed || active > 0 ? $"{SystemServices.AppName}\n{line}" : SystemServices.AppName);
    }

    // ---------------------------------------------------------------- clipboard

    /// <summary>Offer copied download links (by file type) in the Add window, when turned on.</summary>
    private static void WatchClipboard()
    {
        try
        {
            Clipboard.ContentChanged += async (_, _) =>
            {
                try
                {
                    if (!Store.Setting("clipboardMonitor", false)) return;
                    var content = Clipboard.GetContent();
                    if (!content.Contains(StandardDataFormats.Text)) return;
                    var text = (await content.GetTextAsync()).Trim();
                    if (text.Length >= 4096 || text.Any(char.IsWhiteSpace) || !Fmt.IsHttp(text)) return;
                    var ext = Fmt.Extension(Fmt.UrlFileName(text));
                    if (ext.Length == 0 || !J.Strings(Store.Settings, "captureExtensions").Contains(ext)) return;
                    if (Store.Rows.Any(r => r.Url == text && r.Status != "error")) return;
                    OpenAdd(J.Obj(("url", text)));
                }
                catch
                {
                    // the clipboard is busy or holds something unreadable
                }
            };
        }
        catch
        {
            // no clipboard access in this session
        }
    }
}
