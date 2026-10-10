using System.Text.Json.Nodes;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;

namespace OpenDownloadManager;

/// <summary>Queues &amp; Scheduler: one queue at a time, with its schedule, behavior and run order.</summary>
public sealed partial class SchedulerWindow : Window
{
    private static readonly string[] Days = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

    private readonly AppStore store = App.Store;
    private readonly TextBlock title = new() { Style = (Style)Application.Current.Resources["SubtitleTextBlockStyle"], VerticalAlignment = VerticalAlignment.Center };
    private readonly TextBlock running = new() { Text = "● Running", VerticalAlignment = VerticalAlignment.Center, Foreground = Palette.Status("complete") };
    private readonly Button startNow = new() { Content = "Start Now" };
    private readonly Button stopNow = new() { Content = "Stop" };
    private readonly TextBox name = new();
    private readonly Border nameRow;
    private readonly ToggleSwitch startOn = Ui.Switch(), stopOn = Ui.Switch();
    private readonly TimePicker startTime = new() { ClockIdentifier = "24HourClock" };
    private readonly TimePicker stopTime = new() { ClockIdentifier = "24HourClock" };
    private readonly ToggleButton[] days;
    private readonly TextBlock nextRun = new() { VerticalAlignment = VerticalAlignment.Center };
    private readonly Border nextRow;
    private readonly NumberBox concurrent = Ui.Number(1, 32);
    private readonly ComboBox retry = Ui.Combo([("0", "Never"), ("1", "Once"), ("3", "3 times"), ("5", "5 times"), ("10", "10 times")]);
    private readonly ComboBox onFinish = Ui.Combo(
        [("none", "Do nothing"), ("sleep", "Put the PC to sleep"), ("shutdown", "Shut down the PC"), ("exit", "Quit Open Download Manager")], 240);
    private readonly TextBlock countdownNote = Ui.Note("A 60-second countdown with a Cancel button appears first.");
    private readonly StackPanel order = new() { Spacing = 4 };
    private readonly TextBlock orderNote = Ui.Note("");
    private readonly Button delete = new() { Content = "Delete Queue", Margin = new Thickness(0, 16, 0, 0) };

    private string selected = "main";
    private JsonObject? draft;
    private bool dirty, saving, quiet, failed;
    /// <summary>The user reordered the list: only then is the order sent.</summary>
    private bool reordered;

    public SchedulerWindow()
    {
        InitializeComponent();
        Win.Setup(this, "Queues & Scheduler", 880, 700);

        var head = new Grid { ColumnSpacing = 10, Margin = new Thickness(0, 0, 0, 8) };
        head.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        head.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        head.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        head.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        Grid.SetColumn(running, 1);
        Grid.SetColumn(startNow, 2);
        Grid.SetColumn(stopNow, 3);
        foreach (var e in new FrameworkElement[] { title, running, startNow, stopNow }) head.Children.Add(e);
        nameRow = Ui.Wide("Name", name);

        days = Days.Select(d => new ToggleButton { Content = d, MinWidth = 46 }).ToArray();
        var dayPanel = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 4 };
        foreach (var d in days) dayPanel.Children.Add(d);
        nextRow = Ui.Row("Next run", nextRun);

        foreach (var row in new UIElement[]
                 {
                     head, nameRow,
                     Ui.Section("Schedule"),
                     Ui.Row("Start at", Pair(startTime, startOn)),
                     Ui.Row("Stop at", Pair(stopTime, stopOn)),
                     Ui.Wide("Repeat on", dayPanel),
                     nextRow,
                     Ui.Section("Behavior"),
                     Ui.Row("Files at the same time", concurrent),
                     Ui.Row("Retry failed downloads", retry),
                     Ui.Row("When the queue finishes", onFinish),
                     countdownNote,
                     Ui.Section("Order"), orderNote, order,
                     delete,
                 })
            Form.Children.Add(row);

