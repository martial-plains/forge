// SPDX-License-Identifier: MIT OR Apache-2.0
using Microsoft.Graphics.Canvas;
using Microsoft.Graphics.Canvas.Text;
using Microsoft.Graphics.Canvas.UI.Xaml;
using Microsoft.UI;
using Microsoft.UI.Input;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Input;
using Windows.ApplicationModel.DataTransfer;
using Windows.Storage.Pickers;
using Windows.System;
using Windows.UI;
using WinRT.Interop;

namespace Forge;

/// <summary>
/// Forge draws its whole interface (windows, mode lines, menu bar, tool bar,
/// tabs, popups) as a grid of styled runs and picture slots; this window
/// draws that with Win2D, sends WinUI's keys, text and pointer to the editor,
/// and shows Windows' own file pickers when the editor asks for one.
/// Canvases are drawn by the editor with wgpu (Direct3D 12) and arrive as pictures.
/// </summary>
public sealed partial class MainWindow : Window
{
    private readonly IntPtr forge;
    private readonly CancellationTokenSource stop = new();
    private volatile bool closed;
    private Grid? grid;
    private double cellW = 9, cellH = 18;
    private volatile int cols = 100, rows = 30;
    private volatile bool dark = true;
    private uint lastDialog;
    private string? lastClip;
    private bool dragging;
    private readonly Dictionary<string, CanvasBitmap?> bitmaps = new();
    private readonly Dictionary<int, CanvasTextFormat> formats = new();

    public MainWindow(string[] files, bool noConfig)
    {
        InitializeComponent();
        forge = Native.Start(files, noConfig);
        Native.forge_set_native_scenes(forge, 1);
        Title = "Forge";
        Closed += (_, _) =>
        {
            closed = true;
            stop.Cancel();
            Native.forge_save_session(forge);
        };
        _ = Task.Run(Pump);
        _ = Task.Run(ClipboardPump);
        Surface.Focus(FocusState.Programmatic);
    }

    // ---- the editor's grid: asked for on a thread, drawn on the UI thread

    private async Task Pump()
    {
        while (!stop.IsCancellationRequested)
        {
            var g = Native.ReadGrid(forge, cols, rows, dark);
            if (g is not null && !closed)
            {
                DispatcherQueue.TryEnqueue(() =>
                {
                    if (!closed) Show(g);
                });
            }
            await Task.Delay(16);
        }
    }

    private void Show(Grid g)
    {
        if (closed) return;
        var title = $"{(g.Modified ? "• " : "")}{g.Title} — Forge";
        if (Title != title) Title = title;
        if (g.Quit) { Close(); return; }
        if (g.Dialog is { } d && d.Id != lastDialog)
        {
            lastDialog = d.Id;
            _ = Pick(d);
        }
        if (!g.SameAs(grid))
        {
            grid = g;
            foreach (var p in g.Pictures)
            {
                // A canvas is drawn from its scene, not loaded as a picture.
                if (!SceneDraw.IsScene(p.Path)) LoadPicture(p.Path);
            }
            Surface.Invalidate();
        }
    }

    private void LoadPicture(string path)
    {
        if (bitmaps.ContainsKey(path)) return;
        bitmaps[path] = null;
        _ = Task.Run(async () =>
        {
            try
            {
                var bmp = await CanvasBitmap.LoadAsync(Surface, path);
                DispatcherQueue.TryEnqueue(() =>
                {
                    if (closed) return;
                    bitmaps[path] = bmp;
                    Surface.Invalidate();
                });
            }
            catch { /* an unreadable picture stays blank */ }
        });
    }

    private async Task ClipboardPump()
    {
        // The kill ring is the system clipboard too.
        while (!stop.IsCancellationRequested)
        {
            var top = Native.KillTop(forge);
            if (top is not null && top != lastClip && !closed)
            {
                lastClip = top;
                DispatcherQueue.TryEnqueue(() =>
                {
                    if (closed) return;
                    var pkg = new DataPackage();
                    pkg.SetText(top);
                    Clipboard.SetContent(pkg);
                });
            }
            await Task.Delay(400);
        }
    }

    // ---- drawing

