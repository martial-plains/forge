# Native drawing

A frontend can draw canvases (Bases cards and boards, the graph, plugin
drawings) itself instead of showing PNGs the editor rasterised. Drawing
natively follows the system's fonts, scales to any display without
re-rendering, and costs the editor no pixel work.

## Turning it on

Call `set_native_scenes(true)` on the editor (`forge_set_native_scenes` in
the C API) before the first frame. A canvas then arrives where a picture
did (`ImageFrame` / `GridPicture`), with a `path` ending in `.scene.json`.
A frontend that does not call it keeps getting PNGs.

The file is a **display list**: the scene with the view applied (pan and
zoom), the parts off screen left out, colour roles resolved for the
current theme, and web pictures swapped for their cached files. A new file
is written for every change, so the path is a cache key; the editor keeps
the last few files for a frame still showing one.

## The format

```
{"w":W,"h":H,"bg":"#rrggbbaa"|null,"ops":[Op...]}

{"t":"rect","x","y","w","h","r":radius,"f":Fill?,"s":Stroke?}
{"t":"ellipse","cx","cy","rx","ry","f","s"}
{"t":"path","c":[["M",x,y],["L",x,y],["C",x1,y1,x2,y2,x,y],["Z"]],"f","s"}
{"t":"text","x","y","size","color","bold","italic","align","lines":["..."]}
{"t":"image","path","x","y","w","h","fit":"stretch|cover|contain","o":opacity}
{"t":"group","m":[a,b,c,d,e,f],"clip":[x,y,w,h]?,"o":opacity,"ops":[Op...]}

Fill   {"c":"#rrggbbaa"} or {"g":[x1,y1,x2,y2],"stops":[[offset,"#rrggbbaa"]...]}
Stroke {"c":"#rrggbbaa","w":width,"dash":[on,off,...]}
```

* All colours are `#rrggbbaa`.
* A group's matrix maps its own coordinates to its parent's, as SVG's
  `matrix(a b c d e f)` does: `x' = a x + c y + e`, `y' = b x + d y + f`.
* A text line's top is `y + index * size * 1.3`; `x` is its left, centre or
  right edge by `align`. Lines are already wrapped and truncated.
* Lines, arrowheads, quadratic curves and relative path commands are gone:
  a line is a two-point path, an arrowhead a filled path, a quadratic a cubic.

Draw the scene scaled to fit the picture's box, centred vertically, with
the box's left edge as the scene's. The pointer geometry the editor uses
for clicks and drags assumes exactly that.

## The renderers

| Frontend | File | Draws with |
| --- | --- | --- |
| macOS | `frontends/apple/Forge/SceneRenderer.swift` | CoreGraphics, AppKit text |
| GNOME | `frontends/gnome/src/scene.rs` | Cairo, Pango |
| Windows | `frontends/winui/SceneDraw.cs` | Win2D |

The GNOME renderer was compiled against gtk4-rs and drew a test scene
(rounded rectangles, gradients, dashes, text, a clipped translucent group)
correctly. The others have not been compiled or run.
