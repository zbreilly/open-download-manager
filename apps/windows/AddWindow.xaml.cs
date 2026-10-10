using System.Text.Json.Nodes;
using System.Text.RegularExpressions;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Windows.ApplicationModel.DataTransfer;
using Windows.System;

namespace OpenDownloadManager;

/// <summary>
/// The Add Download window (also used for links handed over by the browser extension, the
/// clipboard monitor and odm:// links).
/// </summary>
public sealed partial class AddWindow : Window
{
    private static readonly string[] ConnectionChoices = ["1", "4", "8", "16", "32"];
    private static readonly Regex BatchPattern = new(@"\[[^\]]+-[^\]]+\]");

    private readonly AppStore store = App.Store;
    /// <summary>
    /// Opened from a web page: the page chose the URL, so it is not contacted until the user asks
    /// (Check Link), and Enter doesn't start the download.
    /// </summary>
    private readonly bool fromWeb;

    private readonly TextBox url = Ui.Mono("https://");
    private readonly TextBox batchText = Ui.Mono();
    private readonly TextBlock batchCount = new();
    private readonly TextBlock urlError = Ui.Note("Enter a full http:// or https:// address.");
    private readonly TextBlock infoTitle = new() { TextWrapping = TextWrapping.Wrap };
    private readonly TextBlock infoDetail = Ui.Note("");
    private readonly ProgressRing spinner = new() { Width = 18, Height = 18, IsActive = false };
    private readonly Button check = new() { Content = "Check Link" };
    private readonly TextBox name = new();
    private readonly ComboBox category;
    private readonly ComboBox queue;
    private readonly TextBox dir = Ui.Mono();
    private readonly ComboBox connections = Ui.Combo(ConnectionChoices.Select(c => (c, c)), 100);
    private readonly ToggleSwitch always = Ui.Switch();
    private readonly ToggleSwitch remember = Ui.Switch();
    private readonly ToggleSwitch verify = Ui.Switch();
    private readonly TextBox checksum = Ui.Mono("SHA-256, SHA-1 or MD5");
    private readonly TextBox username = new();
    private readonly PasswordBox password = new();
    private readonly TextBox referrer = new();
    private readonly TextBox cookies = new() { PlaceholderText = "name=value; other=value" };
    private readonly TextBox userAgent = new() { PlaceholderText = "Default" };
    private readonly StackPanel headers = new() { Spacing = 6 };
    private readonly TextBlock error = new() { TextWrapping = TextWrapping.Wrap, IsTextSelectionEnabled = true, Visibility = Visibility.Collapsed };

    private readonly Border urlRow, batchRow, infoRow, nameRow, categoryRow, alwaysRow, rememberRow, verifyRow, checksumRow;
    private readonly TextBlock alwaysLabel = new() { TextWrapping = TextWrapping.Wrap };
    private readonly TextBlock rememberLabel = new() { TextWrapping = TextWrapping.Wrap };

    private List<string> batchUrls = [];
    private JsonObject? probe;
    private string autoName = "", autoDir = "", autoCategory = "general";
    private bool categoryTouched, quiet, busy;
    /// <summary>Bumped to drop the result of an older probe.</summary>
    private int generation;