        name.TextChanged += (_, _) => Edited();
        startOn.Toggled += (_, _) => Edited();
        stopOn.Toggled += (_, _) => Edited();
        startTime.TimeChanged += (_, _) => Edited();
        stopTime.TimeChanged += (_, _) => Edited();
        foreach (var d in days) d.Click += (_, _) => Edited();
        concurrent.ValueChanged += (_, _) => Edited();
        retry.SelectionChanged += (_, _) => Edited();
        onFinish.SelectionChanged += (_, _) => Edited();
        startNow.Click += (_, _) => store.Act("start_queue", J.Obj(("id", selected)));
        stopNow.Click += (_, _) => store.Act("stop_queue", J.Obj(("id", selected)));
        delete.Click += (_, _) =>
        {
            store.Act("delete_queue", J.Obj(("id", selected)));
            Pick("main");
        };

        store.Changed += OnChanged;
        Closed += (_, _) => store.Changed -= OnChanged;
        Pick("main");
    }

    private static StackPanel Pair(FrameworkElement a, FrameworkElement b)
    {
        var p = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 12 };
        p.Children.Add(a);
        p.Children.Add(b);
        return p;
    }

    private void OnChanged(Change c)
    {
        if (c == Change.Queues) QueuesChanged();
        else if (c is Change.List or Change.Progress) FillOrder();
    }

    private static Visibility Show(bool on) => on ? Visibility.Visible : Visibility.Collapsed;

    private void FillList()
    {
        quiet = true;
        QueueList.Items.Clear();
        foreach (var q in store.Queues)
        {
            var line = new Grid { ColumnSpacing = 10 };
            line.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            line.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            line.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            var icon = new FontIcon { Glyph = J.B(q, "running") ? "\uE768" : J.B(q, "builtin") ? "\uE8FD" : "\uE823", FontSize = 14 };
            var label = new TextBlock { Text = J.S(q, "name"), TextTrimming = TextTrimming.CharacterEllipsis };
            var count = new TextBlock { Text = J.Arr(q, "order").Count.ToString(), Style = (Style)Application.Current.Resources["Dim"] };
            Grid.SetColumn(label, 1);
            Grid.SetColumn(count, 2);
            line.Children.Add(icon);
            line.Children.Add(label);
            line.Children.Add(count);
            var item = new ListViewItem { Content = line, Tag = J.S(q, "id"), HorizontalContentAlignment = HorizontalAlignment.Stretch };
            QueueList.Items.Add(item);
            if (J.S(q, "id") == selected) QueueList.SelectedItem = item;
        }
        quiet = false;
    }

    private void QueueList_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (quiet) return;
        if (QueueList.SelectedItem is ListViewItem { Tag: string id } && id != selected) Pick(id);
    }

    /// <summary>Load a queue into the editor, dropping unsaved edits.</summary>
    private void Pick(string id)
    {
        selected = id;
        draft = store.Queue(id)?.DeepClone() as JsonObject;
        dirty = reordered = failed = false;
        FillList();
        ShowDraft();
    }

    private static TimeSpan Time(string hhmm)
    {
        var p = hhmm.Split(':');
        return p.Length == 2 && int.TryParse(p[0], out var h) && int.TryParse(p[1], out var m) ? new TimeSpan(h, m, 0) : TimeSpan.Zero;
    }

    private void ShowDraft()
    {
        Editor.Visibility = Show(draft != null);
        NoQueue.Visibility = Show(draft == null);
        if (draft is not JsonObject q)
        {
            RefreshStatus();
            return;
        }
        quiet = true;
        var builtin = J.B(q, "builtin");
        title.Text = J.S(q, "name");
        nameRow.Visibility = Show(!builtin);
        name.Text = J.S(q, "name");
        startOn.IsOn = J.B(q, "startEnabled");
        stopOn.IsOn = J.B(q, "stopEnabled");
        startTime.Time = Time(J.S(q, "startTime"));
        stopTime.Time = Time(J.S(q, "stopTime"));
        var on = J.Arr(q, "days").Select(d => d is JsonValue v && v.TryGetValue<bool>(out var b) ? b : true).ToList();
        for (var i = 0; i < days.Length; i++) days[i].IsChecked = i >= on.Count || on[i];
        concurrent.Value = Math.Clamp(J.N(q, "maxConcurrent"), 1UL, 32UL);
        retry.Select(J.N(q, "retryFailed").ToString());
        onFinish.Select(J.S(q, "onFinish"));
        delete.Visibility = Show(!builtin);
        quiet = false;
        RefreshStatus();
        FillOrder();
    }

    /// <summary>Running state, next run, summary and button states.</summary>
    private void RefreshStatus()
    {
        RevertButton.IsEnabled = SaveButton.IsEnabled = dirty && !saving;
        if (draft is not JsonObject q)
        {
            Summary.Text = "";
            return;
        }
        var isRunning = J.B(q, "running");
        running.Visibility = Show(isRunning);
        startNow.IsEnabled = !isRunning;
        stopNow.IsEnabled = isRunning;
        startTime.IsEnabled = startOn.IsOn;
        stopTime.IsEnabled = stopOn.IsOn;
        var next = Fmt.NextRun(q);
        nextRow.Visibility = Show(next != null);
        nextRun.Text = next ?? "";
        countdownNote.Visibility = Show(J.S(q, "onFinish") is "sleep" or "shutdown");
        if (failed) return;
        var pending = Order().Select(store.Row).OfType<DownloadRow>().Where(r => !r.IsComplete).ToList();
        ulong left = 0;
        foreach (var r in pending) left += (r.Total ?? 0) > r.Received ? (r.Total ?? 0) - r.Received : 0;
        Summary.Foreground = (Brush)Application.Current.Resources["TextFillColorSecondaryBrush"];
        Summary.Text = $"Next run: {next ?? "not scheduled"} · {pending.Count} files · {Fmt.Bytes(left)}";
    }

    /// <summary>A control changed: copy the controls into the draft.</summary>
    private void Edited()
    {
        if (quiet || draft is not JsonObject q) return;
        if (!J.B(q, "builtin")) q["name"] = name.Text;
        q["startEnabled"] = startOn.IsOn;
        q["startTime"] = startTime.Time.ToString(@"hh\:mm");
        q["stopEnabled"] = stopOn.IsOn;
        q["stopTime"] = stopTime.Time.ToString(@"hh\:mm");
        var list = new JsonArray();
        foreach (var d in days) list.Add(d.IsChecked == true);
        q["days"] = list;
        q["maxConcurrent"] = double.IsNaN(concurrent.Value) ? 1 : (int)Math.Clamp(concurrent.Value, 1, 32);
        q["retryFailed"] = int.TryParse(retry.Value(), out var n) ? n : 0;
        q["onFinish"] = onFinish.Value();
        dirty = true;
        failed = false;
        RefreshStatus();
    }

    private List<string> Order() => J.Strings(draft, "order");

    private void FillOrder()
    {
        order.Children.Clear();
        var ids = Order();
        orderNote.Text = ids.Count == 0
            ? "No files in this queue. Use “Move to Queue” on a download, or pick this queue when adding."
            : "Use the arrows to reorder.";
        for (var i = 0; i < ids.Count; i++)
        {
            var index = i;
            var line = new Grid { ColumnSpacing = 8 };
            line.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            line.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            line.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            line.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            var r = store.Row(ids[i]);
            var label = new TextBlock { Text = r?.Name ?? ids[i], TextTrimming = TextTrimming.CharacterEllipsis, VerticalAlignment = VerticalAlignment.Center };
            var info = new TextBlock { Text = r == null ? "" : $"{r.StatusText} · {r.SizeText}", Style = (Style)Application.Current.Resources["Dim"] };
            var up = new Button { Content = new FontIcon { Glyph = "\uE70E", FontSize = 12 }, IsEnabled = i > 0 };
            var down = new Button { Content = new FontIcon { Glyph = "\uE70D", FontSize = 12 }, IsEnabled = i < ids.Count - 1 };
            ToolTipService.SetToolTip(up, "Move up");
            ToolTipService.SetToolTip(down, "Move down");
            up.Click += (_, _) => Move(index, index - 1);
            down.Click += (_, _) => Move(index, index + 1);
            Grid.SetColumn(info, 1);
            Grid.SetColumn(up, 2);
            Grid.SetColumn(down, 3);
            foreach (var e in new FrameworkElement[] { label, info, up, down }) line.Children.Add(e);
            order.Children.Add(new Border { Style = (Style)Application.Current.Resources["Card"], Padding = new Thickness(12, 4, 6, 4), Child = line });
        }
    }

    private void Move(int from, int to)
    {
        var ids = Order();
        if (draft == null || from < 0 || to < 0 || from >= ids.Count || to >= ids.Count) return;
        var item = ids[from];
        ids.RemoveAt(from);
        ids.Insert(to, item);
        draft["order"] = J.List(ids);
        reordered = dirty = true;
        FillOrder();
        RefreshStatus();
    }

    /// <summary>The engine reported the queues: follow what the user didn't edit.</summary>
    private void QueuesChanged()
    {
        FillList();
        var live = store.Queue(selected);
        if (draft == null)
        {
            // a queue created here shows up once the engine reports it
            if (live != null) Pick(selected);
            return;
        }
        if (live == null)
        {
            Pick("main");
            return;
        }
        draft["running"] = J.B(live, "running");
        if (!reordered) draft["order"] = J.Arr(live, "order").DeepClone();
        RefreshStatus();
        FillOrder();
    }

    /// <summary>
    /// The user's arrangement applied to the live membership: downloads that left the queue are
    /// dropped, ones that joined since go to the end.
    /// </summary>
    private List<string> LiveOrder(List<string> wanted, string id)
    {
        var live = store.Queue(id) is JsonObject q ? J.Strings(q, "order") : wanted;
        var result = wanted.Where(live.Contains).ToList();
        result.AddRange(live.Where(l => !result.Contains(l)));
        return result;
    }

    private async void Save_Click(object sender, RoutedEventArgs e)
    {
        if (draft == null || saving) return;
        saving = true;
        failed = false;
        var wasReordered = reordered;
        var wanted = Order();
        // settings only: the engine keeps the queue's current order
        var payload = (JsonObject)draft.DeepClone();
        payload.Remove("order");
        RefreshStatus();
        try
        {
            var saved = await Engine.CallAsync("save_queue", J.Obj(("queue", payload)));
            if (wasReordered)
            {
                var id = J.S(saved, "id");
                await Engine.CallAsync("reorder_queue", J.Obj(("id", id), ("order", J.List(LiveOrder(wanted, id)))));
            }
            dirty = reordered = false;
        }
        catch (Exception ex)
        {
            // keep the edits (dirty) so the user can retry
            failed = true;
            Summary.Foreground = (Brush)Application.Current.Resources["SystemFillColorCriticalBrush"];
            Summary.Text = $"Could not save: {ex.Message}";
        }
        saving = false;
        RefreshStatus();
    }

    private void Revert_Click(object sender, RoutedEventArgs e) => Pick(selected);

    private async void NewQueue_Click(object sender, RoutedEventArgs e)
    {
        if (await Win.AskText(this, "New Queue", "", "", "Create") is not string queueName) return;
        var days7 = new JsonArray();
        for (var i = 0; i < 7; i++) days7.Add(true);
        // no order: the engine starts its own
        var q = J.Obj(("id", ""), ("name", queueName), ("builtin", false), ("maxConcurrent", 2), ("retryFailed", 3),
            ("startEnabled", false), ("startTime", "01:00"), ("stopEnabled", false), ("stopTime", "07:00"),
            ("days", days7), ("onFinish", "none"), ("running", false));
        try
        {
            var saved = await Engine.CallAsync("save_queue", J.Obj(("queue", q)));
            selected = J.S(saved, "id");
            draft = null;
            QueuesChanged();
        }
        catch (Exception ex)
        {
            store.Report($"Could not create the queue: {ex.Message}");
        }
    }

    private void Escape_Invoked(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs args)
    {
        args.Handled = true;
        Close();
    }
}
