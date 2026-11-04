// SPDX-License-Identifier: MIT OR Apache-2.0
using Microsoft.Graphics.Canvas;
using Microsoft.Graphics.Canvas.Brushes;
using Microsoft.Graphics.Canvas.Geometry;
using Microsoft.Graphics.Canvas.Text;
using System.Numerics;
using System.Text.Json;
using Windows.Foundation;
using Windows.UI;

namespace Forge;

/// <summary>
/// Draws a canvas (Bases cards and boards, the graph, drawings) with Win2D.
/// The editor hands canvases over as <c>.scene.json</c> files in the
/// display-list format described in crates/canvas/src/native.rs: colours are
/// <c>#rrggbbaa</c>, paths are M/L/C/Z and text is already broken into lines,
/// so there is no layout here, only drawing calls.
///
/// UNVERIFIED: written without a Windows machine or the Windows App SDK.
/// </summary>
internal static class SceneDraw
{
    public const string Suffix = ".scene.json";

    private sealed record Scene(float Width, float Height, Color? Bg, JsonElement Ops);

    private static readonly Dictionary<string, Scene> scenes = new();
    private static readonly Dictionary<string, CanvasBitmap?> pictures = new();

    public static bool IsScene(string path) => path.EndsWith(Suffix, StringComparison.Ordinal);

    private static Scene? Load(string path)
    {
        if (scenes.TryGetValue(path, out var hit)) return hit;
        try
        {
            // JsonDocument is kept alive: its elements are read at every draw.
            var doc = JsonDocument.Parse(File.ReadAllText(path));
            var root = doc.RootElement;
            Color? bg = root.TryGetProperty("bg", out var b) && b.ValueKind == JsonValueKind.String ? Rgba(b.GetString()!) : null;
            var scene = new Scene(root.GetProperty("w").GetSingle(), root.GetProperty("h").GetSingle(), bg, root.GetProperty("ops"));
            if (scenes.Count > 24) scenes.Clear();
            scenes[path] = scene;
            return scene;
        }
        catch { return null; }
    }

    /// <summary>#rrggbbaa.</summary>
    private static Color Rgba(string hex)
    {
        var v = Convert.ToUInt32(hex.TrimStart('#'), 16);
        return Color.FromArgb((byte)v, (byte)(v >> 24), (byte)(v >> 16), (byte)(v >> 8));
    }

    private static float F(JsonElement e, string k) => e.TryGetProperty(k, out var v) && v.ValueKind == JsonValueKind.Number ? v.GetSingle() : 0f;

    /// <summary>Draw the scene in <paramref name="path"/> into the box, scaled to fit and centred vertically.</summary>
    public static void Draw(ICanvasResourceCreator device, CanvasDrawingSession ds, string path, double x, double y, double w, double h)
    {
        if (Load(path) is not { } scene || scene.Width <= 0 || scene.Height <= 0) return;
        var s = (float)Math.Min(w / scene.Width, h / scene.Height);
        var saved = ds.Transform;
        using var clip = ds.CreateLayer(1f, new Rect(x, y, w, h));
        ds.Transform = Matrix3x2.CreateScale(s) * Matrix3x2.CreateTranslation((float)x, (float)(y + (h - scene.Height * s) / 2)) * saved;
        if (scene.Bg is { } bg) ds.FillRectangle(0, 0, scene.Width, scene.Height, bg);
        Ops(device, ds, scene.Ops);
        ds.Transform = saved;
    }

    private static void Ops(ICanvasResourceCreator device, CanvasDrawingSession ds, JsonElement list)
    {
        foreach (var op in list.EnumerateArray())
        {
            switch (op.GetProperty("t").GetString())
            {
                case "rect":
                {
                    float x = F(op, "x"), y = F(op, "y"), w = F(op, "w"), h = F(op, "h");
                    float r = Math.Min(F(op, "r"), Math.Min(w, h) / 2);
                    using var g = r > 0
                        ? CanvasGeometry.CreateRoundedRectangle(device, x, y, w, h, r, r)
                        : CanvasGeometry.CreateRectangle(device, x, y, w, h);
                    Paint(device, ds, g, op);
                    break;
                }
                case "ellipse":
                {
                    using var g = CanvasGeometry.CreateEllipse(device, F(op, "cx"), F(op, "cy"), F(op, "rx"), F(op, "ry"));
                    Paint(device, ds, g, op);
                    break;
                }
                case "path":
                {
                    using var g = Path(device, op.GetProperty("c"));
                    Paint(device, ds, g, op);
                    break;
                }
                case "text": Text(device, ds, op); break;
                case "image": Image(ds, op); break;
                case "group": Group(device, ds, op); break;
            }
        }
    }

    private static CanvasGeometry Path(ICanvasResourceCreator device, JsonElement cmds)
    {
        using var b = new CanvasPathBuilder(device);
        bool open = false;
        foreach (var c in cmds.EnumerateArray())
        {
            float N(int i) => c[i].GetSingle();
            switch (c[0].GetString())
            {
                case "M":
                    if (open) b.EndFigure(CanvasFigureLoop.Open);
                    b.BeginFigure(N(1), N(2));
                    open = true;
                    break;
                case "L": if (open) b.AddLine(N(1), N(2)); break;
                case "C": if (open) b.AddCubicBezier(new Vector2(N(1), N(2)), new Vector2(N(3), N(4)), new Vector2(N(5), N(6))); break;
                case "Z":
                    if (open) b.EndFigure(CanvasFigureLoop.Closed);
                    open = false;
                    break;
            }
        }
        if (open) b.EndFigure(CanvasFigureLoop.Open);
        return CanvasGeometry.CreatePath(b);
    }

