import AppKit

/// `--snapshot <file.png>`: wait for the window to draw, write a PNG of it and
/// quit.
///
/// Glass and materials are composited by the window server, so drawing the
/// view tree into a bitmap (`cacheDisplay`) leaves them out. The picture is
/// taken with `CGWindowListCreateImage` of this app's own window instead,
/// which needs no screen-recording permission. The Swift overlay hides that
/// call, so it is looked up by name; if that fails, the flat `cacheDisplay`
/// picture is written.
@MainActor
enum Snapshot {
    static func schedule(to path: String) {
        DispatchQueue.main.asyncAfter(deadline: .now() + 2.5) {
            let ok = write(to: path)
            FileHandle.standardError.write(Data((ok ? "snapshot: wrote \(path)\n" : "snapshot: could not write \(path)\n").utf8))
            exit(ok ? 0 : 1)
        }
    }

    private static func write(to path: String) -> Bool {
        guard let window = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil }) else { return false }
        let bitmap: NSBitmapImageRep
        if let image = composited(window) {
            bitmap = NSBitmapImageRep(cgImage: image)
        } else if let view = window.contentView?.superview ?? window.contentView,
                  let flat = view.bitmapImageRepForCachingDisplay(in: view.bounds) {
            FileHandle.standardError.write(Data("snapshot: no window image, writing the flat view tree (no glass)\n".utf8))
            view.cacheDisplay(in: view.bounds, to: flat)
            bitmap = flat
        } else {
            return false
        }
        guard let png = bitmap.representation(using: .png, properties: [:]) else { return false }
        let url = URL(fileURLWithPath: path)
        try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        return (try? png.write(to: url)) != nil
    }

    private static func composited(_ window: NSWindow) -> CGImage? {
        typealias CreateImage = @convention(c) (CGRect, UInt32, UInt32, UInt32) -> Unmanaged<CGImage>?
        guard let symbol = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "CGWindowListCreateImage") else { return nil }
        let create = unsafeBitCast(symbol, to: CreateImage.self)
        let includingWindow: UInt32 = 1 << 3
        let boundsIgnoreFraming: UInt32 = 1 << 0
        let bestResolution: UInt32 = 1 << 3
        let image = create(.null, includingWindow, UInt32(window.windowNumber), boundsIgnoreFraming | bestResolution)?.takeRetainedValue()
        guard let image, image.width > 16 else { return nil }
        return image
    }
}
