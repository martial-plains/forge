// SPDX-License-Identifier: MIT OR Apache-2.0
//
//  MetalGrid.swift
//  Forge
//
//  The GPU side of drawing a frame. EditorView paints through `GridCanvas`;
//  `MetalCanvas` collects what it paints as quads (colored rectangles and
//  glyphs from a texture atlas) and `MetalGridRenderer` draws them in one
//  call, only when something changed. That is what saves battery: nothing
//  redraws while the editor is idle, and a redraw is one GPU pass instead of
//  a CPU pass over every cell.
//
//  Turn it off with `defaults write <bundle id> ForgeGPURendering -bool NO`
//  to go back to drawing with AppKit.
//

import AppKit
import CoreText
import Darwin
import Metal
import MetalKit
import QuartzCore

// MARK: - Canvas

/// Where a frame is painted. `EditorNSView.paint` calls these; AppKit
/// draws them directly, Metal collects them.
@MainActor
protocol GridCanvas: AnyObject {
    func fill(_ rect: NSRect, _ color: NSColor)
    /// A one-point outline just inside `rect`.
    func outline(_ rect: NSRect, _ color: NSColor)
    /// `string` laid out on the cell grid, its first cell's top-left at `origin`.
    /// `heightPct` scales the font (100 is normal) and `raisePct` lifts the
    /// text by that percentage of its own height; the baseline stays put.
    func text(_ string: String, at origin: NSPoint, style: FaceStyle, heightPct: Int, raisePct: Int)
    func image(_ frame: ImageFrame, x: Int, width: Int, y: Int)
    /// The cells of the window being drawn are this many times their normal
    /// size (its buffer's text scale), counted from `gridOrigin`, the
    /// window's top-left cell. 1 and (0, 0) when nothing is scaled.
    var gridScale: CGFloat { get set }
    var gridOrigin: (x: Int, y: Int) { get set }
    /// Clip to `rect`, then shift everything drawn up by `translateY`.
    func pushClip(_ rect: NSRect, translateY: CGFloat)
    func popClip()
}

extension GridCanvas {
    func text(_ string: String, at origin: NSPoint, style: FaceStyle) {
        text(string, at: origin, style: style, heightPct: 100, raisePct: 0)
    }
}

extension NSFont {
    /// The same font at `pct` percent of its size.
    func scaled(by pct: Int) -> NSFont {
        guard pct != 100 else { return self }
        return NSFont(descriptor: fontDescriptor, size: pointSize * CGFloat(pct) / 100) ?? self
    }
}

/// The fonts and cell size a canvas lays text out with.
struct GridMetrics {
    var font: NSFont
    var boldFont: NSFont
    var italicFont: NSFont
    var cellSize: CGSize
    /// Device pixels per point.
    var scale: CGFloat

    var ascender: CGFloat { font.ascender }
    var descender: CGFloat { font.descender }

    func variant(for style: FaceStyle) -> UInt8 {
        style.bold ? 1 : (style.italic ? 2 : 0)
    }
}

/// A picture the editor asked for, drawn by `ImageOverlayView` over the
/// Metal layer.
struct QueuedImage {
    let frame: ImageFrame
    let x: Int
    let width: Int
    let y: Int
    let clip: NSRect?
    let dy: CGFloat
    /// The window's cell scale and top-left cell when it was queued.
    let scale: CGFloat
    let origin: (x: Int, y: Int)
}

// MARK: - Vertices

/// One corner of a quad. Layout matches `GridVertex` in the shader: 48 bytes.
struct GridVertex {
    var position: SIMD2<Float>
    var uv: SIMD2<Float>
    var color: SIMD4<Float>
    /// 0: a flat color. 1: a glyph from the mask atlas, tinted. 2: a color
    /// glyph (emoji) from the color atlas.
    var kind: Float
}

private struct Uniforms {
    var viewport: SIMD2<Float>
}

// MARK: - Glyph atlas

/// Hands out space on a texture in rows.
private struct ShelfPacker {
    let width: Int
    let height: Int
    var x = 0
    var y = 0
    var shelf = 0

    init(width: Int, height: Int) {
        self.width = width
        self.height = height
    }

    mutating func reset() {
        x = 0
        y = 0
        shelf = 0
    }

