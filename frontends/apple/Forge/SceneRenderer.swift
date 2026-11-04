// SPDX-License-Identifier: MIT OR Apache-2.0
import AppKit

/// Draws a canvas natively with CoreGraphics. The editor hands canvases
/// over as `.scene.json` files in the display-list format described in
/// `crates/canvas/src/native.rs` (colours are `#rrggbbaa`, paths are
/// M/L/C/Z, text is already broken into lines), so this file holds no
/// layout logic: it walks the list and issues drawing calls.
///
/// UNVERIFIED: written without a Swift compiler or a Mac to run on.
final class SceneRenderer {
    struct Parsed {
        let width: CGFloat
        let height: CGFloat
        let root: [String: Any]
    }

    static let marker = ".scene.json"

    private static var cache: [String: Parsed] = [:]
    private static var pictures: [String: NSImage] = [:]

    static func isScene(_ path: String) -> Bool { path.hasSuffix(marker) }

    /// The scene in `path`, parsed once (the editor writes a new file for
    /// every change, so the path is the cache key).
    static func load(_ path: String) -> Parsed? {
        if let hit = cache[path] { return hit }
        guard let data = try? Data(contentsOf: URL(fileURLWithPath: path)),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let w = (json["w"] as? NSNumber)?.doubleValue,
              let h = (json["h"] as? NSNumber)?.doubleValue
        else { return nil }
        if cache.count > 24 { cache.removeAll() }
        let parsed = Parsed(width: CGFloat(w), height: CGFloat(h), root: json)
        cache[path] = parsed
        return parsed
    }

    /// Draw `scene` into `box`, scaled to fit and centred vertically, the
    /// way a picture is laid over its rows. The context must have its y
    /// axis pointing down (the editor's view is flipped).
    static func draw(_ scene: Parsed, in box: NSRect, context ctx: CGContext) {
        guard scene.width > 0, scene.height > 0 else { return }
        let s = min(box.width / scene.width, box.height / scene.height)
        ctx.saveGState()
        ctx.clip(to: box)
        ctx.translateBy(x: box.minX, y: box.minY + (box.height - scene.height * s) / 2)
        ctx.scaleBy(x: s, y: s)
        if let bg = scene.root["bg"] as? String {
            ctx.setFillColor(color(bg))
            ctx.fill(CGRect(x: 0, y: 0, width: scene.width, height: scene.height))
        }
        ops(scene.root["ops"] as? [[String: Any]] ?? [], ctx)
        ctx.restoreGState()
    }

    // MARK: - values

    private static func num(_ v: Any?) -> CGFloat { CGFloat((v as? NSNumber)?.doubleValue ?? 0) }

    /// `#rrggbbaa`.
    static func color(_ hex: String) -> CGColor {
        let s = hex.dropFirst()
        guard s.count == 8, let v = UInt32(s, radix: 16) else { return CGColor(gray: 0, alpha: 1) }
        return CGColor(
            red: CGFloat(v >> 24 & 255) / 255, green: CGFloat(v >> 16 & 255) / 255,
            blue: CGFloat(v >> 8 & 255) / 255, alpha: CGFloat(v & 255) / 255)
    }

    private static func ops(_ list: [[String: Any]], _ ctx: CGContext) {
        for op in list {
            switch op["t"] as? String {
            case "rect":
                let r = CGRect(x: num(op["x"]), y: num(op["y"]), width: num(op["w"]), height: num(op["h"]))
                let radius = min(num(op["r"]), min(r.width, r.height) / 2)
                let path = radius > 0
                    ? CGPath(roundedRect: r, cornerWidth: radius, cornerHeight: radius, transform: nil)
                    : CGPath(rect: r, transform: nil)
                paint(path, op, ctx)
            case "ellipse":
                let cx = num(op["cx"]), cy = num(op["cy"]), rx = num(op["rx"]), ry = num(op["ry"])
                paint(CGPath(ellipseIn: CGRect(x: cx - rx, y: cy - ry, width: 2 * rx, height: 2 * ry),
                             transform: nil), op, ctx)
            case "path":
                paint(path(op["c"] as? [[Any]] ?? []), op, ctx)
            case "text":
                text(op, ctx)
            case "image":
                image(op, ctx)
            case "group":
                group(op, ctx)
            default:
                break
            }
        }
    }

    private static func path(_ cmds: [[Any]]) -> CGPath {
        let p = CGMutablePath()
        for c in cmds {
            guard let name = c.first as? String else { continue }
            let n = c.dropFirst().map { num($0) }
            switch name {
            case "M" where n.count >= 2: p.move(to: CGPoint(x: n[0], y: n[1]))
            case "L" where n.count >= 2: p.addLine(to: CGPoint(x: n[0], y: n[1]))
            case "C" where n.count >= 6:
                p.addCurve(to: CGPoint(x: n[4], y: n[5]),
                           control1: CGPoint(x: n[0], y: n[1]), control2: CGPoint(x: n[2], y: n[3]))
            case "Z": p.closeSubpath()
            default: break
            }
        }
        return p
    }

