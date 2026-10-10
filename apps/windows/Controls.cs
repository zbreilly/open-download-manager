using System.Text.Json.Nodes;
using Microsoft.UI;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;
using Windows.Foundation;

namespace OpenDownloadManager;

/// <summary>One cell per connection segment, sized by its share of the file.</summary>
public sealed class SegmentMap : Canvas
{
    private JsonObject? download;
    public bool ShowWriteHead { get; set; }

    public SegmentMap()
    {
        Height = 18;
        HorizontalAlignment = HorizontalAlignment.Stretch;
        SizeChanged += (_, _) => Draw();
    }

    public void Set(JsonObject d)
    {
        download = d;
        Draw();
    }

    private static Brush Track => (Brush)Application.Current.Resources["ControlStrongFillColorDisabledBrush"];

    private void Add(double x, double width, Brush fill, double opacity = 1, double radius = 3)
    {
        if (width <= 0) return;
        var r = new Rectangle { Width = width, Height = ActualHeight, Fill = fill, Opacity = opacity, RadiusX = radius, RadiusY = radius };
        SetLeft(r, x);
        Children.Add(r);
    }

    private void Draw()
    {
        Children.Clear();
        var w = ActualWidth;
        if (download == null || w <= 0 || ActualHeight <= 0) return;
        var status = J.S(download, "status");
        var color = Palette.Status(status);
        var segs = J.Objects(download, "segments").OrderBy(s => J.N(s, "start")).ToList();
        if (segs.Count == 0)
        {
            Add(0, w, Track, 0.5, 4);
            if (status == "complete") Add(0, w, color, 0.55, 4);
            return;
        }
        static bool Known(JsonObject s) => J.N(s, "end") < ulong.MaxValue / 2;
        double total = Math.Max(1, J.Num(download, "totalBytes") ?? segs.Max(s => J.N(s, "start") + J.N(s, "received")));
        double Len(JsonObject s) => Known(s) ? Math.Max(0, (double)J.N(s, "end") - J.N(s, "start")) : Math.Max(1, total - J.N(s, "start"));
        var sizes = segs.Select(s => Math.Max(Len(s) / total, 0.012)).ToList();
        var sum = sizes.Sum();
        const double gap = 3;
        var usable = w - gap * (segs.Count - 1);
        double x = 0;
        for (var i = 0; i < segs.Count; i++)
        {
            var s = segs[i];
            var sw = Math.Max(3, usable * sizes[i] / sum);
            Add(x, sw, Track, 0.5);
            var frac = status == "complete" ? 1.0 : Math.Min(1.0, J.N(s, "received") / Math.Max(Len(s), 1));
            if (frac > 0)
            {
                var done = J.S(s, "state") == "done" || frac >= 1;
                Add(x, sw * frac, color, done ? 0.55 : 1);
                if (ShowWriteHead && J.S(s, "state") == "receiving" && frac < 1)
                {
                    var head = Math.Min(8, sw * frac);
                    Add(x + sw * frac - head, head, new SolidColorBrush(Colors.White), 0.35, 0);
                }
            }
            x += sw + gap;
        }
    }
}

/// <summary>60-second speed history.</summary>
public sealed class Sparkline : Canvas
{
    private List<ulong> samples = [];

    public Sparkline()
    {
        Height = 56;
        HorizontalAlignment = HorizontalAlignment.Stretch;
        SizeChanged += (_, _) => Draw();
    }

    /// <summary>The fastest sample shown, for the "peak" label.</summary>
    public ulong Peak => samples.Count == 0 ? 0 : samples.Max();

    public void Set(JsonObject d)
    {
        samples = J.Arr(d, "speedHistory").Select(v => v is JsonValue j && j.TryGetValue<ulong>(out var u) ? u : 0).ToList();
        Draw();
    }

