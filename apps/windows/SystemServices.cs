using System.Diagnostics;
using System.Runtime.InteropServices;
using Microsoft.Win32;
using Microsoft.Windows.AppNotifications;
using Microsoft.Windows.AppNotifications.Builder;

namespace OpenDownloadManager;

/// <summary>What the app asks of Windows: opening files, notifications, launch at login, staying awake, power.</summary>
public static class SystemServices
{
    public const string AppName = "Open Download Manager";

    // ------------------------------------------------------------- files, links

    /// <summary>Open a file or folder with its default app.</summary>
    public static void Open(string path)
    {
        try { Process.Start(new ProcessStartInfo(path) { UseShellExecute = true }); }
        catch (Exception e) { App.Store.Report($"Could not open {path}: {e.Message}"); }
    }

    /// <summary>Show a file selected in File Explorer.</summary>
    public static void Reveal(string path)
    {
        try { Process.Start(new ProcessStartInfo("explorer.exe", $"/select,\"{path}\"") { UseShellExecute = false }); }
        catch (Exception e) { App.Store.Report($"Could not open the folder: {e.Message}"); }
    }

    // ------------------------------------------------------------ notifications

    private static bool notificationsReady;

    public static void StartNotifications()
    {
        try
        {
            var manager = AppNotificationManager.Default;
            // clicking a notification brings the app up
            manager.NotificationInvoked += (_, _) => App.UI.TryEnqueue(App.ShowMain);
            manager.Register();
            notificationsReady = true;
        }
        catch
        {
            // no notification platform (e.g. a server session): go without
        }
    }

    public static void StopNotifications()
    {
        if (!notificationsReady) return;
        try { AppNotificationManager.Default.Unregister(); } catch { }
    }

    public static void Notify(string title, string body)
    {
        if (!notificationsReady) return;
        try
        {
            AppNotificationManager.Default.Show(new AppNotificationBuilder().AddText(title).AddText(body).BuildNotification());
        }
        catch { }
    }

    // ---------------------------------------------------------- launch at login

    private const string RunKey = @"Software\Microsoft\Windows\CurrentVersion\Run";

    public static bool AutostartEnabled
    {
        get
        {
            try
            {
                using var key = Registry.CurrentUser.OpenSubKey(RunKey);
                return key?.GetValue(AppName) is string;
            }
            catch { return false; }
        }
    }

    /// <summary>Add or remove the Run entry. Returns what to tell the user when it didn't work.</summary>
    public static string? SetAutostart(bool on)
    {
        try
        {
            using var key = Registry.CurrentUser.CreateSubKey(RunKey);
            if (on) key.SetValue(AppName, $"\"{Environment.ProcessPath}\" --minimized");
            else key.DeleteValue(AppName, throwOnMissingValue: false);
            return null;
        }
        catch (Exception e)
        {
            return $"Could not change the startup setting: {e.Message}";
        }
    }

    // --------------------------------------------------------- keep-awake, power

    [DllImport("kernel32.dll")]
    private static extern uint SetThreadExecutionState(uint flags);

    [DllImport("powrprof.dll", SetLastError = true)]
    private static extern bool SetSuspendState(bool hibernate, bool force, bool wakeupEventsDisabled);

    private const uint ES_CONTINUOUS = 0x80000000, ES_SYSTEM_REQUIRED = 0x00000001;
    private static bool awake;

    /// <summary>Keep the system from idle-sleeping while downloads run. Call on the UI thread (the state belongs to the thread).</summary>
    public static void KeepAwake(bool on)
    {
        if (on == awake) return;
        awake = on;
        SetThreadExecutionState(on ? ES_CONTINUOUS | ES_SYSTEM_REQUIRED : ES_CONTINUOUS);
    }

    public static void Sleep()
    {
        if (!SetSuspendState(false, false, false)) App.Store.Report("Windows refused to sleep.");
    }

    public static void ShutDown()
    {
        try { Process.Start(new ProcessStartInfo("shutdown.exe", "/s /t 0") { UseShellExecute = false, CreateNoWindow = true }); }
        catch (Exception e) { App.Store.Report($"Could not shut down: {e.Message}"); }
    }
}

