using System.Collections.ObjectModel;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Windows.System;

namespace OpenDownloadManager;

/// <summary>One line of the per-connection table.</summary>
public sealed record ConnectionRow(string Number, string Received, string State, string Speed);

/// <summary>
/// Per-download progress window: big percentage, segment map, one line per connection, the speed
/// graph, a limit for this download and what to do when it finishes.
/// </summary>
public sealed partial class ProgressWindow : Window
{
    private readonly AppStore store = App.Store;
    private readonly string id;
    private readonly ObservableCollection<ConnectionRow> connections = [];
    private readonly TextBlock[] stats = new TextBlock[4];
    private readonly DispatcherTimer timer = new() { Interval = TimeSpan.FromSeconds(1) };
    private bool sawIncomplete, hovering, quiet;
    private DateTime? closeAt;

    public ProgressWindow(string id)
    {
        this.id = id;
        InitializeComponent();
        Win.Setup(this, "Download", 780, 700);
        Connections.ItemsSource = connections;
        var titles = new[] { "DOWNLOADED", "SPEED", "TIME LEFT", "RESUME" };
        for (var i = 0; i < titles.Length; i++)
        {
            Stats.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            var title = Ui.Note(titles[i]);
            var value = new TextBlock { Style = (Style)Application.Current.Resources["Num"] };
            Grid.SetColumn(title, i);
            Grid.SetColumn(value, i);
            Grid.SetRow(value, 1);
            Stats.Children.Add(title);
            Stats.Children.Add(value);
            stats[i] = value;
        }

        if (store.Row(id) is DownloadRow r)
        {
            sawIncomplete = !r.IsComplete;
            quiet = true;
            if (J.Num(r.Data, "speedLimitBps") is ulong limit && limit > 0)
            {
                LimitOn.IsOn = true;
                LimitBox.Text = Fmt.Mbps(limit);
                LimitRow.Visibility = Visibility.Visible;
            }
            else
            {
                LimitBox.Text = "5";
            }
            WhenFinished.Select(J.S(r.Data, "whenFinished"));
            quiet = false;
        }

        store.Changed += OnChanged;
        timer.Tick += (_, _) => Tick();
        timer.Start();
        Closed += (_, _) =>
        {
            store.Changed -= OnChanged;
            timer.Stop();
        };
        Update();
    }

    private void OnChanged(Change c)
    {
        if (c is Change.List or Change.Progress or Change.Totals) Update();
    }

    private static Visibility Show(bool on) => on ? Visibility.Visible : Visibility.Collapsed;

    private static string StateLabel(string state) => state switch
    {
        "receiving" => "Receiving",
        "reconnecting" => "Reconnecting",
        "done" => "Done",
        "connecting" => "Connecting",
        _ => "Idle",
    };

    private void Update()
    {
        if (store.Row(id) is not DownloadRow r)
        {
            Title = "Download";
            NameText.Text = "This download no longer exists.";
            return;
        }
        var d = r.Data;
        Title = $"{r.PercentText} {r.Name}";
        Icon.Glyph = r.Glyph;
        NameText.Text = r.Name;
        PlaceText.Text = $"{Fmt.Host(J.Str(d, "finalUrl") ?? r.Url)} · {r.SaveDir}";
        StatusText.Text = r.StatusText;
        StatusDot.Fill = r.StatusBrush;
        PercentText.Text = r.IsComplete ? "✓ Complete" : r.PercentText;
        stats[0].Text = $"{Fmt.Bytes(r.Received)} / {Fmt.Bytes(r.Total)}";
        stats[1].Text = r.SpeedText;
        stats[2].Text = r.EtaText;
        stats[3].Text = J.B(d, "resumable") ? "Supported" : "Not supported";
        var error = r.Status == "error" ? J.Str(d, "error") : null;
        ErrorText.Text = error ?? "";
        ErrorText.Visibility = Show(error != null);
        Verified.Visibility = Show(J.B(d, "checksumOk"));

        var segs = J.Objects(d, "segments").OrderBy(s => J.N(s, "start")).ToList();
        SegTitle.Text = $"Segment map · {Math.Max((int)J.N(d, "activeConnections"), segs.Count)} connections";
        Segments.Set(d);
        Spark.Set(d);
        PeakText.Text = $"peak {Fmt.Speed(Spark.Peak)}";
        for (var i = 0; i < segs.Count; i++)
        {
            var s = segs[i];
            var state = J.S(s, "state");
            var received = Fmt.Bytes(J.N(s, "received"));
            if (J.N(s, "end") < ulong.MaxValue / 2) received += $" / {Fmt.Bytes(J.N(s, "end") - Math.Min(J.N(s, "end"), J.N(s, "start")))}";
            var row = new ConnectionRow($"{i + 1:00}", received, StateLabel(state), state == "receiving" ? Fmt.Speed(J.N(s, "speedBps")) : Fmt.Dash);
            if (i >= connections.Count) connections.Add(row);
            else if (connections[i] != row) connections[i] = row;
        }
        while (connections.Count > segs.Count) connections.RemoveAt(connections.Count - 1);

        var complete = r.IsComplete;
        HideButton.Content = complete ? "Close" : "Hide";
        CancelButton.Visibility = ToggleButton.Visibility = Show(!complete);
        ToggleButton.Content = r.IsActive || r.Status == "queued" ? "Pause" : r.Status == "error" ? "Retry" : "Resume";
        RevealButton.Visibility = OpenButton.Visibility = Show(complete);

        if (!complete)
        {
            sawIncomplete = true;
            closeAt = null;
            ClosingText.Text = "";
        }
        else if (sawIncomplete)
        {
            // finished while open: close by itself in a moment
            sawIncomplete = false;
            closeAt = DateTime.Now.AddSeconds(5);
            Tick();
        }
    }