    private void Draw()
    {
        Children.Clear();
        double w = ActualWidth, h = ActualHeight;
        var s = samples.Skip(Math.Max(0, samples.Count - 60)).ToList();
        if (s.Count < 2 || w <= 0 || h <= 0) return;
        double peak = Math.Max(1, s.Max());
        var off = 60 - s.Count;
        Point At(int i) => new(w * (off + i) / 59.0, h - 3 - s[i] / peak * (h - 8));
        var color = Palette.Status("downloading");
        var line = new Polyline { Stroke = color, StrokeThickness = 1.6 };
        var area = new Polygon { Fill = color, Opacity = 0.18 };
        for (var i = 0; i < s.Count; i++)
        {
            line.Points.Add(At(i));
            area.Points.Add(At(i));
        }
        area.Points.Add(new Point(At(s.Count - 1).X, h));
        area.Points.Add(new Point(At(0).X, h));
        Children.Add(area);
        Children.Add(line);
    }
}

/// <summary>Builders for the form-like windows (settings, add, scheduler).</summary>
public static class Ui
{
    private static Style Res(string key) => (Style)Application.Current.Resources[key];

    public static TextBlock Section(string title) => new() { Text = title, Style = Res("Section") };

    public static TextBlock Note(string text) => new()
    {
        Text = text,
        TextWrapping = TextWrapping.Wrap,
        FontSize = 12,
        Foreground = (Brush)Application.Current.Resources["TextFillColorSecondaryBrush"],
    };

    /// <summary>A card with a label (and optional note) on the left and a control on the right.</summary>
    public static Border Row(string label, FrameworkElement? control, string? note = null)
    {
        var grid = new Grid { ColumnSpacing = 16 };
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var text = new StackPanel { VerticalAlignment = VerticalAlignment.Center };
        text.Children.Add(new TextBlock { Text = label, TextWrapping = TextWrapping.Wrap });
        if (note != null) text.Children.Add(Note(note));
        grid.Children.Add(text);
        if (control != null)
        {
            control.VerticalAlignment = VerticalAlignment.Center;
            Grid.SetColumn(control, 1);
            grid.Children.Add(control);
        }
        return new Border { Style = Res("Card"), Child = grid };
    }

    /// <summary>A card holding one full-width control under a label.</summary>
    public static Border Wide(string label, FrameworkElement control, string? note = null)
    {
        var panel = new StackPanel { Spacing = 6 };
        panel.Children.Add(new TextBlock { Text = label });
        panel.Children.Add(control);
        if (note != null) panel.Children.Add(Note(note));
        return new Border { Style = Res("Card"), Child = panel };
    }

    public static ToggleSwitch Switch() => new() { OnContent = "", OffContent = "", MinWidth = 0 };

    public static ComboBox Combo(IEnumerable<(string Value, string Label)> items, double minWidth = 180)
    {
        var box = new ComboBox { MinWidth = minWidth };
        foreach (var (value, label) in items) box.Items.Add(new ComboBoxItem { Content = label, Tag = value });
        return box;
    }

    public static string Value(this ComboBox box) => (box.SelectedItem as ComboBoxItem)?.Tag as string ?? "";

    public static void Select(this ComboBox box, string value)
    {
        foreach (var item in box.Items.OfType<ComboBoxItem>())
        {
            if (item.Tag as string != value) continue;
            if (!ReferenceEquals(box.SelectedItem, item)) box.SelectedItem = item;
            return;
        }
    }

    public static NumberBox Number(double min, double max) => new()
    {
        Minimum = min,
        Maximum = max,
        SpinButtonPlacementMode = NumberBoxSpinButtonPlacementMode.Inline,
        SmallChange = 1,
        MinWidth = 150,
    };

    public static TextBox Mono(string placeholder = "") => new()
    {
        PlaceholderText = placeholder,
        FontFamily = (FontFamily)Application.Current.Resources["Mono"],
        IsSpellCheckEnabled = false,
    };
}