    private static void Paint(ICanvasResourceCreator device, CanvasDrawingSession ds, CanvasGeometry g, JsonElement op)
    {
        if (op.TryGetProperty("f", out var fill) && fill.ValueKind == JsonValueKind.Object)
        {
            if (fill.TryGetProperty("c", out var c))
            {
                ds.FillGeometry(g, Rgba(c.GetString()!));
            }
            else if (fill.TryGetProperty("g", out var line))
            {
                var stops = fill.GetProperty("stops").EnumerateArray()
                    .Select(st => new CanvasGradientStop { Position = st[0].GetSingle(), Color = Rgba(st[1].GetString()!) })
                    .ToArray();
                using var brush = new CanvasLinearGradientBrush(device, stops)
                {
                    StartPoint = new Vector2(line[0].GetSingle(), line[1].GetSingle()),
                    EndPoint = new Vector2(line[2].GetSingle(), line[3].GetSingle()),
                };
                ds.FillGeometry(g, brush);
            }
        }
        if (op.TryGetProperty("s", out var stroke) && stroke.ValueKind == JsonValueKind.Object)
        {
            var style = new CanvasStrokeStyle { LineJoin = CanvasLineJoin.Round };
            if (stroke.TryGetProperty("dash", out var dash) && dash.GetArrayLength() >= 2)
            {
                // Win2D's custom dashes are in multiples of the stroke width.
                float w = Math.Max(0.01f, F(stroke, "w"));
                style.CustomDashStyle = dash.EnumerateArray().Select(d => d.GetSingle() / w).ToArray();
                style.DashCap = CanvasCapStyle.Round;
            }
            ds.DrawGeometry(g, Rgba(stroke.GetProperty("c").GetString()!), Math.Max(0.01f, F(stroke, "w")), style);
        }
    }

    private static void Text(ICanvasResourceCreator device, CanvasDrawingSession ds, JsonElement op)
    {
        float size = F(op, "size");
        var format = new CanvasTextFormat
        {
            FontSize = size,
            FontWeight = op.TryGetProperty("bold", out var b) && b.GetBoolean() ? Microsoft.UI.Text.FontWeights.Bold : Microsoft.UI.Text.FontWeights.Normal,
            FontStyle = op.TryGetProperty("italic", out var i) && i.GetBoolean() ? Windows.UI.Text.FontStyle.Italic : Windows.UI.Text.FontStyle.Normal,
            WordWrapping = CanvasWordWrapping.NoWrap,
        };
        var ink = Rgba(op.GetProperty("color").GetString()!);
        string align = op.TryGetProperty("align", out var a) ? a.GetString() ?? "left" : "left";
        int n = 0;
        foreach (var line in op.GetProperty("lines").EnumerateArray())
        {
            var text = line.GetString() ?? "";
            float x = F(op, "x"), y = F(op, "y") + n++ * size * 1.3f;
            if (align != "left")
            {
                using var layout = new CanvasTextLayout(device, text, format, 100000, size * 2);
                float width = (float)layout.LayoutBounds.Width;
                x -= align == "center" ? width / 2 : width;
            }
            ds.DrawText(text, x, y, ink, format);
        }
    }

    private static void Image(CanvasDrawingSession ds, JsonElement op)
    {
        var path = op.GetProperty("path").GetString() ?? "";
        if (!pictures.TryGetValue(path, out var bmp))
        {
            // Loaded here, on the drawing thread, the first time it is needed.
            try { bmp = CanvasBitmap.LoadAsync(ds, path).AsTask().GetAwaiter().GetResult(); }
            catch { bmp = null; }
            if (pictures.Count > 96) pictures.Clear();
            pictures[path] = bmp;
        }
        if (bmp is null) return;
        float x = F(op, "x"), y = F(op, "y"), w = F(op, "w"), h = F(op, "h");
        float bw = (float)bmp.Size.Width, bh = (float)bmp.Size.Height;
        float k = op.GetProperty("fit").GetString() switch
        {
            "cover" => Math.Max(w / bw, h / bh),
            "contain" => Math.Min(w / bw, h / bh),
            _ => 0f,
        };
        var target = k == 0f
            ? new Rect(x, y, w, h)
            : new Rect(x + w / 2 - bw * k / 2, y + h / 2 - bh * k / 2, bw * k, bh * k);
        using var layer = ds.CreateLayer(1f, new Rect(x, y, w, h));
        ds.DrawImage(bmp, target, new Rect(0, 0, bw, bh), F(op, "o") is var o and > 0 ? o : 1f);
    }

    private static void Group(ICanvasResourceCreator device, CanvasDrawingSession ds, JsonElement op)
    {
        var saved = ds.Transform;
        var m = op.GetProperty("m");
        ds.Transform = new Matrix3x2(m[0].GetSingle(), m[1].GetSingle(), m[2].GetSingle(), m[3].GetSingle(), m[4].GetSingle(), m[5].GetSingle()) * saved;
        float opacity = op.TryGetProperty("o", out var o) ? o.GetSingle() : 1f;
        CanvasActiveLayer? layer = null;
        if (op.TryGetProperty("clip", out var c) && c.ValueKind == JsonValueKind.Array)
        {
            layer = ds.CreateLayer(opacity, new Rect(c[0].GetSingle(), c[1].GetSingle(), c[2].GetSingle(), c[3].GetSingle()));
        }
        else if (opacity < 1f)
        {
            layer = ds.CreateLayer(opacity);
        }
        Ops(device, ds, op.GetProperty("ops"));
        layer?.Dispose();
        ds.Transform = saved;
    }
}