    /// Room for a `w` by `h` image, or nil when the texture is full. A
    /// pixel is left between images.
    mutating func allocate(_ w: Int, _ h: Int) -> (x: Int, y: Int)? {
        let w = w + 1
        let h = h + 1
        if w > width || h > height { return nil }
        if x + w > width {
            x = 0
            y += shelf
            shelf = 0
        }
        if y + h > height { return nil }
        let at = (x: x, y: y)
        x += w
        shelf = max(shelf, h)
        return at
    }
}

/// Glyphs drawn once with Core Text and kept on the GPU. Ordinary glyphs are
/// coverage masks tinted by the shader; color glyphs (emoji) keep their colors.
@MainActor
final class GlyphAtlas {
    struct Key: Hashable {
        let text: String
        let variant: UInt8
        let cells: UInt8
        let heightPct: UInt16
    }

    struct Glyph {
        let u0: Float
        let v0: Float
        let u1: Float
        let v1: Float
        /// The quad's size in points, padding included.
        let size: CGSize
        /// How far the quad starts before and above the glyph's cell.
        let pad: CGPoint
        let isColor: Bool
    }

    private static let maskSide = 2048
    private static let colorSide = 1024
    private static let padX: CGFloat = 2
    private static let padY: CGFloat = 1

    let maskTexture: MTLTexture
    let colorTexture: MTLTexture
    private(set) var scale: CGFloat
    private var glyphs: [Key: Glyph] = [:]
    private var maskPacker = ShelfPacker(width: maskSide, height: maskSide)
    private var colorPacker = ShelfPacker(width: colorSide, height: colorSide)