    public AddWindow(JsonObject p)
    {
        InitializeComponent();
        fromWeb = J.S(p, "source") == "web";
        Win.Setup(this, "Add Download", 580, 760);

        category = Ui.Combo(store.Categories.Select(c => (J.S(c, "id"), J.S(c, "name"))));
        queue = Ui.Combo(new[] { ("", "None (start manually)") }.Concat(store.Queues.Select(q => (J.S(q, "id"), J.S(q, "name")))));
        queue.Select("");
        error.Foreground = (Brush)Application.Current.Resources["SystemFillColorCriticalBrush"];
        urlError.Foreground = error.Foreground;
        batchText.AcceptsReturn = true;
        batchText.TextWrapping = TextWrapping.Wrap;
        batchText.MinHeight = 110;

        // ------------------------------------------------------------ layout
        var urlPanel = new StackPanel { Spacing = 6 };
        urlPanel.Children.Add(url);
        urlPanel.Children.Add(urlError);
        urlRow = Ui.Wide("URL", urlPanel);
        var info = new Grid { ColumnSpacing = 12 };
        info.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        info.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        info.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var infoText = new StackPanel { VerticalAlignment = VerticalAlignment.Center };
        infoText.Children.Add(infoTitle);
        infoText.Children.Add(infoDetail);
        Grid.SetColumn(infoText, 1);
        Grid.SetColumn(check, 2);
        info.Children.Add(spinner);
        info.Children.Add(infoText);
        info.Children.Add(check);
        infoRow = new Border { Style = (Style)Application.Current.Resources["Card"], Child = info };
        var batchPanel = new StackPanel { Spacing = 6 };
        batchPanel.Children.Add(batchText);
        batchPanel.Children.Add(batchCount);
        batchRow = Ui.Wide("Links", batchPanel, "One per line. Patterns like file[001-120].jpg or [a-z] expand.");

        nameRow = Ui.Wide("File name", name);
        categoryRow = Ui.Row("Category", category);
        var browse = new DropDownButton { Content = "Choose…" };
        var dirGrid = new Grid { ColumnSpacing = 8 };
        dirGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        dirGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        Grid.SetColumn(browse, 1);
        dirGrid.Children.Add(dir);
        dirGrid.Children.Add(browse);

        alwaysRow = RowWithLabel(alwaysLabel, always);
        rememberRow = RowWithLabel(rememberLabel, remember);
        verifyRow = Ui.Row("Verify checksum after download", verify);
        checksumRow = Ui.Wide("Checksum", checksum);

        var advanced = new StackPanel { Spacing = 8 };
        foreach (var (label, box) in new (string, Control)[] { ("Username", username), ("Password", password), ("Referrer", referrer), ("Cookies", cookies), ("User agent", userAgent) })
        {
            advanced.Children.Add(new TextBlock { Text = label });
            advanced.Children.Add(box);
        }
        advanced.Children.Add(new TextBlock { Text = "Headers" });
        advanced.Children.Add(headers);
        var addHeader = new Button { Content = "Add Header" };
        addHeader.Click += (_, _) => AddHeader();
        advanced.Children.Add(addHeader);
        var expander = new Expander
        {
            Header = "Authentication, referrer, cookies, headers",
            Content = advanced,
            HorizontalAlignment = HorizontalAlignment.Stretch,
            HorizontalContentAlignment = HorizontalAlignment.Stretch,
            Margin = new Thickness(0, 12, 0, 8),
        };

        foreach (var row in new UIElement[]
                 {
                     urlRow, infoRow, batchRow,
                     Ui.Section("Save"), nameRow, categoryRow, Ui.Row("Queue", queue), Ui.Wide("Save to", dirGrid),
                     Ui.Section("Options"), Ui.Row("Connections", connections), alwaysRow, rememberRow, verifyRow, checksumRow,
                     expander, error,
                 })
            Form.Children.Add(row);

        // ------------------------------------------------------------ values
        connections.Select(NearestConnections(J.N(store.Settings, "defaultConnections")));
        autoDir = FolderFor("general") ?? "";
        dir.Text = autoDir;
        category.Select("general");
        remember.IsOn = true;
        Batch.IsOn = J.B(p, "batch");
        if (!fromWeb)
        {
            // a web page only gets to suggest the URL
            name.Text = J.S(p, "fileName");
            referrer.Text = J.S(p, "referrer");
            cookies.Text = J.S(p, "cookies");
            userAgent.Text = J.S(p, "userAgent");
        }
        var recentMenu = new MenuFlyout();
        try
        {
            foreach (var r in (Engine.Call("recent_folders", J.Obj(("limit", 6))) as JsonArray ?? new JsonArray()).Select(v => v?.ToString()).OfType<string>())
            {
                var item = new MenuFlyoutItem { Text = r };
                item.Click += (_, _) => dir.Text = r;
                recentMenu.Items.Add(item);
            }
        }
        catch { }
        if (recentMenu.Items.Count > 0) recentMenu.Items.Add(new MenuFlyoutSeparator());
        var choose = new MenuFlyoutItem { Text = "Choose…" };
        choose.Click += async (_, _) =>
        {
            if (await Win.PickFolder(this) is string picked) dir.Text = picked;
        };
        recentMenu.Items.Add(choose);
        browse.Flyout = recentMenu;

        // ------------------------------------------------------------ wiring
        url.TextChanged += (_, _) => ScheduleProbe();
        batchText.TextChanged += (_, _) => ScheduleProbe();
        name.TextChanged += (_, _) => Layout();
        dir.TextChanged += (_, _) => Layout();
        category.SelectionChanged += (_, _) =>
        {
            if (!quiet) categoryTouched = true;
            CategoryChanged(category.Value());
        };
        verify.Toggled += (_, _) => Layout();
        check.Click += (_, _) => ProbeNow();
        if (!fromWeb)
        {
            // Enter starts the download, except for a link a web page chose
            foreach (var box in new[] { url, name })
                box.KeyDown += (_, e) =>
                {
                    if (e.Key != VirtualKey.Enter) return;
                    e.Handled = true;
                    Submit(start: true, onExists: null);
                };
        }

        Layout();
        var given = J.S(p, "url");
        if (given.Length > 0) url.Text = given;
        else if (!Batch.IsOn) OfferClipboard();
    }

