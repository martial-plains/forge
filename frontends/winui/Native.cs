// SPDX-License-Identifier: MIT OR Apache-2.0
using System.Runtime.InteropServices;
using System.Text.Json;

namespace Forge;

/// <summary>forge.dll: the editor, as built by crates/capi.</summary>
internal static class Native
{
    private const string Lib = "forge";

    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] private static extern IntPtr forge_new(IntPtr filesJson, int noConfig);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] private static extern IntPtr forge_last_error();
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] public static extern void forge_free(IntPtr h);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] private static extern void forge_string_free(IntPtr s);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] public static extern void forge_key(IntPtr h, int kind, uint ch, int flags);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] private static extern void forge_text(IntPtr h, IntPtr utf8);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] public static extern void forge_mouse(IntPtr h, int kind, ushort col, ushort row);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] private static extern void forge_run_command(IntPtr h, IntPtr utf8);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] public static extern void forge_interrupt(IntPtr h);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] public static extern void forge_save_session(IntPtr h);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] public static extern void forge_set_native_scenes(IntPtr h, int on);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] private static extern void forge_dialog_result(IntPtr h, uint id, IntPtr utf8OrNull);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] private static extern IntPtr forge_kill_top(IntPtr h);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] private static extern IntPtr forge_grid_json(IntPtr h, ushort cols, ushort rows, int dark);

    // Key kinds (crates/capi: KEY_*).
    public const int Char = 0, Enter = 1, Tab = 2, BackTab = 3, Backspace = 4, Delete = 5, Escape = 6,
        Left = 7, Right = 8, Up = 9, Down = 10, Home = 11, End = 12, PageUp = 13, PageDown = 14, Insert = 15, F1 = 20;
    // Modifier flags.
    public const int Control = 1, Meta = 2, Shift = 4;
    // Mouse kinds.
    public const int MouseDown = 0, MouseDrag = 1, MouseUp = 2, MouseRightDown = 3, ScrollUp = 4, ScrollDown = 5;

    private static string? TakeString(IntPtr p)
    {
        if (p == IntPtr.Zero) return null;
        var s = Marshal.PtrToStringUTF8(p);
        forge_string_free(p);
        return s;
    }

    private static T WithUtf8<T>(string? s, Func<IntPtr, T> use)
    {
        var p = s is null ? IntPtr.Zero : Marshal.StringToCoTaskMemUTF8(s);
        try { return use(p); } finally { if (p != IntPtr.Zero) Marshal.FreeCoTaskMem(p); }
    }

    public static IntPtr Start(string[] files, bool noConfig)
    {
        var h = WithUtf8(JsonSerializer.Serialize(files), p => forge_new(p, noConfig ? 1 : 0));
        if (h == IntPtr.Zero) throw new InvalidOperationException(TakeString(forge_last_error()) ?? "Forge could not start");
        return h;
    }

    public static void Text(IntPtr h, string s) => WithUtf8(s, p => { forge_text(h, p); return 0; });
    public static void Run(IntPtr h, string command) => WithUtf8(command, p => { forge_run_command(h, p); return 0; });
    public static void DialogResult(IntPtr h, uint id, string? path) => WithUtf8(path, p => { forge_dialog_result(h, id, p); return 0; });
    public static string? KillTop(IntPtr h) => TakeString(forge_kill_top(h));

    /// <summary>The screen. Blocks until the editor answers: call it off the UI thread.</summary>
    public static Grid? ReadGrid(IntPtr h, int cols, int rows, bool dark)
    {
        var json = TakeString(forge_grid_json(h, (ushort)cols, (ushort)rows, dark ? 1 : 0));
        return json is null ? null : Grid.Parse(json);
    }
}