    init?(device: MTLDevice, scale: CGFloat) {
        let mask = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .r8Unorm, width: Self.maskSide, height: Self.maskSide, mipmapped: false)
        mask.usage = .shaderRead
        let color = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .bgra8Unorm, width: Self.colorSide, height: Self.colorSide, mipmapped: false)
        color.usage = .shaderRead
        guard let m = device.makeTexture(descriptor: mask),
              let c = device.makeTexture(descriptor: color)
        else { return nil }
        maskTexture = m
        colorTexture = c
        self.scale = max(1, scale)
    }

    /// Forget every glyph (the atlas is full, or the screen's scale changed).
    func reset(scale newScale: CGFloat? = nil) {
        if let newScale { scale = max(1, newScale) }
        glyphs.removeAll(keepingCapacity: true)
        maskPacker.reset()
        colorPacker.reset()
    }

    /// The glyph for `text`, drawn on first use; nil when the atlas is full.
    func glyph(
        _ text: String, variant: UInt8, cells: Int, heightPct: Int = 100, metrics: GridMetrics
    ) -> Glyph? {
        let key = Key(
            text: text, variant: variant, cells: UInt8(clamping: cells),
            heightPct: UInt16(clamping: heightPct))
        if let g = glyphs[key] { return g }
        let bitmap = rasterize(
            text, variant: variant, cells: cells, heightPct: heightPct, metrics: metrics)
        let side = bitmap.isColor ? Self.colorSide : Self.maskSide
        let spot = bitmap.isColor
            ? colorPacker.allocate(bitmap.width, bitmap.height)
            : maskPacker.allocate(bitmap.width, bitmap.height)
        guard let spot else { return nil }
        let texture = bitmap.isColor ? colorTexture : maskTexture
        let bytesPerPixel = bitmap.isColor ? 4 : 1
        bitmap.pixels.withUnsafeBytes { raw in
            guard let base = raw.baseAddress else { return }
            texture.replace(
                region: MTLRegionMake2D(spot.x, spot.y, bitmap.width, bitmap.height),
                mipmapLevel: 0,
                withBytes: base,
                bytesPerRow: bitmap.width * bytesPerPixel)
        }
        let s = Float(side)
        let glyph = Glyph(
            u0: Float(spot.x) / s,
            v0: Float(spot.y) / s,
            u1: Float(spot.x + bitmap.width) / s,
            v1: Float(spot.y + bitmap.height) / s,
            size: CGSize(width: CGFloat(bitmap.width) / scale, height: CGFloat(bitmap.height) / scale),
            pad: CGPoint(x: Self.padX, y: Self.padY + bitmap.extraUp),
            isColor: bitmap.isColor)
        glyphs[key] = glyph
        return glyph
    }

    private struct Bitmap {
        var width: Int
        var height: Int
        var pixels: [UInt8]
        var isColor: Bool
        /// Room above the cell, for text taller than it.
        var extraUp: CGFloat
    }

    private func rasterize(
        _ text: String, variant: UInt8, cells: Int, heightPct: Int, metrics: GridMetrics
    ) -> Bitmap {
        // Text taller than the cell grows past it, up and down from the baseline.
        let factor = max(1, CGFloat(heightPct) / 100)
        let extraUp = max(0, metrics.ascender * (factor - 1))
        let extraDown = max(0, -metrics.descender * (factor - 1))
        let widthPt = CGFloat(cells) * metrics.cellSize.width * factor + 2 * Self.padX
        let heightPt = metrics.cellSize.height + extraUp + extraDown + 2 * Self.padY
        let w = max(1, Int(ceil(widthPt * scale)))
        let h = max(1, Int(ceil(heightPt * scale)))
        let base = variant == 1 ? metrics.boldFont : (variant == 2 ? metrics.italicFont : metrics.font)
        let font = base.scaled(by: heightPct)
        let attributed = NSAttributedString(
            string: text,
            attributes: [
                NSAttributedString.Key(kCTFontAttributeName as String): font as CTFont,
                // Draw in the context's fill color (white), the coverage mask.
                NSAttributedString.Key(kCTForegroundColorFromContextAttributeName as String): true,
            ])
        let line = CTLineCreateWithAttributedString(attributed)

        // Emoji and other color fonts keep their own colors.
        var isColor = false
        let runs = CTLineGetGlyphRuns(line) as NSArray
        for case let run as CTRun in runs {
            let attrs = CTRunGetAttributes(run) as NSDictionary
            if let value = attrs[kCTFontAttributeName as NSString],
               CFGetTypeID(value as CFTypeRef) == CTFontGetTypeID()
            {
                let runFont = value as! CTFont
                if CTFontGetSymbolicTraits(runFont).contains(.traitColorGlyphs) {
                    isColor = true
                }
            }
        }

        let bytesPerPixel = isColor ? 4 : 1
        var pixels = [UInt8](repeating: 0, count: w * h * bytesPerPixel)
        let space = isColor ? CGColorSpaceCreateDeviceRGB() : CGColorSpaceCreateDeviceGray()
        let info: UInt32 = isColor
            ? CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue
            : CGImageAlphaInfo.none.rawValue
        let drawScale = scale
        pixels.withUnsafeMutableBytes { raw in
            guard let ctx = CGContext(
                data: raw.baseAddress, width: w, height: h, bitsPerComponent: 8,
                bytesPerRow: w * bytesPerPixel, space: space, bitmapInfo: info)
            else { return }
            ctx.scaleBy(x: drawScale, y: drawScale)
            ctx.setAllowsAntialiasing(true)
            ctx.setShouldAntialias(true)
            ctx.setAllowsFontSmoothing(true)
            ctx.setShouldSmoothFonts(true)
            ctx.setFillColor(CGColor(gray: 1, alpha: 1))
            ctx.setTextDrawingMode(.fill)
            // The cell's top sits padY below the bitmap's top, and the
            // baseline one ascender below that, as AppKit draws a line.
            let top = CGFloat(h) / drawScale - Self.padY - extraUp
            ctx.textPosition = CGPoint(x: Self.padX, y: top - metrics.ascender)
            CTLineDraw(line, ctx)
        }
        return Bitmap(width: w, height: h, pixels: pixels, isColor: isColor, extraUp: extraUp)
    }
}

// MARK: - Metal canvas

@MainActor
final class MetalCanvas: GridCanvas {
    private(set) var vertices: [GridVertex] = []
    private(set) var queuedImages: [QueuedImage] = []
    /// The atlas ran out of room: reset it and paint again.
    private(set) var overflowed = false

    private let atlas: GlyphAtlas
    private let metrics: GridMetrics
    private var clip: NSRect?
    private var dy: CGFloat = 0
    private var saved: [(clip: NSRect?, dy: CGFloat)] = []
    var gridScale: CGFloat = 1
    var gridOrigin: (x: Int, y: Int) = (0, 0)

    init(atlas: GlyphAtlas, metrics: GridMetrics) {
        self.atlas = atlas
        self.metrics = metrics
        vertices.reserveCapacity(24_000)
    }

    /// A color as sRGB components, resolved for the appearance current now.
    func resolve(_ color: NSColor) -> SIMD4<Float> {
        let c = color.usingColorSpace(.sRGB) ?? color.usingColorSpace(.deviceRGB) ?? NSColor.black
        return SIMD4(Float(c.redComponent), Float(c.greenComponent), Float(c.blueComponent),
                     Float(c.alphaComponent))
    }

    private func snap(_ v: CGFloat) -> CGFloat {
        (v * metrics.scale).rounded() / metrics.scale
    }

