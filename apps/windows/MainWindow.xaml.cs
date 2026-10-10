using System.Collections.ObjectModel;
using System.Text.Json.Nodes;
using Microsoft.UI.Input;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Windows.ApplicationModel.DataTransfer;
using Windows.System;

namespace OpenDownloadManager;

/// <summary>The main window: library, categories and queues on the left, the download list, details and a status bar.</summary>
public sealed partial class MainWindow : Window
{
    private static readonly int[] LimitPresetsMB = [1, 2, 5, 10, 25, 50, 100];

    private readonly AppStore store = App.Store;
    private readonly ObservableCollection<DownloadRow> visible = [];
    private readonly Dictionary<string, InfoBadge> badges = [];
    private readonly DispatcherTimer powerTimer = new() { Interval = TimeSpan.FromSeconds(1) };
    private readonly TextBlock[] detailValues;
    private string filter = "library:all";
    private string search = "";
    private string sortKey = "added";
    private bool sortDescending = true;
    private bool rebuilding;
    private DownloadRow? shown;

    public MainWindow()
    {
        InitializeComponent();
        Win.Setup(this, SystemServices.AppName, 1280, 820);
        Downloads.ItemsSource = visible;

        detailValues = new TextBlock[6];
        var labels = new[] { "Downloaded", "Speed", "Time left", "Connections", "Resume", "Save to" };
        for (var i = 0; i < labels.Length; i++)
        {
            DetailGrid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
            var label = new TextBlock { Text = labels[i], Style = (Style)Application.Current.Resources["Dim"] };
            var value = new TextBlock { Style = (Style)Application.Current.Resources["Num"], IsTextSelectionEnabled = true };
            Grid.SetRow(label, i);
            Grid.SetRow(value, i);
            Grid.SetColumn(value, 1);
            DetailGrid.Children.Add(label);
            DetailGrid.Children.Add(value);
            detailValues[i] = value;
        }

        // closing hides the window while a notification-area icon keeps the app reachable
        AppWindow.Closing += (_, e) =>
        {
            e.Cancel = true;
            if (App.HideInsteadOfClosing) AppWindow.Hide();
            else App.Quit();
        };
        powerTimer.Tick += (_, _) => TickPower();
        powerTimer.Start();
        store.Changed += OnChanged;
        store.Error += ShowError;

        BuildSidebar();
        foreach (var c in new[] { Change.List, Change.Settings, Change.Totals, Change.Ext }) OnChanged(c);
        if (store.StartError is string err) ShowError($"The download engine failed to start: {err}");
    }

    private void ShowError(string message)
    {
        ErrorBar.Message = message;
        ErrorBar.IsOpen = true;
    }

    // ------------------------------------------------------------------ sidebar

    private NavigationViewItem Item(string key, string title, string glyph)
    {
        var badge = new InfoBadge { Visibility = Visibility.Collapsed };
        badges[key] = badge;
        return new NavigationViewItem { Content = title, Tag = key, Icon = new FontIcon { Glyph = glyph }, InfoBadge = badge };
    }