/// <summary>
/// Notification-area icon with a native menu, through Shell_NotifyIcon. The icon's messages arrive
/// at the main window, which is subclassed for them.
/// </summary>
public sealed class Tray : IDisposable
{
    public enum Command { Show = 1, Add, Grabber, StartMainQueue, PauseAll, ToggleLimiter, Scheduler, Settings, Quit }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct NOTIFYICONDATA
    {
        public int cbSize;
        public IntPtr hWnd;
        public int uID;
        public int uFlags;
        public int uCallbackMessage;
        public IntPtr hIcon;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)] public string szTip;
        public int dwState;
        public int dwStateMask;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 256)] public string szInfo;
        public int uVersion;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 64)] public string szInfoTitle;
        public int dwInfoFlags;
        public Guid guidItem;
        public IntPtr hBalloonIcon;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct POINT { public int X, Y; }

    private delegate IntPtr SubclassProc(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam, nuint id, nuint data);

    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    private static extern bool Shell_NotifyIconW(int message, ref NOTIFYICONDATA data);
    [DllImport("comctl32.dll")]
    private static extern bool SetWindowSubclass(IntPtr hWnd, SubclassProc proc, nuint id, nuint data);
    [DllImport("comctl32.dll")]
    private static extern bool RemoveWindowSubclass(IntPtr hWnd, SubclassProc proc, nuint id);
    [DllImport("comctl32.dll")]
    private static extern IntPtr DefSubclassProc(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern IntPtr LoadImageW(IntPtr instance, string name, uint type, int cx, int cy, uint flags);
    [DllImport("user32.dll")]
    private static extern bool DestroyIcon(IntPtr icon);
    [DllImport("user32.dll")]
    private static extern IntPtr CreatePopupMenu();
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern bool AppendMenuW(IntPtr menu, uint flags, nuint id, string? text);
    [DllImport("user32.dll")]
    private static extern bool DestroyMenu(IntPtr menu);
    [DllImport("user32.dll")]
    private static extern int TrackPopupMenu(IntPtr menu, uint flags, int x, int y, int reserved, IntPtr hWnd, IntPtr rect);
    [DllImport("user32.dll")]
    private static extern bool GetCursorPos(out POINT point);
    [DllImport("user32.dll")]
    private static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")]
    private static extern bool PostMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern uint RegisterWindowMessageW(string name);
    [DllImport("user32.dll")]
    private static extern int GetSystemMetrics(int index);

    private const int NIM_ADD = 0, NIM_MODIFY = 1, NIM_DELETE = 2;
    private const int NIF_MESSAGE = 1, NIF_ICON = 2, NIF_TIP = 4;
    private const uint WM_APP_TRAY = 0x8000 + 41, WM_NULL = 0, WM_LBUTTONUP = 0x0202, WM_RBUTTONUP = 0x0205;
    private const uint MF_STRING = 0, MF_GRAYED = 1, MF_CHECKED = 8, MF_SEPARATOR = 0x800;
    private const uint TPM_RIGHTBUTTON = 2, TPM_RETURNCMD = 0x100;
    private const uint IMAGE_ICON = 1, LR_LOADFROMFILE = 0x10;
    private const int SM_CXSMICON = 49;

    private readonly IntPtr hwnd;
    private readonly SubclassProc proc; // kept alive while subclassed
    private readonly uint taskbarCreated;
    private readonly Action<Command> onCommand;
    private readonly Dictionary<string, IntPtr> icons = [];
    private string state = "idle";
    private string tip = SystemServices.AppName;
    private bool added;

    /// <summary>First line of the menu, e.g. "2 active · 4.1 MB/s".</summary>
    public string StatusLine { get; set; } = "No active downloads";
    public bool LimiterOn { get; set; }

    /// <summary>The icon is showing, so closing the main window may hide the app instead of quitting it.</summary>
    public bool Visible => added;

    public Tray(IntPtr mainWindow, Action<Command> onCommand)
    {
        hwnd = mainWindow;
        this.onCommand = onCommand;
        proc = WndProc;
        // Explorer restarted: put the icon back
        taskbarCreated = RegisterWindowMessageW("TaskbarCreated");
        SetWindowSubclass(hwnd, proc, 1, 0);
        added = Send(NIM_ADD);
    }

    private IntPtr Icon(string name)
    {
        if (icons.TryGetValue(name, out var icon)) return icon;
        var size = GetSystemMetrics(SM_CXSMICON);
        var path = Path.Combine(AppContext.BaseDirectory, "Assets", "tray", $"tray-{name}.ico");
        icon = LoadImageW(IntPtr.Zero, path, IMAGE_ICON, size, size, LR_LOADFROMFILE);
        if (icon == IntPtr.Zero)
            icon = LoadImageW(IntPtr.Zero, Path.Combine(AppContext.BaseDirectory, "Assets", "icon.ico"), IMAGE_ICON, size, size, LR_LOADFROMFILE);
        icons[name] = icon;
        return icon;
    }

    private bool Send(int message)
    {
        var data = new NOTIFYICONDATA
        {
            cbSize = Marshal.SizeOf<NOTIFYICONDATA>(),
            hWnd = hwnd,
            uID = 1,
            uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage = (int)WM_APP_TRAY,
            hIcon = Icon(state),
            szTip = tip.Length > 127 ? tip[..127] : tip,
            szInfo = "",
            szInfoTitle = "",
        };
        return Shell_NotifyIconW(message, ref data);
    }

    /// <summary>Icon ("idle", "downloading", "limited", "paused", "error", "complete") and tooltip.</summary>
    public void Set(string newState, string tooltip)
    {
        if (newState == state && tooltip == tip) return;
        state = newState;
        tip = tooltip;
        if (added) Send(NIM_MODIFY);
    }

    private IntPtr WndProc(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam, nuint id, nuint data)
    {
        if (msg == WM_APP_TRAY)
        {
            var mouse = (uint)(lParam.ToInt64() & 0xFFFF);
            if (mouse == WM_LBUTTONUP) onCommand(Command.Show);
            else if (mouse == WM_RBUTTONUP) ShowMenu();
            return IntPtr.Zero;
        }
        if (msg == taskbarCreated && taskbarCreated != 0) added = Send(NIM_ADD);
        return DefSubclassProc(hWnd, msg, wParam, lParam);
    }

    private void ShowMenu()
    {
        var menu = CreatePopupMenu();
        void Item(Command c, string text, uint flags = MF_STRING) => AppendMenuW(menu, flags, (nuint)(uint)c, text);
        void Separator() => AppendMenuW(menu, MF_SEPARATOR, nuint.Zero, null);
        AppendMenuW(menu, MF_STRING | MF_GRAYED, nuint.Zero, StatusLine);
        Separator();
        Item(Command.Show, "Show Open Download Manager");
        Item(Command.Add, "Add URL…");
        Item(Command.Grabber, "Site Grabber…");
        Separator();
        Item(Command.StartMainQueue, "Start Main Queue");
        Item(Command.PauseAll, "Pause All");
        Item(Command.ToggleLimiter, "Speed Limiter", LimiterOn ? MF_STRING | MF_CHECKED : MF_STRING);
        Separator();
        Item(Command.Scheduler, "Scheduler…");
        Item(Command.Settings, "Settings…");
        Separator();
        Item(Command.Quit, "Quit");
        GetCursorPos(out var at);
        // the menu closes on a click elsewhere only when its owner is in front
        SetForegroundWindow(hwnd);
        var chosen = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_RETURNCMD, at.X, at.Y, 0, hwnd, IntPtr.Zero);
        PostMessageW(hwnd, WM_NULL, IntPtr.Zero, IntPtr.Zero);
        DestroyMenu(menu);
        if (chosen != 0) onCommand((Command)chosen);
    }

    public void Dispose()
    {
        if (added) Send(NIM_DELETE);
        added = false;
        RemoveWindowSubclass(hwnd, proc, 1);
        foreach (var icon in icons.Values)
            if (icon != IntPtr.Zero) DestroyIcon(icon);
        icons.Clear();
    }
}
