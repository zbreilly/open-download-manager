using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Text.Json.Nodes;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Windows.System;

namespace OpenDownloadManager;

/// <summary>One link in the grabber's list.</summary>
public sealed class GrabbedLink(string url, string name, string? cookies) : INotifyPropertyChanged
{
    private bool on = true;
    public event PropertyChangedEventHandler? PropertyChanged;

    public string Url { get; } = url;
    public string Name { get; } = name;
    /// <summary>This link's own cookies (never shared with other links).</summary>
    public string? Cookies { get; } = cookies;

    // nullable because that is what a CheckBox binds to
    public bool? On
    {
        get => on;
        set
        {
            if (on == (value == true)) return;
            on = value == true;
            PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(On)));
        }
    }

    public bool Chosen => on;
}

/// <summary>
/// Site Grabber: list the files linked from a page (or a list of links handed over by the
/// extension, a drop or an imported file) and add the chosen ones.
/// </summary>
public sealed partial class GrabberWindow : Window
{
    private static readonly (string Id, string Label, string Extensions)[] Presets =
    [
        ("all", "All files", ""),
        ("images", "Images", "jpg jpeg png gif webp bmp svg tif tiff avif heic"),
        ("video", "Video", "mp4 mkv avi mov wmv flv webm m4v mpg mpeg ts"),
        ("audio", "Audio", "mp3 wav flac aac m4a ogg opus wma"),
        ("documents", "Documents", "pdf doc docx xls xlsx ppt pptx odt ods txt epub csv"),
        ("archives", "Archives", "zip rar 7z tar gz tgz bz2 xz zst"),
        ("programs", "Programs", "exe msi dmg pkg deb rpm appimage apk iso"),
        ("custom", "Custom…", ""),
    ];

    private readonly AppStore store = App.Store;
    private readonly List<GrabbedLink> all = [];
    private readonly ObservableCollection<GrabbedLink> shown = [];
    private string saveDir = "";
    private string? referrer, userAgent;
    private bool busy, ready;

    public GrabberWindow()
    {
        InitializeComponent();
        Win.Setup(this, "Site Grabber", 920, 680);
        Links.ItemsSource = shown;
        foreach (var p in Presets) Types.Items.Add(new ComboBoxItem { Content = p.Label, Tag = p.Id });
        Types.SelectedIndex = 0;
        Queue.Items.Add(new ComboBoxItem { Content = "None (start now)", Tag = "" });
        foreach (var q in store.Queues) Queue.Items.Add(new ComboBoxItem { Content = J.S(q, "name"), Tag = J.S(q, "id") });
        Queue.SelectedIndex = 0;
        ready = true;
        Refresh();
    }

    private static Visibility Show(bool on) => on ? Visibility.Visible : Visibility.Collapsed;

    private List<GrabbedLink> Chosen => all.Where(l => l.Chosen).ToList();

    private List<string> Extensions()
    {
        var preset = Presets[Math.Clamp(Types.SelectedIndex, 0, Presets.Length - 1)];
        return Fmt.ParseExtensions(preset.Id == "custom" ? Custom.Text : preset.Extensions);
    }

    private void Fail(string message)
    {
        ErrorText.Text = message;
        ErrorText.Visibility = Show(message.Length > 0);
    }

    private void Refresh()
    {
        if (!ready) return;
        var chosen = Chosen.Count;
        CountText.Text = $"{chosen} of {all.Count} selected";
        DownloadButton.Content = $"Download {chosen}";
        DownloadButton.IsEnabled = AddQueueButton.IsEnabled = chosen > 0 && !busy;
        FindButton.Content = busy ? "Scanning…" : "Find Files";
        FindButton.IsEnabled = !busy && PageUrl.Text.Trim().Length > 0;
        Custom.Visibility = Show(Presets[Math.Clamp(Types.SelectedIndex, 0, Presets.Length - 1)].Id == "custom");
        SameHost.Visibility = Show(Depth.SelectedIndex > 0);
        DirText.Text = saveDir.Length == 0 ? "Category folders" : saveDir;
        UseCategories.Visibility = Show(saveDir.Length > 0);
        Placeholder.Text = busy ? "Scanning…" : "Files found on the page appear here.";
        Placeholder.Visibility = Show(all.Count == 0);
    }

    private void Refilter()
    {
        var q = Filter.Text.Trim();
        var wanted = q.Length == 0 ? all : all.Where(l => l.Url.Contains(q, StringComparison.OrdinalIgnoreCase)).ToList();
        shown.Clear();
        foreach (var l in wanted) shown.Add(l);
        Refresh();
    }