    private static Border RowWithLabel(TextBlock label, FrameworkElement control)
    {
        var row = Ui.Row("", control);
        // put the changing label where the fixed one would be
        if (row.Child is Grid g && g.Children[0] is StackPanel text)
        {
            text.Children.Clear();
            text.Children.Add(label);
        }
        return row;
    }

    private static string NearestConnections(ulong want) =>
        ConnectionChoices.OrderBy(c => Math.Abs(long.Parse(c) - (long)Math.Clamp(want, 1UL, 32UL))).First();

    private static string? FolderFor(string categoryId)
    {
        try { return Engine.Call("folder_for", J.Obj(("category", categoryId)))?.ToString(); }
        catch { return null; }
    }

    /// <summary>Offer a link that is on the clipboard.</summary>
    private async void OfferClipboard()
    {
        try
        {
            var content = Clipboard.GetContent();
            if (!content.Contains(StandardDataFormats.Text)) return;
            var text = (await content.GetTextAsync()).Trim();
            if (url.Text.Length == 0 && Fmt.IsHttp(text) && !text.Any(char.IsWhiteSpace)) url.Text = text;
        }
        catch { }
    }

    // -------------------------------------------------------------------- state

    private bool IsBatch => Batch.IsOn;
    private string Link => url.Text.Trim();
    private string NameExt => Fmt.Extension(name.Text.Trim());
    private bool DirTouched => dir.Text != autoDir;
    private bool Valid => IsBatch ? batchUrls.Count > 0 : Fmt.IsHttp(Link);
    private string CategoryName => J.Str(store.Categories.FirstOrDefault(c => J.S(c, "id") == category.Value()), "name") ?? "this category";

    private static Visibility Show(bool on) => on ? Visibility.Visible : Visibility.Collapsed;

    /// <summary>Show what applies to the current mode and input.</summary>
    private void Layout()
    {
        var batch = IsBatch;
        urlRow.Visibility = Show(!batch);
        batchRow.Visibility = Show(batch);
        if (batch) infoRow.Visibility = Visibility.Collapsed;
        urlError.Visibility = Show(!batch && Link.Length > 0 && !Fmt.IsHttp(Link));
        nameRow.Visibility = Show(!batch);
        categoryRow.Visibility = Show(!batch);
        alwaysLabel.Text = $"Always put .{NameExt} files in {CategoryName}";
        alwaysRow.Visibility = Show(!batch && NameExt.Length > 0 && category.Value() != autoCategory);
        rememberLabel.Text = $"Remember this folder for {CategoryName}";
        rememberRow.Visibility = Show(!batch);
        verifyRow.Visibility = Show(!batch);
        checksumRow.Visibility = Show(!batch && verify.IsOn);
        connections.IsEnabled = probe == null || J.B(probe, "resumable", true);
        batchCount.Text = $"{batchUrls.Count} files";
        StartButton.Content = batch ? $"Start {batchUrls.Count} Downloads" : "Start Download";
        StartButton.IsEnabled = QueueButton.IsEnabled = Valid && !busy;
    }

    private void CategoryChanged(string id)
    {
        if (!DirTouched && FolderFor(id) is string folder)
        {
            autoDir = folder;
            dir.Text = folder;
        }
        Layout();
    }