    private void BuildSidebar()
    {
        rebuilding = true;
        Nav.MenuItems.Clear();
        badges.Clear();
        Nav.MenuItems.Add(new NavigationViewItemHeader { Content = "Library" });
        Nav.MenuItems.Add(Item("library:all", "All Downloads", "\uE896"));
        Nav.MenuItems.Add(Item("library:active", "Active", "\uE768"));
        Nav.MenuItems.Add(Item("library:unfinished", "Unfinished", "\uE916"));
        Nav.MenuItems.Add(Item("library:finished", "Finished", "\uE73E"));
        Nav.MenuItems.Add(new NavigationViewItemHeader { Content = "Categories" });
        foreach (var c in store.Categories)
            Nav.MenuItems.Add(Item($"category:{J.S(c, "id")}", J.S(c, "name"), Palette.CategoryGlyph(J.S(c, "icon"))));
        Nav.MenuItems.Add(new NavigationViewItemHeader { Content = "Queues" });
        foreach (var q in store.Queues)
        {
            var item = Item($"queue:{J.S(q, "id")}", J.S(q, "name"), J.B(q, "running") ? "\uE768" : J.B(q, "builtin") ? "\uE8FD" : "\uE823");
            var id = J.S(q, "id");
            var menu = new MenuFlyout();
            menu.Items.Add(MenuItem("Start", () => store.Act("start_queue", J.Obj(("id", id))), !J.B(q, "running")));
            menu.Items.Add(MenuItem("Stop", () => store.Act("stop_queue", J.Obj(("id", id))), J.B(q, "running")));
            menu.Items.Add(MenuItem("Edit Schedule…", App.OpenScheduler));
            if (!J.B(q, "builtin"))
            {
                menu.Items.Add(new MenuFlyoutSeparator());
                menu.Items.Add(MenuItem("Delete Queue", () => store.Act("delete_queue", J.Obj(("id", id)))));
            }
            item.ContextFlyout = menu;
            Nav.MenuItems.Add(item);
        }
        var current = Nav.MenuItems.OfType<NavigationViewItem>().FirstOrDefault(i => (string)i.Tag == filter);
        if (current == null)
        {
            // the selected category or queue is gone
            filter = "library:all";
            current = Nav.MenuItems.OfType<NavigationViewItem>().First();
        }
        Nav.SelectedItem = current;
        rebuilding = false;
        UpdateCounts();
    }

    private static MenuFlyoutItem MenuItem(string text, Action run, bool enabled = true)
    {
        var item = new MenuFlyoutItem { Text = text, IsEnabled = enabled };
        item.Click += (_, _) => run();
        return item;
    }

    private void UpdateCounts()
    {
        var counts = new Dictionary<string, int>();
        void Count(string key) => counts[key] = counts.GetValueOrDefault(key) + 1;
        foreach (var r in store.Rows)
        {
            Count("library:all");
            if (r.IsActive) Count("library:active");
            Count(r.IsComplete ? "library:finished" : "library:unfinished");
            Count($"category:{r.Category}");
            if (r.QueueId is string q) Count($"queue:{q}");
        }
        foreach (var (key, badge) in badges)
        {
            var n = counts.GetValueOrDefault(key);
            badge.Value = n;
            badge.Visibility = n > 0 ? Visibility.Visible : Visibility.Collapsed;
        }
    }

    private void Nav_SelectionChanged(NavigationView sender, NavigationViewSelectionChangedEventArgs args)
    {
        if (rebuilding) return;
        if (args.IsSettingsSelected)
        {
            App.OpenSettings();
            // settings is a window of its own: stay on the list that was showing
            rebuilding = true;
            Nav.SelectedItem = Nav.MenuItems.OfType<NavigationViewItem>().FirstOrDefault(i => (string)i.Tag == filter);
            rebuilding = false;
            return;
        }
        if (args.SelectedItem is NavigationViewItem { Tag: string key })
        {
            filter = key;
            Refill();
        }
    }

    private string CurrentQueue => filter.StartsWith("queue:") ? filter["queue:".Length..] : "main";

    // --------------------------------------------------------------------- list

    private bool Matches(DownloadRow r)
    {
        var parts = filter.Split(':', 2);
        var ok = (parts[0], parts.Length > 1 ? parts[1] : "") switch
        {
            ("library", "active") => r.IsActive,
            ("library", "unfinished") => !r.IsComplete,
            ("library", "finished") => r.IsComplete,
            ("category", var c) => r.Category == c,
            ("queue", var q) => r.QueueId == q,
            _ => true,
        };
        if (!ok || search.Length == 0) return ok;
        return r.Name.Contains(search, StringComparison.OrdinalIgnoreCase)
            || r.Url.Contains(search, StringComparison.OrdinalIgnoreCase)
            || r.SaveDir.Contains(search, StringComparison.OrdinalIgnoreCase);
    }

