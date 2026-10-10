using System.Runtime.InteropServices;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.Windows.Storage.Pickers;
using Windows.Graphics;

namespace OpenDownloadManager;

/// <summary>Things every window does: size, icon, theme, dialogs, pickers.</summary>
public static class Win
{
    [DllImport("user32.dll")]
    private static extern uint GetDpiForWindow(IntPtr hWnd);
    [DllImport("user32.dll")]
    private static extern bool SetForegroundWindow(IntPtr hWnd);

    public static IntPtr Handle(Window w) => WinRT.Interop.WindowNative.GetWindowHandle(w);

    /// <summary>Title, icon, backdrop, size (in device-independent pixels) and theme.</summary>
    public static void Setup(Window w, string title, int width, int height)
    {
        w.Title = title;
        try { w.SystemBackdrop = new MicaBackdrop(); } catch { }
        try { w.AppWindow.SetIcon(Path.Combine(AppContext.BaseDirectory, "Assets", "icon.ico")); } catch { }
        var scale = Math.Max(1.0, GetDpiForWindow(Handle(w)) / 96.0);
        w.AppWindow.Resize(new SizeInt32((int)(width * scale), (int)(height * scale)));
        App.Register(w);
        ApplyTheme(w);
    }

    public static void ApplyTheme(Window w)
    {
        if (w.Content is not FrameworkElement root) return;
        root.RequestedTheme = J.S(App.Store.Settings, "theme") switch
        {
            "light" => ElementTheme.Light,
            "system" => ElementTheme.Default,
            _ => ElementTheme.Dark,
        };
    }

    /// <summary>Bring a window up and to the front.</summary>
    public static void Show(Window w)
    {
        w.AppWindow.Show();
        w.Activate();
        SetForegroundWindow(Handle(w));
    }

    private static async Task<ContentDialogResult> ShowDialog(Window owner, ContentDialog dialog)
    {
        if (owner.Content is not FrameworkElement root || root.XamlRoot == null) return ContentDialogResult.None;
        dialog.XamlRoot = root.XamlRoot;
        dialog.RequestedTheme = root.RequestedTheme;
        try { return await dialog.ShowAsync(); }
        catch { return ContentDialogResult.None; } // another dialog is open
    }

    /// <summary>A message with one button.</summary>
    public static Task Message(Window owner, string title, string text) =>
        ShowDialog(owner, new ContentDialog { Title = title, Content = Wrapped(text), CloseButtonText = "OK" });

    /// <summary>
    /// Ask a question with up to two answers and Cancel. Returns 1 for <paramref name="primary"/>,
    /// 2 for <paramref name="secondary"/>, 0 for Cancel.
    /// </summary>
    public static async Task<int> Ask(Window owner, string title, string text, string primary, string? secondary = null, bool primaryIsDefault = true)
    {
        var dialog = new ContentDialog
        {
            Title = title,
            Content = Wrapped(text),
            PrimaryButtonText = primary,
            CloseButtonText = "Cancel",
            DefaultButton = primaryIsDefault ? ContentDialogButton.Primary : ContentDialogButton.Close,
        };
        if (secondary != null) dialog.SecondaryButtonText = secondary;
        return await ShowDialog(owner, dialog) switch
        {
            ContentDialogResult.Primary => 1,
            ContentDialogResult.Secondary => 2,
            _ => 0,
        };
    }

    /// <summary>Ask for one line of text; null when cancelled or left empty.</summary>
    public static async Task<string?> AskText(Window owner, string title, string text, string initial, string ok)
    {
        var box = new TextBox { Text = initial, AcceptsReturn = false };
        var panel = new StackPanel { Spacing = 10 };
        if (text.Length > 0) panel.Children.Add(Wrapped(text));
        panel.Children.Add(box);
        var dialog = new ContentDialog
        {
            Title = title,
            Content = panel,
            PrimaryButtonText = ok,
            CloseButtonText = "Cancel",
            DefaultButton = ContentDialogButton.Primary,
        };
        var result = await ShowDialog(owner, dialog);
        var value = box.Text.Trim();
        return result == ContentDialogResult.Primary && value.Length > 0 ? value : null;
    }

    private static TextBlock Wrapped(string text) => new() { Text = text, TextWrapping = TextWrapping.Wrap, IsTextSelectionEnabled = true };

    public static async Task<string?> PickFolder(Window owner)
    {
        try
        {
            var picker = new FolderPicker(owner.AppWindow.Id) { SuggestedStartLocation = PickerLocationId.Downloads };
            return (await picker.PickSingleFolderAsync())?.Path;
        }
        catch (Exception e)
        {
            App.Store.Report($"Could not open the folder picker: {e.Message}");
            return null;
        }
    }

    public static async Task<string?> PickTextFile(Window owner)
    {
        try
        {
            var picker = new FileOpenPicker(owner.AppWindow.Id);
            picker.FileTypeFilter.Add(".txt");
            picker.FileTypeFilter.Add(".csv");
            picker.FileTypeFilter.Add("*");
            return (await picker.PickSingleFileAsync())?.Path;
        }
        catch (Exception e)
        {
            App.Store.Report($"Could not open the file picker: {e.Message}");
            return null;
        }
    }

    public static async Task<string?> PickSaveFile(Window owner, string suggestedName)
    {
        try
        {
            var picker = new FileSavePicker(owner.AppWindow.Id) { SuggestedFileName = suggestedName, DefaultFileExtension = ".txt" };
            picker.FileTypeChoices.Add("Text file", [".txt"]);
            return (await picker.PickSaveFileAsync())?.Path;
        }
        catch (Exception e)
        {
            App.Store.Report($"Could not open the file picker: {e.Message}");
            return null;
        }
    }
}
