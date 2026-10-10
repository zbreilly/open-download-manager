using System.Text.Json.Nodes;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;

namespace OpenDownloadManager;

/// <summary>
/// Settings. Changes apply as you make them (saved after a short pause), and settings changed
/// elsewhere (tray, toolbar, extension) flow in without losing edits in progress.
/// </summary>
public sealed partial class SettingsWindow : Window
{
    private readonly AppStore store = App.Store;
    private JsonObject draft;
    /// <summary>The engine's settings the draft is based on.</summary>
    private JsonObject based;
    /// <summary>Put the draft's values into the controls of the page that is showing.</summary>
    private readonly List<Action> loaders = [];
    private bool quiet;
    private int saveGeneration;
    private string page = "general";

    public SettingsWindow()
    {
        draft = (JsonObject)store.Settings.DeepClone();
        based = (JsonObject)store.Settings.DeepClone();
        InitializeComponent();
        Win.Setup(this, "Settings", 940, 680);
        store.Changed += OnChanged;
        Closed += (_, _) =>
        {
            store.Changed -= OnChanged;
            // pending edits are saved right away
            SaveNow();
        };
        Nav.SelectedItem = Nav.MenuItems[0];
    }

    private void OnChanged(Change c)
    {
        if (c == Change.Ext && page == "browser") Build();
        if (c != Change.Settings) return;
        // settings changed elsewhere: take them without losing edits here
        var merged = AppStore.Merge(based, draft, store.Settings);
        based = (JsonObject)store.Settings.DeepClone();
        if (J.Same(merged, draft)) return;
        var categoriesChanged = !J.Same(merged["categories"], draft["categories"]);
        draft = merged;
        if (categoriesChanged && page == "types") Build(); else Load();
    }

    // ----------------------------------------------------------------- plumbing

    /// <summary>A control changed: put its value in the draft and save soon.</summary>
    private async void Put(string key, JsonNode? value)
    {
        if (quiet || J.Same(draft[key], value)) return;
        draft[key] = value;
        var mine = ++saveGeneration;
        await Task.Delay(400);
        if (mine == saveGeneration) SaveNow();
    }

    private void SaveNow()
    {
        saveGeneration++;
        var merged = AppStore.Merge(based, draft, store.Settings);
        if (J.Same(merged, store.Settings)) return;
        store.SaveSettings(merged);
        // what was stored (e.g. launch at login adjusted to what happened)
        draft = (JsonObject)store.Settings.DeepClone();
        based = (JsonObject)store.Settings.DeepClone();
        Load();
    }

    private void Load()
    {
        quiet = true;
        foreach (var load in loaders) load();
        quiet = false;
    }

    private void Add(UIElement e) => Pane.Children.Add(e);

    private Border Toggle(string key, string label, string? note = null)
    {
        var s = Ui.Switch();
        s.Toggled += (_, _) => Put(key, s.IsOn);
        loaders.Add(() => s.IsOn = J.B(draft, key));
        return Ui.Row(label, s, note);
    }

    private Border Number(string key, string label, double min, double max, string? note = null)
    {
        var box = Ui.Number(min, max);
        box.ValueChanged += (_, _) =>
        {
            if (!double.IsNaN(box.Value)) Put(key, (ulong)Math.Clamp(Math.Round(box.Value), min, max));
        };
        loaders.Add(() => box.Value = J.N(draft, key));
        return Ui.Row(label, box, note);
    }

    private Border Text(string key, string label, bool password = false)
    {
        if (password)
        {
            var secret = new PasswordBox();
            secret.PasswordChanged += (_, _) => Put(key, secret.Password);
            loaders.Add(() => { if (secret.Password != J.S(draft, key)) secret.Password = J.S(draft, key); });
            return Ui.Wide(label, secret);
        }
        var box = new TextBox();
        box.TextChanged += (_, _) => Put(key, box.Text);
        loaders.Add(() => { if (box.Text != J.S(draft, key)) box.Text = J.S(draft, key); });
        return Ui.Wide(label, box);
    }

    private Border Choice(string key, string label, (string, string)[] items)
    {
        var box = Ui.Combo(items.Select(i => (i.Item1, i.Item2)));
        box.SelectionChanged += (_, _) => { if (box.Value().Length > 0) Put(key, box.Value()); };
        loaders.Add(() => box.Select(J.S(draft, key)));
        return Ui.Row(label, box);
    }

