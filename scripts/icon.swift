import AppKit

let output = CommandLine.arguments[1]
try FileManager.default.createDirectory(atPath: output, withIntermediateDirectories: true)
for (name, pixels) in [("icon_16x16",16),("icon_16x16@2x",32),("icon_32x32",32),("icon_32x32@2x",64),("icon_128x128",128),("icon_128x128@2x",256),("icon_256x256",256),("icon_256x256@2x",512),("icon_512x512",512),("icon_512x512@2x",1024)] {
    let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
    let context = NSGraphicsContext.current!.cgContext
    context.scaleBy(x: CGFloat(pixels)/1024, y: CGFloat(pixels)/1024)
    let rect = NSRect(x: 50, y: 50, width: 924, height: 924)
    let path = NSBezierPath(roundedRect: rect, xRadius: 210, yRadius: 210)
    NSGradient(starting: NSColor(srgbRed: 0.22, green: 0.58, blue: 0.41, alpha: 1), ending: NSColor(srgbRed: 0.04, green: 0.27, blue: 0.22, alpha: 1))!.draw(in: path, angle: -70)
    NSColor(white: 1, alpha: 0.16).setStroke()
    path.lineWidth = 3
    path.stroke()
    let envelope = NSBezierPath(roundedRect: NSRect(x: 221, y: 302, width: 582, height: 420), xRadius: 52, yRadius: 52)
    NSColor(srgbRed: 0.94, green: 0.98, blue: 0.95, alpha: 1).setFill()
    envelope.fill()
    NSColor(srgbRed: 0.13, green: 0.39, blue: 0.28, alpha: 1).setStroke()
    let flap = NSBezierPath()
    flap.move(to: NSPoint(x: 239, y: 682))
    flap.line(to: NSPoint(x: 480, y: 487))
    flap.curve(to: NSPoint(x: 544, y: 487), controlPoint1: NSPoint(x: 500, y: 471), controlPoint2: NSPoint(x: 524, y: 471))
    flap.line(to: NSPoint(x: 785, y: 682))
    flap.lineWidth = 27
    flap.lineCapStyle = .round
    flap.lineJoinStyle = .round
    flap.stroke()
    NSGraphicsContext.restoreGraphicsState()
    try bitmap.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: "\(output)/\(name).png"))
}