    private void AddHeader()
    {
        var grid = new Grid { ColumnSpacing = 6 };
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(2, GridUnitType.Star) });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var key = new TextBox { PlaceholderText = "Header" };
        var value = new TextBox { PlaceholderText = "Value" };
        var remove = new Button { Content = new SymbolIcon(Symbol.Remove) };
        ToolTipService.SetToolTip(remove, "Remove header");
        remove.Click += (_, _) => headers.Children.Remove(grid);
        Grid.SetColumn(value, 1);
        Grid.SetColumn(remove, 2);
        grid.Children.Add(key);
        grid.Children.Add(value);
        grid.Children.Add(remove);
        headers.Children.Add(grid);
    }

    private JsonArray HeaderPairs()
    {
        var list = new JsonArray();
        foreach (var grid in headers.Children.OfType<Grid>())
        {
            var boxes = grid.Children.OfType<TextBox>().ToList();
            if (boxes.Count == 2 && boxes[0].Text.Trim().Length > 0)
                list.Add(new JsonArray(boxes[0].Text.Trim(), boxes[1].Text.Trim()));
        }
        return list;
    }

    private void ShowInfo(string title, string detail, bool spinning)
    {
        infoRow.Visibility = Visibility.Visible;
        infoTitle.Text = title;
        infoDetail.Text = detail;
        infoDetail.Visibility = Show(detail.Length > 0);
        spinner.IsActive = spinning;
        spinner.Visibility = Show(spinning);
        check.Visibility = Show(fromWeb && !spinning);
    }

    // -------------------------------------------------------------------- probe

    /// <summary>The input changed: expand the batch, or probe the link after a pause.</summary>
    private async void ScheduleProbe()
    {
        var mine = ++generation;
        if (IsBatch)
        {
            Layout();
            var source = batchText.Text;
            await Task.Delay(300);
            if (mine != generation) return;
            var found = new List<string>();
            foreach (var line in source.Split([' ', '\t', '\r', '\n'], StringSplitOptions.RemoveEmptyEntries))
            {
                if (BatchPattern.IsMatch(line))
                {
                    try
                    {
                        var expanded = await Engine.CallAsync("expand_batch", J.Obj(("pattern", line))) as JsonArray;
                        found.AddRange((expanded ?? new JsonArray()).Select(v => v?.ToString()).OfType<string>());
                    }
                    catch { }
                }
                else
                {
                    found.Add(line);
                }
            }
            if (mine != generation) return;
            batchUrls = found.Where(Fmt.IsHttp).Take(10_000).ToList();
            Layout();
            return;
        }
        probe = null;
        var link = Link;
        if (!Fmt.IsHttp(link))
        {
            infoRow.Visibility = Visibility.Collapsed;
            Layout();
            return;
        }
        if (fromWeb)
        {
            // probe only on request (Check Link)
            ShowInfo(Fmt.Host(link), "Opened from a web page. The link has not been contacted yet.", false);
            check.Content = "Check Link";
            Layout();
            return;
        }
        ShowInfo("Checking link…", "", true);
        Layout();
        await Task.Delay(300);
        if (mine == generation) await RunProbe(link, mine);
    }

    /// <summary>Probe right away (the Check Link button).</summary>
    private async void ProbeNow()
    {
        var link = Link;
        if (IsBatch || !Fmt.IsHttp(link)) return;
        var mine = ++generation;
        probe = null;
        ShowInfo("Checking link…", "", true);
        await RunProbe(link, mine);
    }

    private async Task RunProbe(string link, int mine)
    {
        var opts = J.Obj(
            ("referrer", J.OrNull(referrer.Text)),
            ("cookies", J.OrNull(cookies.Text)),
            ("username", J.OrNull(username.Text)),
            ("password", J.OrNull(password.Password)),
            ("headers", HeaderPairs()));
        var nameTouched = name.Text != autoName;
        try
        {
            var p = await Engine.CallAsync("probe_url", J.Obj(("url", link), ("opts", opts))) as JsonObject;
            if (mine != generation || p == null) return;
            if (!nameTouched)
            {
                autoName = J.S(p, "fileName");
                name.Text = autoName;
            }
            autoCategory = J.S(p, "category");
            if (!categoryTouched)
            {
                quiet = true;
                category.Select(autoCategory);
                quiet = false;
                CategoryChanged(autoCategory);
            }
            if (!J.B(p, "resumable")) connections.Select("1");
            else if (connections.Value() == "1") connections.Select(NearestConnections(J.N(store.Settings, "defaultConnections")));
            var mime = J.Str(p, "mime") is string m ? $" · {m}" : "";
            ShowInfo($"{Fmt.Bytes(J.Num(p, "totalBytes"))}{mime}",
                J.B(p, "resumable") ? $"Resumable · byte ranges · {J.S(p, "protocol")}" : "Server doesn't support resume. Download will use 1 connection.",
                false);
            probe = p;
        }
        catch (Exception e)
        {
            if (mine != generation) return;
            if (!nameTouched && Fmt.UrlFileName(link) is { Length: > 0 } last)
            {
                autoName = last;
                name.Text = last;
            }
            ShowInfo(e.Message, "You can still add it; it will retry when started.", false);
        }
        check.Content = "Check Again";
        Layout();
    }

    // ------------------------------------------------------------------- submit

    private JsonObject Request(bool start, string link, string? onExists)
    {
        var batch = IsBatch;
        var q = queue.Value();
        JsonNode? FromProbe(string key) => batch ? null : probe?[key]?.DeepClone();
        var sum = checksum.Text.Trim();
        return J.Obj(
            ("url", link),
            ("fileName", batch ? null : J.OrNull(name.Text)),
            ("saveDir", DirTouched ? J.OrNull(dir.Text) : null),
            ("category", batch ? null : category.Value()),
            ("queueId", q.Length > 0 ? q : start ? null : "main"),
            ("start", start),
            ("connections", int.Parse(connections.Value().Length > 0 ? connections.Value() : "16")),
            ("referrer", J.OrNull(referrer.Text)),
            ("cookies", J.OrNull(cookies.Text)),
            ("userAgent", J.OrNull(userAgent.Text)),
            ("headers", HeaderPairs()),
            ("username", J.OrNull(username.Text)),
            ("password", J.OrNull(password.Password)),
            ("checksum", !batch && verify.IsOn && sum.Length > 0 ? J.Obj(("algo", ""), ("expected", sum)) : null),
            ("onExists", onExists ?? "rename"),
            ("rememberFolder", !batch && remember.IsOn && DirTouched),
            ("totalBytes", FromProbe("totalBytes")),
            ("resumable", FromProbe("resumable")),
            ("mime", FromProbe("mime")));
    }

    private void Fail(string message)
    {
        error.Text = message;
        error.Visibility = Visibility.Visible;
        busy = false;
        Layout();
    }

    private async void Submit(bool start, string? onExists)
    {
        if (!Valid || busy) return;
        error.Visibility = Visibility.Collapsed;
        var batch = IsBatch;
        var fileName = name.Text.Trim();
        var cat = category.Value();
        if (!batch && onExists == null && fileName.Length > 0)
        {
            var folder = DirTouched ? dir.Text : FolderFor(cat) ?? dir.Text;
            var exists = false;
            try { exists = Engine.Call("target_exists", J.Obj(("dir", folder), ("name", fileName)))?.GetValue<bool>() == true; } catch { }
            if (exists)
            {
                var choice = await Win.Ask(this, $"“{fileName}” already exists in this folder.",
                    "Rename the new file, resume the existing one, or cancel and pick another name.",
                    "Rename New File", "Resume Existing File");
                if (choice == 0)
                {
                    // Cancel offers the destructive choice separately, so it is never one click away
                    if (await Win.Ask(this, "Overwrite the existing file?", $"“{fileName}” will be replaced.", "Overwrite", primaryIsDefault: false) != 1) return;
                    onExists = "overwrite";
                }
                else
                {
                    onExists = choice == 1 ? "rename" : "resume";
                }
            }
        }
        busy = true;
        Layout();
        var requests = batch
            ? batchUrls.Select(u => Request(start, u, null)).ToList()
            : [Request(start, Link, onExists)];
        var ext = NameExt;
        var assign = !batch && always.IsOn && ext.Length > 0 && cat != autoCategory;
        var showProgress = start && store.Setting("showProgressOnStart", false);
        try
        {
            if (assign) await Engine.CallAsync("assign_extensions", J.Obj(("category", cat), ("extensions", J.List([ext]))));
            if (requests.Count == 1)
            {
                var id = (await Engine.CallAsync("add_download", J.Obj(("req", requests[0]))))?.ToString();
                if (showProgress && id != null)
                {
                    // the list learns about it through its event first
                    await Task.Delay(200);
                    App.OpenProgress(id);
                }
            }
            else
            {
                var list = new JsonArray();
                foreach (var r in requests) list.Add(r);
                await Engine.CallAsync("add_many", J.Obj(("reqs", list)));
            }
            Close();
        }
        catch (Exception e)
        {
            Fail(e.Message);
        }
    }

    private void Batch_Toggled(object sender, RoutedEventArgs e)
    {
        // fires while the window is still being built
        if (urlRow == null) return;
        Layout();
        ScheduleProbe();
    }

    private void Queue_Click(object sender, RoutedEventArgs e) => Submit(start: false, onExists: null);
    private void Start_Click(object sender, RoutedEventArgs e) => Submit(start: true, onExists: null);
    private void Cancel_Click(object sender, RoutedEventArgs e) => Close();

    private void Escape_Invoked(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs args)
    {
        args.Handled = true;
        Close();
    }
}