    private void OnCreateResources(CanvasControl sender, Microsoft.Graphics.Canvas.UI.CanvasCreateResourcesEventArgs args)
    {
        Measure(sender);
        UpdateGridSize(sender.ActualWidth, sender.ActualHeight);
    }

    private CanvasTextFormat Format(int flags)
    {
        var key = (flags & 1) | (flags & 2);
        if (!formats.TryGetValue(key, out var f))
        {
            f = new CanvasTextFormat
            {
                FontFamily = "Cascadia Mono, Consolas",
                FontSize = 15,
                FontWeight = (key & 1) != 0 ? Microsoft.UI.Text.FontWeights.Bold : Microsoft.UI.Text.FontWeights.Normal,
                FontStyle = (key & 2) != 0 ? Windows.UI.Text.FontStyle.Italic : Windows.UI.Text.FontStyle.Normal,
                WordWrapping = CanvasWordWrapping.NoWrap,
            };
            formats[key] = f;
        }
        return f;
    }

    private void Measure(CanvasControl device)
    {
        using var layout = new CanvasTextLayout(device, "M", Format(0), 1000, 1000);
        cellW = Math.Max(1, layout.LayoutBounds.Width);
        cellH = Math.Max(1, layout.LayoutBounds.Height);
    }

    private static Color Rgb(uint c) => Color.FromArgb(255, (byte)(c >> 16), (byte)(c >> 8), (byte)c);

    private void OnDraw(CanvasControl sender, CanvasDrawEventArgs args)
    {
        var ds = args.DrawingSession;
        if (grid is not { } g) return;
        ds.Clear(Rgb(g.Bg));
        foreach (var r in g.Runs)
        {
            float x = (float)(r.Col * cellW), y = (float)(r.Row * cellH), w = (float)(r.Cols * cellW);
            if (r.Bg != g.Bg) ds.FillRectangle(x, y, w, (float)cellH, Rgb(r.Bg));
            var fg = Rgb(r.Fg);
            if (r.Text.Trim().Length > 0) ds.DrawText(r.Text, x, y, fg, Format(r.Flags));
            if (r.Underline) ds.FillRectangle(x, y + (float)cellH - 2, w, 1, fg);
            if (r.Strike) ds.FillRectangle(x, y + (float)cellH / 2, w, 1, fg);
        }
        foreach (var p in g.Pictures)
        {
            if (SceneDraw.IsScene(p.Path))
            {
                SceneDraw.Draw(sender, ds, p.Path, p.Col * cellW, p.Row * cellH, p.Cols * cellW, p.Rows * cellH);
                continue;
            }
            if (!bitmaps.TryGetValue(p.Path, out var bmp) || bmp is null) continue;
            double bw = p.Cols * cellW, bh = p.Rows * cellH;
            var s = Math.Min(Math.Min(bw / bmp.Size.Width, bh / bmp.Size.Height), 1.0);
            ds.DrawImage(bmp, new Windows.Foundation.Rect(p.Col * cellW, p.Row * cellH, bmp.Size.Width * s, bmp.Size.Height * s));
        }
        var cur = Rgb(g.Fg);
        cur.A = 140;
        ds.FillRectangle((float)(g.CursorCol * cellW), (float)(g.CursorRow * cellH), (float)cellW, (float)cellH, cur);
    }

    private void OnSizeChanged(object sender, SizeChangedEventArgs e)
    {
        UpdateGridSize(e.NewSize.Width, e.NewSize.Height);
    }

    private void UpdateGridSize(double width, double height)
    {
        cols = Math.Max(20, (int)(width / cellW));
        rows = Math.Max(5, (int)(height / cellH));
        dark = Application.Current.RequestedTheme == ApplicationTheme.Dark;
    }

    // ---- keys and text

    private static bool Down(VirtualKey k) => InputKeyboardSource.GetKeyStateForCurrentThread(k).HasFlag(Windows.UI.Core.CoreVirtualKeyStates.Down);

    private int Mods()
    {
        var f = 0;
        if (Down(VirtualKey.Control)) f |= Native.Control;
        if (Down(VirtualKey.Menu)) f |= Native.Meta;
        if (Down(VirtualKey.Shift)) f |= Native.Shift;
        return f;
    }