    private IEnumerable<DownloadRow> Sorted(IEnumerable<DownloadRow> rows)
    {
        Func<DownloadRow, IComparable> key = sortKey switch
        {
            "name" => r => r.Name.ToLowerInvariant(),
            "size" => r => r.Total ?? 0,
            "progress" => r => r.Percent,
            "speed" => r => r.SpeedSort,
            "eta" => r => r.EtaSort,
            "status" => r => r.Status,
            "queue" => r => r.QueueId ?? "",
            _ => r => r.AddedAt,
        };
        return sortDescending ? rows.OrderByDescending(key) : rows.OrderBy(key);
    }

    /// <summary>Bring the list in line with the filter, search and sort, keeping the selection.</summary>
    private void Refill()
    {
        var wanted = Sorted(store.Rows.Where(Matches)).ToList();
        if (!wanted.SequenceEqual(visible))
        {
            var selected = Downloads.SelectedItems.OfType<DownloadRow>().ToHashSet();
            rebuilding = true;
            visible.Clear();
            foreach (var r in wanted) visible.Add(r);
            foreach (var r in wanted.Where(selected.Contains)) Downloads.SelectedItems.Add(r);
            rebuilding = false;
        }
        Empty.Visibility = wanted.Count == 0 ? Visibility.Visible : Visibility.Collapsed;
        EmptyText.Text = store.Rows.Count == 0 ? "No downloads yet" : search.Length == 0 ? "No downloads here" : "No downloads match your search";
        EmptyHint.Visibility = store.Rows.Count == 0 ? Visibility.Visible : Visibility.Collapsed;
        UpdateButtons();
        UpdateDetails();
    }

    private void Sort_Click(object sender, RoutedEventArgs e)
    {
        if (sender is not HyperlinkButton { Tag: string key }) return;
        sortDescending = key == sortKey && !sortDescending;
        sortKey = key;
        foreach (var b in Header.Children.OfType<HyperlinkButton>())
        {
            var title = ((string)b.Content).TrimEnd(' ', '↓', '↑');
            b.Content = (string)b.Tag == sortKey ? $"{title} {(sortDescending ? "↓" : "↑")}" : title;
        }
        Refill();
    }

    private void Search_TextChanged(AutoSuggestBox sender, AutoSuggestBoxTextChangedEventArgs args)
    {
        search = sender.Text.Trim();
        Refill();
    }

    private void FocusSearch(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs args)
    {
        Search.Focus(FocusState.Keyboard);
        args.Handled = true;
    }

    private List<DownloadRow> Selected => Downloads.SelectedItems.OfType<DownloadRow>().ToList();
    private List<string> SelectedIds => Selected.Select(r => r.Id).ToList();