    private static func paint(_ path: CGPath, _ op: [String: Any], _ ctx: CGContext) {
        if let fill = op["f"] as? [String: Any] {
            if let c = fill["c"] as? String {
                ctx.addPath(path)
                ctx.setFillColor(color(c))
                ctx.fillPath()
            } else if let g = fill["g"] as? [NSNumber], g.count == 4,
                      let stops = fill["stops"] as? [[Any]] {
                let colors = stops.compactMap { ($0.last as? String).map(color) } as CFArray
                let locations = stops.map { num($0.first) }
                if let gradient = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(),
                                             colors: colors, locations: locations) {
                    ctx.saveGState()
                    ctx.addPath(path)
                    ctx.clip()
                    ctx.drawLinearGradient(
                        gradient,
                        start: CGPoint(x: g[0].doubleValue, y: g[1].doubleValue),
                        end: CGPoint(x: g[2].doubleValue, y: g[3].doubleValue),
                        options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])
                    ctx.restoreGState()
                }
            }
        }
        if let s = op["s"] as? [String: Any], let c = s["c"] as? String {
            ctx.addPath(path)
            ctx.setStrokeColor(color(c))
            ctx.setLineWidth(max(0.01, num(s["w"])))
            let dash = (s["dash"] as? [NSNumber])?.map { CGFloat($0.doubleValue) } ?? []
            ctx.setLineDash(phase: 0, lengths: dash)
            ctx.setLineJoin(.round)
            ctx.setLineCap(dash.isEmpty ? .butt : .round)
            ctx.strokePath()
            ctx.setLineDash(phase: 0, lengths: [])
        }
    }

    private static func text(_ op: [String: Any], _ ctx: CGContext) {
        let size = num(op["size"])
        let bold = (op["bold"] as? Bool) ?? false
        let italic = (op["italic"] as? Bool) ?? false
        var font = NSFont.systemFont(ofSize: size, weight: bold ? .bold : .regular)
        if italic {
            font = NSFontManager.shared.convert(font, toHaveTrait: .italicFontMask)
        }
        let ink = NSColor(cgColor: color(op["color"] as? String ?? "#000000ff")) ?? .black
        let attrs: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: ink]
        let x = num(op["x"]), y = num(op["y"])
        let align = op["align"] as? String ?? "left"
        let previous = NSGraphicsContext.current
        NSGraphicsContext.current = NSGraphicsContext(cgContext: ctx, flipped: true)
        for (i, line) in (op["lines"] as? [String] ?? []).enumerated() {
            let s = NSAttributedString(string: line, attributes: attrs)
            let width = s.size().width
            let left = align == "center" ? x - width / 2 : (align == "right" ? x - width : x)
            s.draw(at: NSPoint(x: left, y: y + CGFloat(i) * size * 1.3))
        }
        NSGraphicsContext.current = previous
    }

    private static func image(_ op: [String: Any], _ ctx: CGContext) {
        guard let path = op["path"] as? String else { return }
        let picture: NSImage
        if let cached = pictures[path] {
            picture = cached
        } else if let loaded = NSImage(contentsOfFile: path) {
            if pictures.count > 96 { pictures.removeAll() }
            pictures[path] = loaded
            picture = loaded
        } else {
            return
        }
        let box = CGRect(x: num(op["x"]), y: num(op["y"]), width: num(op["w"]), height: num(op["h"]))
        guard box.width > 0, box.height > 0, picture.size.width > 0, picture.size.height > 0 else { return }
        let fit = op["fit"] as? String ?? "stretch"
        let sx = box.width / picture.size.width, sy = box.height / picture.size.height
        let k = fit == "cover" ? max(sx, sy) : (fit == "contain" ? min(sx, sy) : 0)
        let target = fit == "stretch"
            ? box
            : CGRect(x: box.midX - picture.size.width * k / 2, y: box.midY - picture.size.height * k / 2,
                     width: picture.size.width * k, height: picture.size.height * k)
        ctx.saveGState()
        ctx.clip(to: box)
        let previous = NSGraphicsContext.current
        NSGraphicsContext.current = NSGraphicsContext(cgContext: ctx, flipped: true)
        picture.draw(in: target, from: .zero, operation: .sourceOver,
                     fraction: op["o"] == nil ? 1 : num(op["o"]), respectFlipped: true, hints: nil)
        NSGraphicsContext.current = previous
        ctx.restoreGState()
    }

    private static func group(_ op: [String: Any], _ ctx: CGContext) {
        let opacity = op["o"] == nil ? 1 : num(op["o"])
        ctx.saveGState()
        if let m = op["m"] as? [NSNumber], m.count == 6 {
            ctx.concatenate(CGAffineTransform(
                a: m[0].doubleValue, b: m[1].doubleValue, c: m[2].doubleValue,
                d: m[3].doubleValue, tx: m[4].doubleValue, ty: m[5].doubleValue))
        }
        if let c = op["clip"] as? [NSNumber], c.count == 4 {
            ctx.clip(to: CGRect(x: c[0].doubleValue, y: c[1].doubleValue,
                                width: c[2].doubleValue, height: c[3].doubleValue))
        }
        let list = op["ops"] as? [[String: Any]] ?? []
        if opacity < 1 {
            ctx.setAlpha(opacity)
            ctx.beginTransparencyLayer(auxiliaryInfo: nil)
            ops(list, ctx)
            ctx.endTransparencyLayer()
        } else {
            ops(list, ctx)
        }
        ctx.restoreGState()
    }
}