    private void OnKeyDown(object sender, KeyRoutedEventArgs e)
    {
        var mods = Mods();
        // Ctrl+Alt is AltGr: its characters arrive as text.
        if ((mods & Native.Control) != 0 && (mods & Native.Meta) != 0) return;
        if ((mods & Native.Control) != 0 && (mods & Native.Shift) != 0 && e.Key == VirtualKey.V)
        {
            _ = Paste();
            e.Handled = true;
            return;
        }
        if (e.Key == VirtualKey.G && (mods & Native.Control) != 0) Native.forge_interrupt(forge);
        var kind = e.Key switch
        {
            VirtualKey.Enter => Native.Enter,
            VirtualKey.Tab => (mods & Native.Shift) != 0 ? Native.BackTab : Native.Tab,
            VirtualKey.Back => Native.Backspace,
            VirtualKey.Delete => Native.Delete,
            VirtualKey.Escape => Native.Escape,
            VirtualKey.Left => Native.Left,
            VirtualKey.Right => Native.Right,
            VirtualKey.Up => Native.Up,
            VirtualKey.Down => Native.Down,
            VirtualKey.Home => Native.Home,
            VirtualKey.End => Native.End,
            VirtualKey.PageUp => Native.PageUp,
            VirtualKey.PageDown => Native.PageDown,
            VirtualKey.Insert => Native.Insert,
            >= VirtualKey.F1 and <= VirtualKey.F12 => Native.F1 + (e.Key - VirtualKey.F1),
            _ => -1,
        };
        if (kind >= 0)
        {
            Native.forge_key(forge, kind, 0, mods);
            e.Handled = true;
            return;
        }
        // Control and Alt chords are keys; everything else is text (OnCharacter).
        if ((mods & (Native.Control | Native.Meta)) != 0)
        {
            var c = ChordChar(e.Key, (mods & Native.Shift) != 0);
            if (c != 0)
            {
                Native.forge_key(forge, Native.Char, c, mods);
                e.Handled = true;
            }
        }
    }

    /// <summary>The character a letter, digit or punctuation key makes in a chord (US layout for punctuation).</summary>
    private static uint ChordChar(VirtualKey k, bool shift)
    {
        if (k >= VirtualKey.A && k <= VirtualKey.Z) return (uint)(shift ? 'A' + (k - VirtualKey.A) : 'a' + (k - VirtualKey.A));
        if (k >= VirtualKey.Number0 && k <= VirtualKey.Number9) return (uint)('0' + (k - VirtualKey.Number0));
        if (k == VirtualKey.Space) return ' ';
        return (int)k switch
        {
            186 => shift ? ':' : ';', 187 => shift ? '+' : '=', 188 => shift ? '<' : ',', 189 => shift ? '_' : '-',
            190 => shift ? '>' : '.', 191 => shift ? '?' : '/', 192 => shift ? '~' : '`', 219 => shift ? '{' : '[',
            220 => shift ? '|' : '\\', 221 => shift ? '}' : ']', 222 => shift ? '"' : '\'', _ => 0u,
        };
    }

    private string pendingHigh = "";

    private void OnCharacter(UIElement sender, CharacterReceivedRoutedEventArgs e)
    {
        var mods = Mods();
        if ((mods & Native.Control) != 0 && (mods & Native.Meta) == 0) return;   // a chord, handled in OnKeyDown
        if ((mods & Native.Meta) != 0 && (mods & Native.Control) == 0) return;
        var ch = e.Character;
        if (ch < ' ' || ch == 0x7f) return;                                      // controls came as keys
        if (char.IsHighSurrogate(ch)) { pendingHigh = ch.ToString(); e.Handled = true; return; }
        var s = char.IsLowSurrogate(ch) && pendingHigh.Length > 0 ? pendingHigh + ch : ch.ToString();
        pendingHigh = "";
        if (s.Length == 1) Native.forge_key(forge, Native.Char, ch, 0);
        else Native.Text(forge, s);
        e.Handled = true;
    }

