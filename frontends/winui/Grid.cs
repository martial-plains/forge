// SPDX-License-Identifier: MIT OR Apache-2.0
using System.Text.Json;

namespace Forge;

/// <summary>One stretch of cells on a row that share a style.</summary>
public sealed record Run(int Row, int Col, int Cols, string Text, uint Fg, uint Bg, int Flags)
{
    public bool Bold => (Flags & 1) != 0;
    public bool Italic => (Flags & 2) != 0;
    public bool Underline => (Flags & 4) != 0;
    public bool Strike => (Flags & 8) != 0;
}

public sealed record Picture(int Col, int Row, int Cols, int Rows, string Path);

public sealed record Dialog(uint Id, string Kind, string Title, string Start, string Name);

/// <summary>What Forge wants drawn: crates/capi's forge_grid_json.</summary>
public sealed class Grid
{
    public int Cols, Rows, CursorCol, CursorRow;
    public uint Fg, Bg;
    public string Title = "";
    public bool Modified, Quit;
    public string? Busy, Crashed;
    public List<Run> Runs = new();
    public List<Picture> Pictures = new();
    public Dialog? Dialog;

    public static Grid Parse(string json)
    {
        using var doc = JsonDocument.Parse(json);
        var r = doc.RootElement;
        var g = new Grid
        {
            Cols = r.GetProperty("cols").GetInt32(),
            Rows = r.GetProperty("rows").GetInt32(),
            Fg = r.GetProperty("fg").GetUInt32(),
            Bg = r.GetProperty("bg").GetUInt32(),
            Title = r.GetProperty("title").GetString() ?? "",
            Modified = r.GetProperty("modified").GetBoolean(),
            Quit = r.GetProperty("quit").GetBoolean(),
            Busy = Str(r, "busy"),
            Crashed = Str(r, "crashed"),
        };
        var cur = r.GetProperty("cursor");
        g.CursorCol = cur[0].GetInt32();
        g.CursorRow = cur[1].GetInt32();
        foreach (var e in r.GetProperty("runs").EnumerateArray())
            g.Runs.Add(new Run(e[0].GetInt32(), e[1].GetInt32(), e[2].GetInt32(), e[3].GetString() ?? "", e[4].GetUInt32(), e[5].GetUInt32(), e[6].GetInt32()));
        foreach (var e in r.GetProperty("pictures").EnumerateArray())
            g.Pictures.Add(new Picture(e.GetProperty("col").GetInt32(), e.GetProperty("row").GetInt32(), e.GetProperty("cols").GetInt32(), e.GetProperty("rows").GetInt32(), e.GetProperty("path").GetString() ?? ""));
        if (r.TryGetProperty("dialog", out var d) && d.ValueKind == JsonValueKind.Object)
            g.Dialog = new Dialog(d.GetProperty("id").GetUInt32(), d.GetProperty("kind").GetString() ?? "open_file", d.GetProperty("title").GetString() ?? "", d.GetProperty("start").GetString() ?? "", d.GetProperty("name").GetString() ?? "");
        return g;
    }

    private static string? Str(JsonElement r, string name) =>
        r.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.String ? v.GetString() : null;

    /// <summary>The same picture? Then there is nothing to redraw.</summary>
    public bool SameAs(Grid? o) =>
        o is not null && Cols == o.Cols && Rows == o.Rows && CursorCol == o.CursorCol && CursorRow == o.CursorRow &&
        Bg == o.Bg && Fg == o.Fg && Busy == o.Busy && Runs.SequenceEqual(o.Runs) && Pictures.SequenceEqual(o.Pictures);
}