    func pushClip(_ rect: NSRect, translateY: CGFloat) {
        saved.append((clip, dy))
        clip = rect
        dy = translateY
    }

    func popClip() {
        guard let last = saved.popLast() else { return }
        clip = last.clip
        dy = last.dy
    }

    func fill(_ rect: NSRect, _ color: NSColor) {
        addQuad(rect, uv: nil, color: resolve(color), kind: 0, snapSize: true)
    }

    func outline(_ rect: NSRect, _ color: NSColor) {
        let c = resolve(color)
        let t: CGFloat = 1
        let r = rect
        for edge in [
            NSRect(x: r.minX, y: r.minY, width: r.width, height: t),
            NSRect(x: r.minX, y: r.maxY - t, width: r.width, height: t),
            NSRect(x: r.minX, y: r.minY + t, width: t, height: max(0, r.height - 2 * t)),
            NSRect(x: r.maxX - t, y: r.minY + t, width: t, height: max(0, r.height - 2 * t)),
        ] {
            addQuad(edge, uv: nil, color: c, kind: 0, snapSize: true)
        }
    }

    func image(_ frame: ImageFrame, x: Int, width: Int, y: Int) {
        queuedImages.append(QueuedImage(
            frame: frame, x: x, width: width, y: y, clip: clip, dy: dy,
            scale: gridScale, origin: gridOrigin))
    }

    func text(_ string: String, at origin: NSPoint, style: FaceStyle, heightPct baseHeightPct: Int, raisePct: Int) {
        // A scaled window's cells, and the font with them.
        let heightPct = Int((Double(baseHeightPct) * Double(gridScale)).rounded())
        let cw = metrics.cellSize.width * gridScale
        let lift = CGFloat(raisePct) / 100 * metrics.font.pointSize * CGFloat(heightPct) / 100
        let fg = resolve(style.foreground)
        let variant = metrics.variant(for: style)
        var col = 0
        for character in string {
            let cells = Self.cells(of: character)
            if !character.isWhitespace {
                let cluster = String(character)
                if let g = atlas.glyph(
                    cluster, variant: variant, cells: cells, heightPct: heightPct, metrics: metrics)
                {
                    let rect = NSRect(
                        x: snap(origin.x + CGFloat(col) * cw - g.pad.x),
                        y: snap(origin.y - g.pad.y - lift),
                        width: g.size.width, height: g.size.height)
                    addQuad(rect, uv: (g.u0, g.v0, g.u1, g.v1), color: fg,
                            kind: g.isColor ? 2 : 1, snapSize: false)
                } else {
                    overflowed = true
                    return
                }
            }
            col += cells
        }
        let width = CGFloat(col) * cw
        if let underline = style.underline, width > 0 {
            let y = origin.y + metrics.ascender + max(1, -metrics.descender * 0.35)
            fill(NSRect(x: origin.x, y: y, width: width, height: 1), underline)
        }
        if style.strike, width > 0 {
            let y = origin.y + metrics.ascender - metrics.font.pointSize * 0.3
            fill(NSRect(x: origin.x, y: y, width: width, height: 1), style.foreground)
        }
    }

    /// How many cells a character takes: two for wide (CJK, emoji), else one.
    static func cells(of character: Character) -> Int {
        let scalars = character.unicodeScalars
        guard let first = scalars.first else { return 1 }
        if scalars.contains(where: { $0.value == 0xFE0F }) { return 2 }
        if first.value < 0x20 || first.value == 0x7F { return 1 }
        return wcwidth(wchar_t(first.value)) >= 2 ? 2 : 1
    }