    private async Task Paste()
    {
        var view = Clipboard.GetContent();
        if (view.Contains(StandardDataFormats.Text))
            Native.Text(forge, await view.GetTextAsync());
    }

    // ---- pointer

    private (ushort, ushort) CellAt(PointerRoutedEventArgs e)
    {
        var p = e.GetCurrentPoint(Surface).Position;
        return ((ushort)Math.Max(0, p.X / cellW), (ushort)Math.Max(0, p.Y / cellH));
    }

    private void OnPointerPressed(object sender, PointerRoutedEventArgs e)
    {
        Surface.Focus(FocusState.Pointer);
        var props = e.GetCurrentPoint(Surface).Properties;
        var (c, r) = CellAt(e);
        if (props.IsLeftButtonPressed) { dragging = true; Surface.CapturePointer(e.Pointer); Native.forge_mouse(forge, Native.MouseDown, c, r); }
        else if (props.IsRightButtonPressed) Native.forge_mouse(forge, Native.MouseRightDown, c, r);
    }

    private void OnPointerMoved(object sender, PointerRoutedEventArgs e)
    {
        if (!dragging) return;
        var (c, r) = CellAt(e);
        Native.forge_mouse(forge, Native.MouseDrag, c, r);
    }

    private void OnPointerReleased(object sender, PointerRoutedEventArgs e)
    {
        if (!dragging) return;
        dragging = false;
        Surface.ReleasePointerCapture(e.Pointer);
        var (c, r) = CellAt(e);
        Native.forge_mouse(forge, Native.MouseUp, c, r);
    }

    private void OnWheel(object sender, PointerRoutedEventArgs e)
    {
        var delta = e.GetCurrentPoint(Surface).Properties.MouseWheelDelta;
        var (c, r) = CellAt(e);
        for (var i = 0; i < 3; i++) Native.forge_mouse(forge, delta > 0 ? Native.ScrollUp : Native.ScrollDown, c, r);
    }

    // ---- header buttons and the system pickers

    private void OnOpen(object sender, RoutedEventArgs e) => Native.Run(forge, "find-file-dialog");
    private void OnFolder(object sender, RoutedEventArgs e) => Native.Run(forge, "find-directory-dialog");
    private void OnSave(object sender, RoutedEventArgs e) => Native.Run(forge, "save-buffer");
    private void OnCommand(object sender, RoutedEventArgs e) { Native.Run(forge, "execute-extended-command"); Surface.Focus(FocusState.Programmatic); }

    /// <summary>Windows' own picker for what the editor asked for (File ▸ Open File with Picker…).</summary>
    private async Task Pick(Dialog d)
    {
        var hwnd = WindowNative.GetWindowHandle(this);
        string? path = null;
        try
        {
            switch (d.Kind)
            {
                case "open_folder":
                {
                    var p = new FolderPicker { SuggestedStartLocation = PickerLocationId.DocumentsLibrary };
                    p.FileTypeFilter.Add("*");
                    InitializeWithWindow.Initialize(p, hwnd);
                    path = (await p.PickSingleFolderAsync())?.Path;
                    break;
                }
                case "save_as":
                {
                    var p = new FileSavePicker { SuggestedStartLocation = PickerLocationId.DocumentsLibrary, SuggestedFileName = Path.GetFileNameWithoutExtension(d.Name) };
                    var ext = Path.GetExtension(d.Name);
                    p.FileTypeChoices.Add("File", new List<string> { string.IsNullOrEmpty(ext) ? ".txt" : ext });
                    InitializeWithWindow.Initialize(p, hwnd);
                    path = (await p.PickSaveFileAsync())?.Path;
                    break;
                }
                default:
                {
                    var p = new FileOpenPicker { SuggestedStartLocation = PickerLocationId.DocumentsLibrary };
                    p.FileTypeFilter.Add("*");
                    InitializeWithWindow.Initialize(p, hwnd);
                    path = (await p.PickSingleFileAsync())?.Path;
                    break;
                }
            }
        }
        finally
        {
            if (!closed)
            {
                Native.DialogResult(forge, d.Id, path);
                Surface.Focus(FocusState.Programmatic);
            }
        }
    }
}