    private void List_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (rebuilding) return;
        UpdateButtons();
        UpdateDetails();
    }

    private void List_DoubleTapped(object sender, DoubleTappedRoutedEventArgs e)
    {
        if ((e.OriginalSource as FrameworkElement)?.DataContext is not DownloadRow r) return;
        if (r.IsComplete) store.OpenFile(r); else App.OpenProgress(r.Id);
    }

    private void List_KeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key == VirtualKey.Delete)
        {
            AskDelete();
            e.Handled = true;
        }
        else if (e.Key == VirtualKey.Enter && Selected.FirstOrDefault() is DownloadRow r)
        {
            if (r.IsComplete) store.OpenFile(r); else App.OpenProgress(r.Id);
            e.Handled = true;
        }
    }

    /// <summary>Right-click: select the row under the pointer (unless it is part of the selection) and show its menu.</summary>
    private void List_RightTapped(object sender, RightTappedRoutedEventArgs e)
    {
        if ((e.OriginalSource as FrameworkElement)?.DataContext is not DownloadRow r) return;
        if (!Downloads.SelectedItems.Contains(r)) Downloads.SelectedItem = r;
        RowMenu().ShowAt(Downloads, e.GetPosition(Downloads));
        e.Handled = true;
    }

    private MenuFlyout RowMenu()
    {
        var rows = Selected;
        var ids = SelectedIds;
        var first = rows[0];
        var one = rows.Count == 1;
        var menu = new MenuFlyout();
        menu.Items.Add(MenuItem("Open", () => store.OpenFile(first), one && first.IsComplete));
        menu.Items.Add(MenuItem("Show in Folder", () => store.Reveal(first), one));
        menu.Items.Add(MenuItem("Show Progress Window", () => App.OpenProgress(first.Id), one && !first.IsComplete));
        menu.Items.Add(new MenuFlyoutSeparator());
        menu.Items.Add(MenuItem("Resume", () => store.Resume(ids), rows.Any(r => r.Status is "paused" or "error" or "queued")));
        menu.Items.Add(MenuItem("Pause", () => store.Pause(ids), rows.Any(r => r.IsActive || r.Status == "queued")));
        menu.Items.Add(MenuItem("Redownload", () => store.Act("redownload", J.Obj(("id", first.Id))), one));
        menu.Items.Add(MenuItem("Refresh Link…", () => RefreshLink(first), one && !first.IsComplete));
        menu.Items.Add(MenuItem("Copy Link", () => CopyLink(first), one));
        menu.Items.Add(new MenuFlyoutSeparator());
        var queues = new MenuFlyoutSubItem { Text = "Move to Queue" };
        foreach (var q in store.Queues)
        {
            var id = J.S(q, "id");
            queues.Items.Add(MenuItem(J.S(q, "name"), () => store.Act("move_to_queue", J.Obj(("ids", J.List(ids)), ("queueId", id)))));
        }
        queues.Items.Add(new MenuFlyoutSeparator());
        queues.Items.Add(MenuItem("Remove from Queue", () => store.Act("move_to_queue", J.Obj(("ids", J.List(ids)), ("queueId", null)))));
        menu.Items.Add(queues);
        var categories = new MenuFlyoutSubItem { Text = "Category" };
        var always = new MenuFlyoutSubItem { Text = $"Always Put .{first.FileExtension} Files In" };
        foreach (var c in store.Categories)
        {
            var id = J.S(c, "id");
            categories.Items.Add(MenuItem(J.S(c, "name"), () => store.Act("set_category", J.Obj(("ids", J.List(ids)), ("category", id)))));
            always.Items.Add(MenuItem(J.S(c, "name"), () => store.AssignExtension(first.FileExtension, id)));
        }
        menu.Items.Add(categories);
        if (one && first.FileExtension.Length > 0) menu.Items.Add(always);
        menu.Items.Add(new MenuFlyoutSeparator());
        menu.Items.Add(MenuItem(one ? "Delete…" : $"Delete {rows.Count} Downloads…", AskDelete));
        return menu;
    }

    // ------------------------------------------------------------------ actions

    private void UpdateButtons()
    {
        var rows = Selected;
        ResumeButton.IsEnabled = rows.Any(r => r.Status is "paused" or "error" or "queued");
        PauseButton.IsEnabled = rows.Any(r => r.IsActive || r.Status == "queued");
        DeleteButton.IsEnabled = rows.Count > 0;
    }

    private void Add_Click(object sender, RoutedEventArgs e) => App.OpenAdd();
    private void AddBatch_Click(object sender, RoutedEventArgs e) => App.OpenAdd(J.Obj(("batch", true)));
    private void Resume_Click(object sender, RoutedEventArgs e) => store.Resume(SelectedIds);
    private void Pause_Click(object sender, RoutedEventArgs e) => store.Pause(SelectedIds);
    private void PauseAll_Click(object sender, RoutedEventArgs e) => store.Act("pause_all");
    private void Delete_Click(object sender, RoutedEventArgs e) => AskDelete();
    private void DeleteCompleted_Click(object sender, RoutedEventArgs e) => store.Act("delete_completed");
    private void StartQueue_Click(object sender, RoutedEventArgs e) => store.Act("start_queue", J.Obj(("id", CurrentQueue)));
    private void StopQueue_Click(object sender, RoutedEventArgs e) => store.Act("stop_queue", J.Obj(("id", CurrentQueue)));
    private void Scheduler_Click(object sender, RoutedEventArgs e) => App.OpenScheduler();
    private void Grabber_Click(object sender, RoutedEventArgs e) => App.OpenGrabber();
    private void Settings_Click(object sender, RoutedEventArgs e) => App.OpenSettings();
    private void Quit_Click(object sender, RoutedEventArgs e) => App.Quit();
    private void CancelPower_Click(object sender, RoutedEventArgs e) => store.CancelPower();

    private void Details_Click(object sender, RoutedEventArgs e) => UpdateDetails();

    private async void About_Click(object sender, RoutedEventArgs e)
    {
        var version = typeof(App).Assembly.GetName().Version?.ToString(3) ?? "";
        await Win.Message(this, SystemServices.AppName,
            $"Version {version}\nFast, resumable, segmented downloads.\n\nFree software under the GNU Affero General Public License v3.0.");
    }

    private async void AskDelete()
    {
        var ids = SelectedIds;
        if (ids.Count == 0) return;
        if (!store.Setting("confirmDelete", true))
        {
            store.Delete(ids, false);
            return;
        }
        var choice = await Win.Ask(this,
            ids.Count == 1 ? "Delete this download?" : $"Delete {ids.Count} downloads?",
            "Unfinished parts are always removed. Finished files stay on disk unless you also delete files.",
            "Remove from List", "Remove and Delete Files");
        if (choice == 1) store.Delete(ids, false);
        else if (choice == 2) store.Delete(ids, true);
    }

    private async void RefreshLink(DownloadRow r)
    {
        var url = r.Url;
        var note = "Paste a fresh address for the same file. Progress is kept when the server reports the same size.";
        // ask again with the error until it works or the user gives up
        while (true)
        {
            var fresh = await Win.AskText(this, "Refresh Download Link", note, url, "Update Link");
            if (fresh == null) return;
            try
            {
                await Engine.CallAsync("refresh_link", J.Obj(("id", r.Id), ("newUrl", fresh)));
                return;
            }
            catch (Exception e)
            {
                note = e.Message;
                url = fresh;
            }
        }
    }

    private static void CopyLink(DownloadRow r)
    {
        var data = new DataPackage();
        data.SetText(r.Url);
        Clipboard.SetContent(data);
    }

    private void LimitMenu_Opening(object? sender, object e)
    {
        LimitMenu.Items.Clear();
        foreach (var mb in LimitPresetsMB)
            LimitMenu.Items.Add(MenuItem($"{mb} MB/s", () => store.SetGlobalLimit((ulong)mb * 1024 * 1024)));
        LimitMenu.Items.Add(new MenuFlyoutSeparator());
        LimitMenu.Items.Add(MenuItem("Custom…", CustomLimit));
        if (J.B(store.Settings, "globalLimitEnabled"))
            LimitMenu.Items.Add(MenuItem("Turn Off Limit", () => store.SetGlobalLimit(null)));
        else
            LimitMenu.Items.Add(MenuItem($"Turn On ({Fmt.Mbps(J.N(store.Settings, "globalLimitBps"))} MB/s)", store.ToggleLimit));
    }

    private async void CustomLimit()
    {
        var text = Fmt.Mbps(J.N(store.Settings, "globalLimitBps"));
        var note = "Caps the combined speed of all downloads, in MB/s.";
        while (true)
        {
            var typed = await Win.AskText(this, "Global Speed Limit", note, text, "Turn On Limit");
            if (typed == null) return;
            // reject NaN / inf / 0 instead of silently doing nothing
            if (Fmt.LimitBps(typed) is ulong bps)
            {
                store.SetGlobalLimit(bps);
                return;
            }
            note = Fmt.LimitError;
            text = typed;
        }
    }

    private async void Import_Click(object sender, RoutedEventArgs e)
    {
        if (await Win.PickTextFile(this) is not string path) return;
        try
        {
            var urls = await Engine.CallAsync("read_url_list", J.Obj(("path", path))) as JsonArray;
            var links = new JsonArray();
            foreach (var u in urls ?? new JsonArray()) links.Add(J.Obj(("url", u?.DeepClone())));
            if (links.Count == 0) store.Report("No http(s) links found in that file.");
            else App.OpenGrabber(J.Obj(("links", links)));
        }
        catch (Exception ex)
        {
            store.Report(ex.Message);
        }
    }

    private async void Export_Click(object sender, RoutedEventArgs e)
    {
        if (await Win.PickSaveFile(this, "downloads") is string path)
            store.Act("export_list", J.Obj(("path", path), ("onlyUnfinished", false)));
    }

    // links dragged from a browser
    private void Root_DragOver(object sender, DragEventArgs e)
    {
        if (e.DataView.Contains(StandardDataFormats.WebLink) || e.DataView.Contains(StandardDataFormats.Text))
            e.AcceptedOperation = DataPackageOperation.Copy;
    }

    private async void Root_Drop(object sender, DragEventArgs e)
    {
        var deferral = e.GetDeferral();
        try
        {
            var links = new List<string>();
            if (e.DataView.Contains(StandardDataFormats.WebLink)) links.Add((await e.DataView.GetWebLinkAsync()).AbsoluteUri);
            else if (e.DataView.Contains(StandardDataFormats.Text))
                links.AddRange((await e.DataView.GetTextAsync()).Split('\n').Select(l => l.Trim()).Where(Fmt.IsHttp));
            if (links.Count == 1) App.OpenAdd(J.Obj(("url", links[0])));
            else if (links.Count > 1)
            {
                var list = new JsonArray();
                foreach (var l in links) list.Add(J.Obj(("url", l)));
                App.OpenGrabber(J.Obj(("links", list)));
            }
        }
        catch
        {
            // not something we can read as a link
        }
        finally
        {
            deferral.Complete();
        }
    }

    // ------------------------------------------------------------------ updates

    private void OnChanged(Change c)
    {
        switch (c)
        {
            case Change.List:
                Refill();
                UpdateCounts();
                break;
            case Change.Progress:
                // only a sort by a column that moves with progress needs a new order
                if (sortKey is "progress" or "speed" or "eta" or "size") Refill(); else UpdateDetails();
                break;
            case Change.Queues:
                BuildSidebar();
                UpdateDetails();
                break;
            case Change.Settings:
                BuildSidebar();
                var on = J.B(store.Settings, "globalLimitEnabled");
                LimitButton.Label = on ? $"{Fmt.Mbps(J.N(store.Settings, "globalLimitBps"))} MB/s" : "No Limit";
                break;
            case Change.Totals:
                var t = store.Totals;
                var active = J.N(t, "active");
                StatusSummary.Text = Fmt.TotalsSummary(t);
                StatusSpeed.Text = $"↓ {Fmt.Speed(J.N(t, "speedBps"))}";
                StatusLimit.Text = J.B(t, "limitOn") ? $"Limit {Fmt.Mbps(J.N(t, "limitBps"))} MB/s" : "No speed limit";
                StatusFree.Text = J.Num(t, "freeBytes") is ulong free ? $"{Fmt.Bytes(free)} free" : "";
                Title = active > 0 ? $"{SystemServices.AppName} — {active} active · ↓ {Fmt.Speed(J.N(t, "speedBps"))}" : SystemServices.AppName;
                UpdateDetails();
                break;
            case Change.Ext:
                var ext = store.Ext;
                var error = J.Str(ext, "bridgeError");
                var browsers = J.Strings(ext, "browsers");
                ExtDot.Fill = Palette.Status(error != null ? "error" : J.B(ext, "connected") ? "complete" : "queued");
                StatusExt.Text = error != null ? "Browser bridge offline: port in use"
                    : J.B(ext, "connected") ? $"Browser extension connected{(browsers.Count > 0 ? $" ({string.Join(", ", browsers)})" : "")}"
                    : "Browser extension not detected";
                ToolTipService.SetToolTip(StatusExt, error);
                break;
            case Change.Power:
                TickPower();
                break;
        }
    }

    private void TickPower()
    {
        if (store.Power is not PowerCountdown p)
        {
            PowerBar.IsOpen = false;
            return;
        }
        var left = (int)Math.Ceiling((p.Deadline - DateTime.Now).TotalSeconds);
        if (left <= 0)
        {
            store.RunPower();
            return;
        }
        var verb = p.Action == "sleep" ? "Sleeping" : p.Action == "shutdown" ? "Shutting down" : "Quitting";
        PowerBar.Message = $"“{p.Queue}” finished. {verb} in {left}s.";
        PowerBar.IsOpen = true;
    }

    // ------------------------------------------------------------------ details

    private void UpdateDetails()
    {
        // the first selected download, else the first in the list
        shown = Selected.FirstOrDefault() ?? visible.FirstOrDefault();
        if (DetailsToggle.IsChecked != true || shown is not DownloadRow r)
        {
            Details.Visibility = Visibility.Collapsed;
            return;
        }
        Details.Visibility = Visibility.Visible;
        var d = r.Data;
        DetailIcon.Glyph = r.Glyph;
        DetailName.Text = r.Name;
        DetailUrl.Text = r.Url;
        DetailStatus.Text = r.StatusText;
        DetailDot.Fill = r.StatusBrush;
        var error = r.Status == "error" ? J.Str(d, "error") : null;
        DetailError.Text = error ?? "";
        DetailError.Visibility = error != null ? Visibility.Visible : Visibility.Collapsed;
        DetailPrimary.Content = r.Status switch
        {
            "complete" => "Open",
            "error" => "Refresh Link…",
            "paused" => "Resume",
            _ => "Pause",
        };
        DetailRetry.Visibility = r.Status == "error" ? Visibility.Visible : Visibility.Collapsed;
        var values = new[]
        {
            $"{Fmt.Bytes(r.Received)} of {Fmt.Bytes(r.Total)}",
            r.SpeedText,
            r.EtaText,
            $"{J.N(d, "activeConnections")} active / {J.N(d, "maxConnections")} max",
            J.B(d, "resumable") ? "Supported (byte ranges)" : "Not supported by server",
            r.SaveDir,
        };
        for (var i = 0; i < values.Length; i++) detailValues[i].Text = values[i];
        DetailPercent.Text = r.PercentText;
        DetailSegments.Set(d);
        DetailSpark.Set(d);
        DetailPeak.Text = $"peak {Fmt.Speed(DetailSpark.Peak)}";
    }

    private void DetailPrimary_Click(object sender, RoutedEventArgs e)
    {
        if (shown is not DownloadRow r) return;
        switch (r.Status)
        {
            case "complete": store.OpenFile(r); break;
            case "error": RefreshLink(r); break;
            case "paused": store.Resume([r.Id]); break;
            default: store.Pause([r.Id]); break;
        }
    }

    private void DetailRetry_Click(object sender, RoutedEventArgs e)
    {
        if (shown is DownloadRow r) store.Resume([r.Id]);
    }

    private void DetailReveal_Click(object sender, RoutedEventArgs e)
    {
        if (shown is DownloadRow r) store.Reveal(r);
    }

    private void DetailCopy_Click(object sender, RoutedEventArgs e)
    {
        if (shown is DownloadRow r) CopyLink(r);
    }
}