    private func addQuad(
        _ rect: NSRect, uv: (Float, Float, Float, Float)?, color: SIMD4<Float>, kind: Float,
        snapSize: Bool
    ) {
        var r = rect.offsetBy(dx: 0, dy: -dy)
        var coords = uv
        if let clip {
            let i = r.intersection(clip)
            if i.isNull || i.width <= 0 || i.height <= 0 { return }
            if let c = coords, r.width > 0, r.height > 0 {
                let fx0 = Float((i.minX - r.minX) / r.width)
                let fx1 = Float((i.maxX - r.minX) / r.width)
                let fy0 = Float((i.minY - r.minY) / r.height)
                let fy1 = Float((i.maxY - r.minY) / r.height)
                coords = (
                    c.0 + (c.2 - c.0) * fx0, c.1 + (c.3 - c.1) * fy0,
                    c.0 + (c.2 - c.0) * fx1, c.1 + (c.3 - c.1) * fy1)
            }
            r = i
        }
        var x0 = r.minX, y0 = r.minY, x1 = r.maxX, y1 = r.maxY
        if snapSize {
            x0 = snap(x0)
            y0 = snap(y0)
            x1 = snap(x1)
            y1 = snap(y1)
            let pixel = 1 / metrics.scale
            if x1 <= x0 { x1 = x0 + pixel }
            if y1 <= y0 { y1 = y0 + pixel }
        }
        let u0 = coords?.0 ?? 0, v0 = coords?.1 ?? 0, u1 = coords?.2 ?? 0, v1 = coords?.3 ?? 0
        func vertex(_ x: CGFloat, _ y: CGFloat, _ u: Float, _ v: Float) -> GridVertex {
            GridVertex(position: SIMD2(Float(x), Float(y)), uv: SIMD2(u, v), color: color, kind: kind)
        }
        let a = vertex(x0, y0, u0, v0)
        let b = vertex(x1, y0, u1, v0)
        let c = vertex(x0, y1, u0, v1)
        let d = vertex(x1, y1, u1, v1)
        vertices.append(contentsOf: [a, b, c, b, d, c])
    }
}

// MARK: - Views

/// The Metal layer under the grid. Clicks and keys go to the editor view.
final class GridMTKView: MTKView {
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
    override var acceptsFirstResponder: Bool { false }
}

/// Pictures over the Metal layer (rarely repainted, so drawn with AppKit).
@MainActor
final class ImageOverlayView: NSView {
    weak var owner: EditorNSView?
    private var items: [QueuedImage] = []

    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    func show(_ new: [QueuedImage]) {
        if items.isEmpty && new.isEmpty { return }
        items = new
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        owner?.paintQueuedImages(items)
    }
}

/// MTKView calls its delegate from the main thread, whatever the SDK says
/// about isolation.
final class GridViewDelegate: NSObject, MTKViewDelegate {
    var onDraw: (@MainActor () -> Void)?

    /// A drawable of a new size is blank until it is drawn into.
    nonisolated func mtkView(_ view: MTKView, drawableSizeWillChange size: CGSize) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated { view.needsDisplay = true }
        }
    }

    nonisolated func draw(in view: MTKView) {
        MainActor.assumeIsolated { self.onDraw?() }
    }
}

// MARK: - Renderer

@MainActor
final class MetalGridRenderer {
    let atlas: GlyphAtlas
    let view: GridMTKView
    private let device: MTLDevice
    private let queue: MTLCommandQueue
    private let pipeline: MTLRenderPipelineState
    private let sampler: MTLSamplerState
    private let delegate = GridViewDelegate()
    private var drawCount = 0

    var onDraw: (@MainActor () -> Void)? {
        get { delegate.onDraw }
        set { delegate.onDraw = newValue }
    }

    init?(scale: CGFloat) {
        guard let device = MTLCreateSystemDefaultDevice(),
              let queue = device.makeCommandQueue(),
              let atlas = GlyphAtlas(device: device, scale: scale)
        else { return nil }
        let library: MTLLibrary
        do {
            library = try device.makeLibrary(source: Self.shaderSource, options: nil)
        } catch {
            NSLog("Forge: Metal shaders did not compile: \(error)")
            return nil
        }
        let descriptor = MTLRenderPipelineDescriptor()
        descriptor.vertexFunction = library.makeFunction(name: "grid_vertex")
        descriptor.fragmentFunction = library.makeFunction(name: "grid_fragment")
        let attachment = descriptor.colorAttachments[0]!
        attachment.pixelFormat = .bgra8Unorm
        // Colors are premultiplied by the fragment shader.
        attachment.isBlendingEnabled = true
        attachment.rgbBlendOperation = .add
        attachment.alphaBlendOperation = .add
        attachment.sourceRGBBlendFactor = .one
        attachment.sourceAlphaBlendFactor = .one
        attachment.destinationRGBBlendFactor = .oneMinusSourceAlpha
        attachment.destinationAlphaBlendFactor = .oneMinusSourceAlpha
        guard let pipeline = try? device.makeRenderPipelineState(descriptor: descriptor) else {
            return nil
        }
        let samplerDescriptor = MTLSamplerDescriptor()
        samplerDescriptor.minFilter = .nearest
        samplerDescriptor.magFilter = .nearest
        samplerDescriptor.sAddressMode = .clampToEdge
        samplerDescriptor.tAddressMode = .clampToEdge
        guard let sampler = device.makeSamplerState(descriptor: samplerDescriptor) else {
            return nil
        }

        self.device = device
        self.queue = queue
        self.atlas = atlas
        self.pipeline = pipeline
        self.sampler = sampler
        let view = GridMTKView(frame: .zero, device: device)
        view.colorPixelFormat = .bgra8Unorm
        view.framebufferOnly = true
        // Draw only when asked (`needsDisplay`): an idle editor costs nothing.
        view.isPaused = true
        view.enableSetNeedsDisplay = true
        view.autoResizeDrawable = true
        (view.layer as? CAMetalLayer)?.colorspace = CGColorSpace(name: CGColorSpace.sRGB)
        view.delegate = delegate
        self.view = view
    }

