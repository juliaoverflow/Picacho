import AppKit

// Draws the Picacho logo (assets/brand/picacho.svg) on a light app-icon tile.
let output = CommandLine.arguments[1]
try FileManager.default.createDirectory(atPath: output, withIntermediateDirectories: true)

func color(_ hex: UInt32) -> CGColor {
    CGColor(srgbRed: CGFloat((hex >> 16) & 0xff) / 255, green: CGFloat((hex >> 8) & 0xff) / 255, blue: CGFloat(hex & 0xff) / 255, alpha: 1)
}
func gradient(_ stops: [(CGFloat, UInt32)]) -> CGGradient {
    CGGradient(colorsSpace: CGColorSpace(name: CGColorSpace.sRGB), colors: stops.map { color($0.1) } as CFArray, locations: stops.map { $0.0 })!
}
func polygon(_ points: [(CGFloat, CGFloat)]) -> CGPath {
    let path = CGMutablePath()
    path.addLines(between: points.map { CGPoint(x: $0.0, y: $0.1) })
    path.closeSubpath()
    return path
}
// Paths in the SVG's 640×540 coordinate space, y pointing down.
let peak = CGMutablePath()
peak.move(to: CGPoint(x: 40, y: 396))
peak.addLine(to: CGPoint(x: 40, y: 352))
peak.addLine(to: CGPoint(x: 222, y: 201))
peak.addQuadCurve(to: CGPoint(x: 235, y: 200), control: CGPoint(x: 228, y: 196))
peak.addLine(to: CGPoint(x: 258, y: 214))
peak.addLine(to: CGPoint(x: 318, y: 88))
peak.addQuadCurve(to: CGPoint(x: 344, y: 86), control: CGPoint(x: 330, y: 64))
peak.addLine(to: CGPoint(x: 600, y: 352))
peak.addLine(to: CGPoint(x: 600, y: 396))
peak.addQuadCurve(to: CGPoint(x: 592, y: 397), control: CGPoint(x: 600, y: 403))
peak.addCurve(to: CGPoint(x: 432, y: 327), control1: CGPoint(x: 550, y: 362), control2: CGPoint(x: 493, y: 339))
peak.addQuadCurve(to: CGPoint(x: 208, y: 327), control: CGPoint(x: 320, y: 306))
peak.addCurve(to: CGPoint(x: 48, y: 397), control1: CGPoint(x: 147, y: 339), control2: CGPoint(x: 90, y: 362))
peak.addQuadCurve(to: CGPoint(x: 40, y: 396), control: CGPoint(x: 40, y: 403))
peak.closeSubpath()
// The envelope flap is the gap between these two shapes.
let halves = CGMutablePath()
halves.addPath(polygon([(0, 0), (640, 0), (640, 196), (320, 262), (0, 196)]))
halves.addPath(polygon([(0, 216), (320, 282), (640, 216), (640, 540), (0, 540)]))

for (name, pixels) in [("icon_16x16",16),("icon_16x16@2x",32),("icon_32x32",32),("icon_32x32@2x",64),("icon_128x128",128),("icon_128x128@2x",256),("icon_256x256",256),("icon_256x256@2x",512),("icon_512x512",512),("icon_512x512@2x",1024)] {
    let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
    let context = NSGraphicsContext.current!.cgContext
    context.scaleBy(x: CGFloat(pixels) / 1024, y: CGFloat(pixels) / 1024)
    // Tile, with the inset and corner radius of macOS app icons.
    let tile = CGPath(roundedRect: CGRect(x: 100, y: 100, width: 824, height: 824), cornerWidth: 185, cornerHeight: 185, transform: nil)
    context.saveGState()
    context.setShadow(offset: CGSize(width: 0, height: -10), blur: 24, color: CGColor(gray: 0, alpha: 0.28))
    context.addPath(tile)
    context.setFillColor(color(0xffffff))
    context.fillPath()
    context.restoreGState()
    context.saveGState()
    context.addPath(tile)
    context.clip()
    context.drawLinearGradient(gradient([(0, 0xffffff), (1, 0xe6f2ea)]), start: CGPoint(x: 0, y: 924), end: CGPoint(x: 0, y: 100), options: [])
    context.restoreGState()
    // Logo: flip to SVG coordinates and fit 600 units wide, centered.
    context.translateBy(x: 0, y: 1024)
    context.scaleBy(x: 1, y: -1)
    let scale: CGFloat = 1.0
    context.translateBy(x: 512 - 320 * scale, y: 512 - 292 * scale)
    context.scaleBy(x: scale, y: scale)
    context.saveGState()
    context.addPath(peak)
    context.clip()
    context.addPath(halves)
    context.clip()
    context.drawLinearGradient(gradient([(0, 0x35cd58), (0.55, 0x18b965), (1, 0x008d77)]), start: CGPoint(x: 40, y: 60), end: CGPoint(x: 600, y: 400), options: [])
    context.restoreGState()
    context.saveGState()
    context.addEllipse(in: CGRect(x: 40, y: 358, width: 560, height: 284))
    context.clip()
    context.clip(to: CGRect(x: 0, y: 0, width: 640, height: 500))
    context.drawLinearGradient(gradient([(0, 0x003d40), (1, 0x005353)]), start: CGPoint(x: 40, y: 0), end: CGPoint(x: 600, y: 0), options: [])
    context.restoreGState()
    NSGraphicsContext.restoreGraphicsState()
    try bitmap.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: "\(output)/\(name).png"))
}