    private void Tick()
    {
        if (closeAt is not DateTime at) return;
        var left = (int)Math.Ceiling((at - DateTime.Now).TotalSeconds);
        if (left <= 0 && !hovering) Close();
        else ClosingText.Text = $"Closing in {Math.Max(0, left)}s";
    }

    private void Root_PointerEntered(object sender, PointerRoutedEventArgs e)
    {
        hovering = true;
        // looking at it: don't close it under the pointer
        closeAt = null;
        ClosingText.Text = "";
    }

    private void Root_PointerExited(object sender, PointerRoutedEventArgs e) => hovering = false;

    private void ApplyLimit()
    {
        LimitError.Visibility = Visibility.Collapsed;
        if (!LimitOn.IsOn)
        {
            store.Act("set_download_limit", J.Obj(("id", id), ("bps", null)));
            return;
        }
        // reject NaN / inf / 0 instead of silently turning the limit off
        if (Fmt.LimitBps(LimitBox.Text) is ulong bps)
        {
            store.Act("set_download_limit", J.Obj(("id", id), ("bps", bps)));
        }
        else
        {
            LimitError.Text = Fmt.LimitError;
            LimitError.Visibility = Visibility.Visible;
        }
    }

    private void LimitOn_Toggled(object sender, RoutedEventArgs e)
    {
        if (LimitRow == null) return;
        LimitRow.Visibility = Show(LimitOn.IsOn);
        if (!quiet) ApplyLimit();
    }

    private void ApplyLimit_Click(object sender, RoutedEventArgs e) => ApplyLimit();

    private void LimitBox_KeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key != VirtualKey.Enter) return;
        e.Handled = true;
        ApplyLimit();
    }

    private void WhenFinished_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (quiet || WhenFinished == null || WhenFinished.Value().Length == 0) return;
        store.Act("update_download", J.Obj(("id", id), ("patch", J.Obj(("whenFinished", WhenFinished.Value())))));
    }

    private void Hide_Click(object sender, RoutedEventArgs e) => Close();

    private async void Cancel_Click(object sender, RoutedEventArgs e)
    {
        var sure = await Win.Ask(this, "Cancel this download?",
            "The partial file is deleted and the download is removed from the list.", "Cancel Download", primaryIsDefault: false);
        if (sure != 1) return;
        store.Delete([id], false);
        Close();
    }

    private void Toggle_Click(object sender, RoutedEventArgs e)
    {
        if (store.Row(id) is not DownloadRow r) return;
        if (r.IsActive || r.Status == "queued") store.Pause([id]); else store.Resume([id]);
    }

    private void Reveal_Click(object sender, RoutedEventArgs e)
    {
        if (store.Row(id) is DownloadRow r) store.Reveal(r);
    }

    private void Open_Click(object sender, RoutedEventArgs e)
    {
        if (store.Row(id) is DownloadRow r) store.OpenFile(r);
        Close();
    }

    private void Escape_Invoked(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs args)
    {
        args.Handled = true;
        Close();
    }
}