    /// Draw `canvas` over a background of `clear`.
    func present(_ canvas: MetalCanvas, clear: SIMD4<Float>) {
        guard let drawable = view.currentDrawable,
              let pass = view.currentRenderPassDescriptor,
              let commands = queue.makeCommandBuffer()
        else {
            NSLog("Forge GPU: no drawable (view \(view.bounds.size), drawable \(view.drawableSize))")
            return
        }
        if drawCount < 3 {
            drawCount += 1
            NSLog("Forge GPU: draw \(drawCount), \(canvas.vertices.count / 6) quads, view \(view.bounds.size)")
        }
        pass.colorAttachments[0].loadAction = .clear
        pass.colorAttachments[0].storeAction = .store
        pass.colorAttachments[0].clearColor = MTLClearColor(
            red: Double(clear.x), green: Double(clear.y), blue: Double(clear.z), alpha: 1)
        guard let encoder = commands.makeRenderCommandEncoder(descriptor: pass) else { return }
        let vertices = canvas.vertices
        if !vertices.isEmpty {
            let length = vertices.count * MemoryLayout<GridVertex>.stride
            let buffer = vertices.withUnsafeBytes { raw in
                device.makeBuffer(bytes: raw.baseAddress!, length: length, options: .storageModeShared)
            }
            if let buffer {
                var uniforms = Uniforms(
                    viewport: SIMD2(Float(view.bounds.width), Float(view.bounds.height)))
                encoder.setRenderPipelineState(pipeline)
                encoder.setVertexBuffer(buffer, offset: 0, index: 0)
                encoder.setVertexBytes(&uniforms, length: MemoryLayout<Uniforms>.stride, index: 1)
                encoder.setFragmentTexture(atlas.maskTexture, index: 0)
                encoder.setFragmentTexture(atlas.colorTexture, index: 1)
                encoder.setFragmentSamplerState(sampler, index: 0)
                encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: vertices.count)
            }
        }
        encoder.endEncoding()
        commands.present(drawable)
        commands.commit()
    }

    private static let shaderSource = """
    #include <metal_stdlib>
    using namespace metal;

    struct GridVertex {
        float2 position;
        float2 uv;
        float4 color;
        float kind;
    };

    struct Uniforms {
        float2 viewport;
    };

    struct VertexOut {
        float4 position [[position]];
        float2 uv;
        float4 color;
        float kind [[flat]];
    };

    vertex VertexOut grid_vertex(uint vid [[vertex_id]],
                                 constant GridVertex *vertices [[buffer(0)]],
                                 constant Uniforms &u [[buffer(1)]]) {
        GridVertex v = vertices[vid];
        VertexOut out;
        out.position = float4(v.position.x / u.viewport.x * 2.0 - 1.0,
                              1.0 - v.position.y / u.viewport.y * 2.0,
                              0.0, 1.0);
        out.uv = v.uv;
        out.color = v.color;
        out.kind = v.kind;
        return out;
    }

    fragment float4 grid_fragment(VertexOut in [[stage_in]],
                                  texture2d<float> maskTexture [[texture(0)]],
                                  texture2d<float> colorTexture [[texture(1)]],
                                  sampler samp [[sampler(0)]]) {
        if (in.kind < 0.5) {
            return float4(in.color.rgb * in.color.a, in.color.a);
        }
        if (in.kind < 1.5) {
            // A touch of boost: grayscale coverage looks thinner than the
            // smoothing AppKit applies.
            float coverage = pow(maskTexture.sample(samp, in.uv).r, 0.85);
            float a = coverage * in.color.a;
            return float4(in.color.rgb * a, a);
        }
        return colorTexture.sample(samp, in.uv) * in.color.a;
    }
    """
}