    /// <summary>Replace the list, keeping the first entry for each URL.</summary>
    private void SetLinks(IEnumerable<GrabbedLink> links)
    {
        foreach (var l in all) l.PropertyChanged -= LinkChanged;
        all.Clear();
        var seen = new HashSet<string>();
        foreach (var l in links)
        {
            if (!seen.Add(l.Url)) continue;
            l.PropertyChanged += LinkChanged;
            all.Add(l);
        }
        Refilter();
    }

    private void LinkChanged(object? sender, PropertyChangedEventArgs e) => Refresh();

    /// <summary>A list of links from the extension, a drop or an imported file.</summary>
    public void TakeList(JsonObject p)
    {
        Scan.Visibility = Visibility.Collapsed;
        Fail("");
        // a new list is a new context: nothing carries over from the last one
        referrer = J.Str(p, "referrer") is { Length: > 0 } r ? r : null;
        userAgent = J.Str(p, "userAgent") is { Length: > 0 } ua ? ua : null;
        SetLinks(J.Objects(p, "links").Where(l => J.S(l, "url").Length > 0).Select(l =>
        {
            var url = J.S(l, "url");
            var fileName = Fmt.UrlFileName(url);
            return new GrabbedLink(url, fileName.Length > 0 ? fileName : url, J.Str(l, "cookies") is { Length: > 0 } c ? c : null);
        }));
    }

    private async void Run()
    {
        var url = PageUrl.Text.Trim();
        if (url.Length == 0 || busy) return;
        busy = true;
        Fail("");
        Refresh();
        var args = J.Obj(("url", url), ("depth", Math.Max(0, Depth.SelectedIndex)), ("extensions", J.List(Extensions())),
            ("sameHost", SameHost.IsChecked == true));
        try
        {
            var found = await Engine.CallAsync("grab", args) as JsonArray ?? new JsonArray();
            busy = false;
            if (found.Count == 0) Fail("No matching files were found on that page.");
            referrer = url;
            userAgent = null;
            SetLinks(found.OfType<JsonObject>().Select(l => new GrabbedLink(J.S(l, "url"), J.S(l, "fileName"), null)));
        }
        catch (Exception e)
        {
            busy = false;
            Fail(e.Message);
            Refresh();
        }
    }

    private async void Add(bool start)
    {
        var chosen = Chosen;
        if (chosen.Count == 0 || busy) return;
        busy = true;
        Refresh();
        var queueId = (Queue.SelectedItem as ComboBoxItem)?.Tag as string ?? "";
        var list = new JsonArray();
        foreach (var l in chosen)
        {
            list.Add(J.Obj(
                ("url", l.Url),
                ("saveDir", J.OrNull(saveDir)),
                ("queueId", queueId.Length > 0 ? queueId : start ? null : "main"),
                ("start", start),
                ("connections", store.Settings["defaultConnections"]?.DeepClone()),
                ("referrer", J.OrNull(referrer)),
                ("cookies", J.OrNull(l.Cookies)),
                ("userAgent", J.OrNull(userAgent))));
        }
        try
        {
            await Engine.CallAsync("add_many", J.Obj(("reqs", list)));
            Close();
        }
        catch (Exception e)
        {
            busy = false;
            Fail(e.Message);
            Refresh();
        }
    }

    private void Find_Click(object sender, RoutedEventArgs e) => Run();
    private void PageUrl_TextChanged(object sender, TextChangedEventArgs e) => Refresh();
    private void Option_Changed(object sender, SelectionChangedEventArgs e) => Refresh();
    private void Filter_TextChanged(object sender, TextChangedEventArgs e) => Refilter();
    private void AddQueue_Click(object sender, RoutedEventArgs e) => Add(start: false);
    private void Download_Click(object sender, RoutedEventArgs e) => Add(start: true);
    private void Cancel_Click(object sender, RoutedEventArgs e) => Close();

    private void PageUrl_KeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key != VirtualKey.Enter) return;
        e.Handled = true;
        Run();
    }

    private void SelectAll_Click(object sender, RoutedEventArgs e)
    {
        foreach (var l in all) l.On = true;
    }

    private void SelectNone_Click(object sender, RoutedEventArgs e)
    {
        foreach (var l in all) l.On = false;
    }

    private async void Choose_Click(object sender, RoutedEventArgs e)
    {
        if (await Win.PickFolder(this) is not string picked) return;
        saveDir = picked;
        Refresh();
    }

    private void UseCategories_Click(object sender, RoutedEventArgs e)
    {
        saveDir = "";
        Refresh();
    }

    private void Escape_Invoked(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs args)
    {
        args.Handled = true;
        Close();
    }
}