    /// <summary>A speed in MB/s with its on/off switch.</summary>
    private Border Speed(string key, string enabledKey, string label)
    {
        var box = new TextBox { Width = 90 };
        var unit = new TextBlock { Text = "MB/s", VerticalAlignment = VerticalAlignment.Center };
        var on = Ui.Switch();
        var panel = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        panel.Children.Add(box);
        panel.Children.Add(unit);
        panel.Children.Add(on);
        // text that isn't a usable speed is ignored; the old value stays
        box.TextChanged += (_, _) => { if (Fmt.LimitBps(box.Text) is ulong bps) Put(key, bps); };
        on.Toggled += (_, _) => Put(enabledKey, on.IsOn);
        loaders.Add(() =>
        {
            if (Fmt.LimitBps(box.Text) != J.N(draft, key)) box.Text = Fmt.Mbps(J.N(draft, key));
            on.IsOn = J.B(draft, enabledKey);
        });
        return Ui.Row(label, panel);
    }

    private Border Folder(string key, string label, string? note = null)
    {
        var path = new TextBlock { Style = (Style)Application.Current.Resources["Num"], MaxWidth = 360 };
        var choose = new Button { Content = "Choose…" };
        var panel = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 10 };
        panel.Children.Add(path);
        panel.Children.Add(choose);
        choose.Click += async (_, _) =>
        {
            if (await Win.PickFolder(this) is not string picked) return;
            Put(key, picked);
            Load();
        };
        loaders.Add(() => path.Text = J.S(draft, key));
        return Ui.Row(label, panel, note);
    }

    // -------------------------------------------------------------------- pages

    private void Nav_SelectionChanged(NavigationView sender, NavigationViewSelectionChangedEventArgs args)
    {
        if (args.SelectedItem is NavigationViewItem { Tag: string tag }) page = tag;
        Build();
    }

    private void Build()
    {
        Pane.Children.Clear();
        loaders.Clear();
        switch (page)
        {
            case "general": General(); break;
            case "connection": Connection(); break;
            case "browser": Browser(); break;
            case "types": FileTypes(); break;
            case "proxy": Proxy(); break;
            case "notifications": Notifications(); break;
            case "advanced": Advanced(); break;
        }
        Load();
    }

    private void General()
    {
        Add(Ui.Section("General"));
        Add(Toggle("launchAtLogin", "Start when I sign in to Windows"));
        Add(Toggle("minimizeToTray", "Keep running in the notification area when the window is closed"));
        Add(Toggle("clipboardMonitor", "Watch the clipboard for download links",
            "Copying a link to a file type listed under Browser opens the Add window."));
        Add(Toggle("showProgressOnStart", "Show the progress window when a download starts"));
        Add(Toggle("showCompleteDialog", "Show the download window when a download completes"));
        Add(Toggle("confirmDelete", "Confirm before deleting downloads"));
        Add(Ui.Section("Appearance"));
        Add(Choice("theme", "Theme", [("dark", "Dark"), ("light", "Light"), ("system", "Use Windows setting")]));
        Add(Toggle("traySpeed", "Show download speed in the notification-area tooltip"));
    }

    private void Connection()
    {
        Add(Ui.Section("Connections"));
        Add(Number("defaultConnections", "Connections per download", 1, 32,
            "Files are split dynamically: a connection that finishes takes over half of the largest remaining piece."));
        Add(Number("maxSimultaneous", "Simultaneous downloads", 1, 64, "Outside of queues. Each queue sets its own limit."));
        Add(Ui.Section("Speed limits"));
        Add(Speed("globalLimitBps", "globalLimitEnabled", "Global limit (all downloads combined)"));
        Add(Speed("perDownloadLimitBps", "perDownloadLimitEnabled", "Per-download limit"));
        Add(Toggle("limitOnStartup", "Turn on the global limit when the app starts"));
    }

    private void Browser()
    {
        var ext = store.Ext;
        var error = J.Str(ext, "bridgeError");
        var browsers = J.Strings(ext, "browsers");
        Add(Ui.Section("Browser extension"));
        var status = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 10 };
        status.Children.Add(new TextBlock
        {
            Text = error != null ? "● Not listening" : J.B(ext, "connected") ? $"● Connected: {string.Join(", ", browsers)}" : "○ Not detected",
            Foreground = Palette.Status(error != null ? "error" : J.B(ext, "connected") ? "complete" : "queued"),
            VerticalAlignment = VerticalAlignment.Center,
        });
        var show = new Button { Content = "Show Extension Folder" };
        show.Click += (_, _) =>
        {
            var folder = Path.Combine(AppContext.BaseDirectory, "extension", "chromium");
            if (Directory.Exists(folder)) SystemServices.Open(Path.GetDirectoryName(folder)!);
            else store.Report("The extension isn't bundled in this build. Run extension/build.sh in the source folder and load extension/dist/chromium.");
        };
        status.Children.Add(show);
        Add(Ui.Row("Extension", status,
            "Chrome, Edge, Brave, Vivaldi, Opera: open the extensions page, turn on Developer mode, choose “Load unpacked” and select the chromium folder. Firefox: about:debugging → This Firefox → Load Temporary Add-on → firefox/manifest.json."));
        if (error != null)
        {
            var warning = Ui.Note($"{error}. Another app may be using this port; choose a different one under Advanced.");
            warning.Foreground = (Brush)Application.Current.Resources["SystemFillColorCriticalBrush"];
            Add(warning);
        }

        Add(Ui.Section("Capture"));
        Add(Toggle("captureEnabled", "Take over browser downloads"));
        var pause = Ui.Switch();
        pause.Toggled += (_, _) => Put("capturePausedUntil", pause.IsOn ? DateTimeOffset.UtcNow.ToUnixTimeMilliseconds() + 600_000 : null);
        loaders.Add(() => pause.IsOn = draft["capturePausedUntil"] is JsonValue v && v.TryGetValue<long>(out var until)
                                       && until > DateTimeOffset.UtcNow.ToUnixTimeMilliseconds());
        Add(Ui.Row("Pause capture for 10 minutes", pause));
        Add(Choice("bypassKey", "Bypass key (hold while clicking)", [("alt", "Alt"), ("shift", "Shift"), ("ctrl", "Ctrl"), ("none", "None")]));
        Add(Number("captureMinSizeKb", "Skip files smaller than (KB)", 0, 10_000_000));

        Add(Ui.Section("File types to capture"));
        var types = Ui.Mono();
        types.AcceptsReturn = true;
        types.TextWrapping = TextWrapping.Wrap;
        types.MinHeight = 90;
        types.TextChanged += (_, _) => Put("captureExtensions", J.List(Fmt.ParseExtensions(types.Text)));
        loaders.Add(() =>
        {
            var have = J.Strings(draft, "captureExtensions");
            if (!Fmt.ParseExtensions(types.Text).SequenceEqual(have)) types.Text = string.Join(" ", have);
        });
        Add(types);
    }

    private JsonObject? Category(string id) => J.Objects(draft, "categories").FirstOrDefault(c => J.S(c, "id") == id);

    /// <summary>
    /// Change one category, and optionally take its new extensions away from the others
    /// (an extension belongs to one category).
    /// </summary>
    private void EditCategory(string id, Action<JsonObject> change, List<string>? take = null)
    {
        var categories = (JsonArray)J.Arr(draft, "categories").DeepClone();
        foreach (var c in categories.OfType<JsonObject>())
        {
            if (J.S(c, "id") == id) change(c);
            else if (take is { Count: > 0 }) c["extensions"] = J.List(J.Strings(c, "extensions").Where(e => !take.Contains(e)));
        }
        Put("categories", categories);
    }

    private string DefaultFolder(JsonObject c)
    {
        var root = J.S(draft, "defaultFolder");
        return J.S(c, "id") == "general" ? root : Path.Combine(root, J.S(c, "name").Replace('/', '_').Replace('\\', '_'));
    }

    private void FileTypes()
    {
        Add(Ui.Section("Categories"));
        Add(Ui.Note("Each category has its own file types and folder."));
        foreach (var c in J.Objects(draft, "categories").ToList())
        {
            var id = J.S(c, "id");
            var general = id == "general";
            var body = new StackPanel { Spacing = 8 };
            if (general)
            {
                body.Children.Add(Ui.Note("General holds every file type no other category claims, and saves to your download folder."));
            }
            else
            {
                var name = new TextBox { Text = J.S(c, "name"), Header = "Name" };
                name.LostFocus += (_, _) =>
                {
                    if (name.Text.Trim().Length == 0 || name.Text == J.S(Category(id), "name")) return;
                    EditCategory(id, x => x["name"] = name.Text.Trim());
                    Build();
                };
                body.Children.Add(name);
            }

            var own = J.Str(c, "folder") is { Length: > 0 } f ? f : null;
            var folder = new Grid { ColumnSpacing = 8 };
            folder.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            folder.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            folder.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            var path = new TextBlock { Text = own ?? DefaultFolder(c), Style = (Style)Application.Current.Resources[own == null ? "Dim" : "Num"] };
            var choose = new Button { Content = "Choose…" };
            var reset = new Button { Content = "Use Default", Visibility = own == null ? Visibility.Collapsed : Visibility.Visible };
            choose.Click += async (_, _) =>
            {
                if (await Win.PickFolder(this) is not string picked) return;
                EditCategory(id, x => x["folder"] = picked);
                Build();
            };
            reset.Click += (_, _) =>
            {
                EditCategory(id, x => x["folder"] = null);
                Build();
            };
            Grid.SetColumn(choose, 1);
            Grid.SetColumn(reset, 2);
            folder.Children.Add(path);
            folder.Children.Add(choose);
            folder.Children.Add(reset);
            body.Children.Add(new TextBlock { Text = "Folder" });
            body.Children.Add(folder);

            if (!general)
            {
                var types = Ui.Mono("mkv webm");
                types.Header = "File types, separated by spaces";
                types.Text = string.Join(" ", J.Strings(c, "extensions"));
                types.LostFocus += (_, _) =>
                {
                    var list = Fmt.ParseExtensions(types.Text);
                    if (list.SequenceEqual(J.Strings(Category(id), "extensions"))) return;
                    EditCategory(id, x => x["extensions"] = J.List(list), list);
                    Build();
                };
                body.Children.Add(types);
                var remove = new Button { Content = "Remove This Category" };
                remove.Click += (_, _) =>
                {
                    var kept = new JsonArray();
                    foreach (var other in J.Objects(draft, "categories").Where(x => J.S(x, "id") != id)) kept.Add(other.DeepClone());
                    Put("categories", kept);
                    Build();
                };
                body.Children.Add(remove);
            }

            var count = J.Arr(c, "extensions").Count;
            Add(new Expander
            {
                Header = general ? $"{J.S(c, "name")} — everything else" : $"{J.S(c, "name")} — {count} types",
                Content = body,
                HorizontalAlignment = HorizontalAlignment.Stretch,
                HorizontalContentAlignment = HorizontalAlignment.Stretch,
                Margin = new Thickness(0, 0, 0, 4),
            });
        }
        var add = new Button { Content = "Add Category", Margin = new Thickness(0, 8, 0, 0) };
        add.Click += (_, _) =>
        {
            var categories = (JsonArray)J.Arr(draft, "categories").DeepClone();
            var at = categories.Count;
            for (var i = 0; i < categories.Count; i++)
                if (J.S(categories[i], "id") == "general") at = i;
            categories.Insert(at, J.Obj(("id", "c" + Guid.NewGuid().ToString("N")[..6]), ("name", "New Category"), ("icon", "cat-general"),
                ("extensions", new JsonArray()), ("folder", null)));
            Put("categories", categories);
            Build();
        };
        Add(add);
    }

    private void Proxy()
    {
        Add(Ui.Section("Proxy"));
        Add(Choice("proxyMode", "Connection", [("none", "No proxy"), ("system", "Use Windows settings"), ("http", "HTTP proxy"), ("socks5", "SOCKS5 proxy")]));
        var manual = new StackPanel();
        foreach (var row in new[] { Text("proxyHost", "Host"), Number("proxyPort", "Port", 1, 65535), Text("proxyUser", "Username"), Text("proxyPass", "Password", password: true) })
            manual.Children.Add(row);
        loaders.Add(() => manual.Visibility = J.S(draft, "proxyMode") is "http" or "socks5" ? Visibility.Visible : Visibility.Collapsed);
        Add(manual);
    }

    private void Notifications()
    {
        Add(Ui.Section("Notifications"));
        Add(Toggle("notifyComplete", "Download complete"));
        Add(Toggle("notifyFailure", "Download failed"));
        Add(Toggle("notifyQueueFinished", "Queue finished"));
    }

    private void Advanced()
    {
        Add(Ui.Section("Save locations"));
        Add(Folder("defaultFolder", "Download folder",
            "Each category saves to a subfolder of this folder unless you choose another under File Types."));
        Add(Ui.Section("Network"));
        Add(Text("userAgent", "User agent"));
        Add(Number("maxRetries", "Retries per connection", 0, 1000));
        Add(Number("retryDelaySecs", "Retry delay (seconds)", 0, 3600));
        Add(Number("timeoutSecs", "Timeout (seconds)", 1, 3600));
        Add(Number("minSegmentKb", "Minimum segment size (KB)", 1, 1_048_576));
        Add(Number("extensionPort", "Extension port", 1, 65535));
        Add(Ui.Section("Security"));
        Add(Toggle("verifyTls", "Verify TLS certificates"));
        var warning = Ui.Note("Certificate checks are off. Anyone on the network path can alter what you download.");
        warning.Foreground = (Brush)Application.Current.Resources["SystemFillColorCriticalBrush"];
        loaders.Add(() => warning.Visibility = J.B(draft, "verifyTls", true) ? Visibility.Collapsed : Visibility.Visible);
        Add(warning);
    }

    private void Escape_Invoked(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs args)
    {
        args.Handled = true;
        Close();
    }
}
