import SwiftUI

// A tiny software "3D" for the mock viewport: an orbit camera, a perspective
// projection, and boxes drawn back to front in a SwiftUI Canvas.

struct Vec3: Hashable, Sendable {
    var x, y, z: Double

    init(_ x: Double, _ y: Double, _ z: Double) {
        self.x = x
        self.y = y
        self.z = z
    }

    static func + (a: Vec3, b: Vec3) -> Vec3 { Vec3(a.x + b.x, a.y + b.y, a.z + b.z) }
    static func - (a: Vec3, b: Vec3) -> Vec3 { Vec3(a.x - b.x, a.y - b.y, a.z - b.z) }
    static func * (a: Vec3, s: Double) -> Vec3 { Vec3(a.x * s, a.y * s, a.z * s) }

    func dot(_ o: Vec3) -> Double { x * o.x + y * o.y + z * o.z }
    func cross(_ o: Vec3) -> Vec3 { Vec3(y * o.z - z * o.y, z * o.x - x * o.z, x * o.y - y * o.x) }
    var length: Double { dot(self).squareRoot() }
    var normalized: Vec3 { self * (1 / max(length, 1e-9)) }

    func rotatedZ(_ angle: Double) -> Vec3 {
        let c = cos(angle), s = sin(angle)
        return Vec3(x * c - y * s, x * s + y * c, z)
    }
}

/// An orbit camera. It is a vector so SwiftUI can animate it (Frame, Focus).
struct Camera: VectorArithmetic, Sendable {
    var yaw, pitch, distance, tx, ty, tz: Double

    static let home = Camera(yaw: -1.02, pitch: 0.30, distance: 30, tx: 0, ty: 0, tz: 0.6)
    static let zero = Camera(yaw: 0, pitch: 0, distance: 0, tx: 0, ty: 0, tz: 0)

    static func + (a: Camera, b: Camera) -> Camera {
        Camera(yaw: a.yaw + b.yaw, pitch: a.pitch + b.pitch, distance: a.distance + b.distance, tx: a.tx + b.tx, ty: a.ty + b.ty, tz: a.tz + b.tz)
    }

    static func - (a: Camera, b: Camera) -> Camera {
        Camera(yaw: a.yaw - b.yaw, pitch: a.pitch - b.pitch, distance: a.distance - b.distance, tx: a.tx - b.tx, ty: a.ty - b.ty, tz: a.tz - b.tz)
    }

    mutating func scale(by s: Double) {
        yaw *= s; pitch *= s; distance *= s; tx *= s; ty *= s; tz *= s
    }

    var magnitudeSquared: Double {
        yaw * yaw + pitch * pitch + distance * distance + tx * tx + ty * ty + tz * tz
    }

    var target: Vec3 { Vec3(tx, ty, tz) }
}

struct Projector {
    let position, right, up, forward: Vec3
    let focal: Double
    let centre: CGPoint
    let horizonY: Double
    static let near = 0.25

    init(camera: Camera, size: CGSize) {
        let cp = cos(camera.pitch), sp = sin(camera.pitch)
        position = camera.target + Vec3(cp * cos(camera.yaw), cp * sin(camera.yaw), sp) * camera.distance
        forward = (camera.target - position).normalized
        right = forward.cross(Vec3(0, 0, 1)).normalized
        up = right.cross(forward)
        focal = size.height * 0.92
        centre = CGPoint(x: size.width / 2, y: size.height * 0.52)
        horizonY = centre.y - focal * tan(camera.pitch)
    }

    func toCamera(_ p: Vec3) -> Vec3 {
        let v = p - position
        return Vec3(v.dot(right), v.dot(up), v.dot(forward))
    }

    private func screen(_ c: Vec3) -> CGPoint {
        CGPoint(x: centre.x + c.x / c.z * focal, y: centre.y - c.y / c.z * focal)
    }

    func project(_ p: Vec3) -> CGPoint? {
        let c = toCamera(p)
        return c.z > Self.near ? screen(c) : nil
    }

    func depth(_ p: Vec3) -> Double { toCamera(p).z }

    /// A world segment clipped against the near plane.
    func segment(_ a: Vec3, _ b: Vec3) -> (CGPoint, CGPoint)? {
        var ca = toCamera(a), cb = toCamera(b)
        if ca.z <= Self.near && cb.z <= Self.near { return nil }
        if ca.z <= Self.near {
            ca = ca + (cb - ca) * ((Self.near - ca.z) / (cb.z - ca.z))
            ca.z = Self.near
        } else if cb.z <= Self.near {
            cb = cb + (ca - cb) * ((Self.near - cb.z) / (ca.z - cb.z))
            cb.z = Self.near
        }
        return (screen(ca), screen(cb))
    }
}

struct SceneBox: Hashable, Sendable {
    var center: Vec3
    var half: Vec3
    var yaw: Double
    var color: Color
    /// The placement it stands for; `nil` for level blocks.
    var id: Int?
    var isGround = false

    /// Corner `i` has +x when bit 0 is set, +y for bit 1, +z for bit 2.
    var corners: [Vec3] {
        (0..<8).map { i in
            let local = Vec3(i & 1 == 0 ? -half.x : half.x, i & 2 == 0 ? -half.y : half.y, i & 4 == 0 ? -half.z : half.z)
            return center + local.rotatedZ(yaw)
        }
    }

    static let faces: [(corners: [Int], normal: Vec3)] = [
        ([1, 3, 7, 5], Vec3(1, 0, 0)), ([0, 4, 6, 2], Vec3(-1, 0, 0)),
        ([2, 6, 7, 3], Vec3(0, 1, 0)), ([0, 1, 5, 4], Vec3(0, -1, 0)),
        ([4, 5, 7, 6], Vec3(0, 0, 1)), ([0, 2, 3, 1], Vec3(0, 0, -1)),
    ]

    static let edges: [(Int, Int)] = [
        (0, 1), (1, 3), (3, 2), (2, 0), (4, 5), (5, 7), (7, 6), (6, 4), (0, 4), (1, 5), (2, 6), (3, 7),
    ]
}

struct SceneRenderer: Sendable {
    var camera: Camera
    var slabs: [SceneBox]
    var items: [SceneBox]
    var selection: Int?
    var selectionLabel: String?
    var gizmo: GizmoMode
    var localAxes: Bool
    var fineGrid: Bool
    var h1Overlay: Bool
    var paired: Bool
    var dark: Bool

    private static let light = Vec3(0.35, 0.45, 0.82).normalized
    private static let axisColors: [Color] = [
        Color(red: 1.0, green: 0.30, blue: 0.32), Color(red: 0.40, green: 0.85, blue: 0.35), Color(red: 0.30, green: 0.55, blue: 1.0),
    ]
    private static let overlayOrange = Color(red: 1.0, green: 0.55, blue: 0.10)

    // MARK: Drawing

    func draw(_ ctx: inout GraphicsContext, size: CGSize) {
        let proj = Projector(camera: camera, size: size)
        drawBackdrop(&ctx, size: size, proj: proj)
        drawGrid(&ctx, proj: proj)
        drawHaze(&ctx, size: size, proj: proj)

        var boxes = slabs.filter { !$0.isGround } + items
        if paired {
            // The paired game's player, where it "is now".
            boxes.append(SceneBox(center: Vec3(-3.5, -2.5, 1.1), half: Vec3(0.3, 0.3, 0.8), yaw: 0.4, color: Color(red: 0.25, green: 0.95, blue: 0.45), id: nil))
        }
        boxes.sort { proj.depth($0.center) > proj.depth($1.center) }
        for ground in slabs where ground.isGround {
            drawBox(&ctx, ground, proj: proj)
        }
        for box in boxes {
            drawShadow(&ctx, box, proj: proj)
        }
        for box in boxes {
            drawBox(&ctx, box, proj: proj)
        }
        if h1Overlay {
            drawOverlay(&ctx, proj: proj)
        }
        if let box = items.first(where: { $0.id == selection && selection != nil }) {
            drawSelection(&ctx, box, proj: proj)
        }
    }

    private func drawBackdrop(_ ctx: inout GraphicsContext, size: CGSize, proj: Projector) {
        let horizon = min(max(proj.horizonY, 0), size.height)
        let sky: [Color] = dark
            ? [Color(red: 0.03, green: 0.05, blue: 0.13), Color(red: 0.10, green: 0.16, blue: 0.36), Color(red: 0.45, green: 0.36, blue: 0.52), Color(red: 0.95, green: 0.62, blue: 0.42)]
            : [Color(red: 0.22, green: 0.50, blue: 0.92), Color(red: 0.50, green: 0.74, blue: 0.98), Color(red: 0.80, green: 0.90, blue: 1.0), Color(red: 1.0, green: 0.95, blue: 0.86)]
        ctx.fill(
            Path(CGRect(x: 0, y: 0, width: size.width, height: horizon)),
            with: .linearGradient(Gradient(colors: sky), startPoint: .zero, endPoint: CGPoint(x: 0, y: max(horizon, 1)))
        )
        // A soft sun low on the horizon.
        let sun = CGPoint(x: size.width * 0.68, y: horizon - size.height * 0.03)
        let glow = Color(red: 1.0, green: 0.86, blue: 0.62)
        ctx.drawLayer { layer in
            layer.clip(to: Path(CGRect(x: 0, y: 0, width: size.width, height: horizon)))
            layer.fill(
                Path(ellipseIn: CGRect(x: sun.x - 420, y: sun.y - 420, width: 840, height: 840)),
                with: .radialGradient(
                    Gradient(colors: [glow.opacity(dark ? 0.75 : 0.9), glow.opacity(0.18), glow.opacity(0)]),
                    center: sun, startRadius: 0, endRadius: 420
                )
            )
        }
        let ground: [Color] = dark
            ? [Color(red: 0.20, green: 0.19, blue: 0.24), Color(red: 0.07, green: 0.08, blue: 0.11)]
            : [Color(red: 0.80, green: 0.80, blue: 0.80), Color(red: 0.62, green: 0.65, blue: 0.70)]
        ctx.fill(
            Path(CGRect(x: 0, y: horizon, width: size.width, height: size.height - horizon)),
            with: .linearGradient(Gradient(colors: ground), startPoint: CGPoint(x: 0, y: horizon), endPoint: CGPoint(x: 0, y: size.height))
        )
    }

    private func drawGrid(_ ctx: inout GraphicsContext, proj: Projector) {
        let extent = 60
        let ink: Color = dark ? .white : .black
        var minor = Path(), major = Path(), fine = Path()
        for i in -extent...extent {
            let v = Double(i)
            let e = Double(extent)
            for (a, b) in [(Vec3(v, -e, 0), Vec3(v, e, 0)), (Vec3(-e, v, 0), Vec3(e, v, 0))] {
                guard let (p, q) = proj.segment(a, b) else { continue }
                if i % 5 == 0 {
                    major.move(to: p); major.addLine(to: q)
                } else {
                    minor.move(to: p); minor.addLine(to: q)
                }
            }
        }
        if fineGrid {
            // Half-unit lines near the camera's target, where snapping happens.
            let cx = camera.tx.rounded(), cy = camera.ty.rounded()
            for i in -24...24 where i % 2 != 0 {
                let o = Double(i) / 2
                for (a, b) in [(Vec3(cx + o, cy - 12, 0), Vec3(cx + o, cy + 12, 0)), (Vec3(cx - 12, cy + o, 0), Vec3(cx + 12, cy + o, 0))] {
                    guard let (p, q) = proj.segment(a, b) else { continue }
                    fine.move(to: p); fine.addLine(to: q)
                }
            }
        }
        ctx.stroke(fine, with: .color(ink.opacity(dark ? 0.05 : 0.06)), lineWidth: 0.5)
        ctx.stroke(minor, with: .color(ink.opacity(dark ? 0.10 : 0.10)), lineWidth: 0.6)
        ctx.stroke(major, with: .color(ink.opacity(dark ? 0.22 : 0.20)), lineWidth: 1)
        for (axis, color) in [(Vec3(1, 0, 0), Self.axisColors[0]), (Vec3(0, 1, 0), Self.axisColors[1])] {
            if let (p, q) = proj.segment(axis * -60, axis * 60) {
                var path = Path()
                path.move(to: p); path.addLine(to: q)
                ctx.stroke(path, with: .color(color.opacity(0.55)), lineWidth: 1.2)
            }
        }
    }

    private func drawHaze(_ ctx: inout GraphicsContext, size: CGSize, proj: Projector) {
        let horizon = proj.horizonY
        let haze: Color = dark ? Color(red: 0.42, green: 0.33, blue: 0.42) : Color(red: 0.94, green: 0.93, blue: 0.92)
        let depth = size.height * 0.16
        ctx.fill(
            Path(CGRect(x: 0, y: horizon, width: size.width, height: depth)),
            with: .linearGradient(
                Gradient(colors: [haze.opacity(0.95), haze.opacity(0)]),
                startPoint: CGPoint(x: 0, y: horizon), endPoint: CGPoint(x: 0, y: horizon + depth)
            )
        )
    }

    private func facePath(_ points: [CGPoint?], _ indices: [Int]) -> Path? {
        var path = Path()
        for (n, index) in indices.enumerated() {
            guard let point = points[index] else { return nil }
            if n == 0 { path.move(to: point) } else { path.addLine(to: point) }
        }
        path.closeSubpath()
        return path
    }

    private func visibleFaces(_ box: SceneBox, proj: Projector) -> [(path: Path, shade: Double)] {
        let corners = box.corners
        let points = corners.map(proj.project)
        return SceneBox.faces.compactMap { face in
            let normal = face.normal.rotatedZ(box.yaw)
            let centre = face.corners.reduce(Vec3(0, 0, 0)) { $0 + corners[$1] } * 0.25
            guard (centre - proj.position).dot(normal) < 0, let path = facePath(points, face.corners) else { return nil }
            return (path, 0.50 + 0.50 * max(0, normal.dot(Self.light)))
        }
    }

    private func drawBox(_ ctx: inout GraphicsContext, _ box: SceneBox, proj: Projector) {
        for face in visibleFaces(box, proj: proj) {
            ctx.fill(face.path, with: .color(box.color))
            ctx.fill(face.path, with: .color(.black.opacity((1 - face.shade) * (dark ? 0.85 : 0.55))))
            ctx.stroke(face.path, with: .color(.white.opacity(dark ? 0.10 : 0.35)), lineWidth: 0.5)
        }
    }

    private func drawShadow(_ ctx: inout GraphicsContext, _ box: SceneBox, proj: Projector) {
        let floor = 0.305
        let offset = Vec3(-0.35, -0.45, 0) * (box.center.z + box.half.z - floor)
        let points: [CGPoint?] = [0, 1, 3, 2].map { i in
            let c = box.corners[i]
            return proj.project(Vec3(c.x + offset.x, c.y + offset.y, floor))
        }
        if let path = facePath(points, [0, 1, 2, 3]) {
            ctx.fill(path, with: .color(.black.opacity(dark ? 0.30 : 0.16)))
        }
    }

    private func drawOverlay(_ ctx: inout GraphicsContext, proj: Projector) {
        // The "original Halo 1 level": the same blocks, a touch off, in orange wire.
        var wire = Path()
        for slab in slabs {
            var ghost = slab
            ghost.half = slab.half * 1.03
            ghost.center = slab.center + Vec3(0.18, -0.12, 0.05)
            let corners = ghost.corners
            for (a, b) in SceneBox.edges {
                guard let (p, q) = proj.segment(corners[a], corners[b]) else { continue }
                wire.move(to: p); wire.addLine(to: q)
            }
        }
        ctx.stroke(wire, with: .color(Self.overlayOrange.opacity(0.9)), lineWidth: 1.3)
        ctx.stroke(wire, with: .color(Self.overlayOrange.opacity(0.25)), lineWidth: 4)
        for item in items where item.half.z == PlacementKind.playerStarts.half.z {
            guard let p = proj.project(item.center + Vec3(0.15, -0.1, item.half.z + 0.35)) else { continue }
            var diamond = Path()
            diamond.move(to: CGPoint(x: p.x, y: p.y - 6))
            diamond.addLine(to: CGPoint(x: p.x + 5, y: p.y))
            diamond.addLine(to: CGPoint(x: p.x, y: p.y + 6))
            diamond.addLine(to: CGPoint(x: p.x - 5, y: p.y))
            diamond.closeSubpath()
            ctx.stroke(diamond, with: .color(Self.overlayOrange), lineWidth: 1.5)
        }
    }

    private func drawSelection(_ ctx: inout GraphicsContext, _ box: SceneBox, proj: Projector) {
        let corners = box.corners
        var outline = Path()
        for (a, b) in SceneBox.edges {
            guard let (p, q) = proj.segment(corners[a], corners[b]) else { continue }
            outline.move(to: p); outline.addLine(to: q)
        }
        ctx.stroke(outline, with: .color(.white.opacity(0.35)), lineWidth: 5)
        ctx.stroke(outline, with: .color(.white), lineWidth: 1.5)

        let origin = box.center
        guard let o = proj.project(origin) else { return }
        let reach = proj.depth(origin) * 0.11
        let axes = [Vec3(1, 0, 0), Vec3(0, 1, 0), Vec3(0, 0, 1)].map { localAxes ? $0.rotatedZ(box.yaw) : $0 }
        let style = StrokeStyle(lineWidth: 2.5, lineCap: .round, lineJoin: .round)

        for (n, axis) in axes.enumerated() {
            let color = Self.axisColors[n]
            switch gizmo {
            case .move, .scale:
                guard let tip = proj.project(origin + axis * reach) else { continue }
                var shaft = Path()
                shaft.move(to: o); shaft.addLine(to: tip)
                ctx.stroke(shaft, with: .color(color), style: style)
                let dx = tip.x - o.x, dy = tip.y - o.y
                let len = max((dx * dx + dy * dy).squareRoot(), 1)
                let ux = dx / len, uy = dy / len
                if gizmo == .move {
                    var head = Path()
                    head.move(to: CGPoint(x: tip.x + ux * 12, y: tip.y + uy * 12))
                    head.addLine(to: CGPoint(x: tip.x - uy * 5.5, y: tip.y + ux * 5.5))
                    head.addLine(to: CGPoint(x: tip.x + uy * 5.5, y: tip.y - ux * 5.5))
                    head.closeSubpath()
                    ctx.fill(head, with: .color(color))
                } else {
                    ctx.fill(Path(CGRect(x: tip.x - 5, y: tip.y - 5, width: 10, height: 10)), with: .color(color))
                }
            case .rotate:
                // A ring in the plane across this axis.
                let a = axes[(n + 1) % 3], b = axes[(n + 2) % 3]
                var ring = Path()
                var started = false
                for step in 0...64 {
                    let t = Double(step) / 64 * 2 * .pi
                    guard let p = proj.project(origin + (a * cos(t) + b * sin(t)) * (reach * 0.85)) else {
                        started = false
                        continue
                    }
                    if started { ring.addLine(to: p) } else { ring.move(to: p); started = true }
                }
                ctx.stroke(ring, with: .color(color), style: StrokeStyle(lineWidth: 2.2, lineCap: .round, lineJoin: .round))
            }
        }
        let hub = CGRect(x: o.x - 4.5, y: o.y - 4.5, width: 9, height: 9)
        ctx.fill(gizmo == .scale ? Path(hub) : Path(ellipseIn: hub), with: .color(.white))

        if let selectionLabel, let top = proj.project(origin + Vec3(0, 0, box.half.z + reach * 1.15)) {
            let text = ctx.resolve(
                Text(selectionLabel).font(.system(size: 11, weight: .semibold, design: .rounded)).foregroundStyle(.white)
            )
            let measured = text.measure(in: CGSize(width: 400, height: 40))
            let plate = CGRect(x: top.x - measured.width / 2 - 8, y: top.y - measured.height / 2 - 4, width: measured.width + 16, height: measured.height + 8)
            ctx.fill(Path(roundedRect: plate, cornerRadius: plate.height / 2), with: .color(.black.opacity(0.55)))
            ctx.draw(text, at: top)
        }
    }

    // MARK: Picking

    /// The placement under a point, nearest first.
    func hit(_ point: CGPoint, size: CGSize) -> Int? {
        let proj = Projector(camera: camera, size: size)
        let nearFirst = items.sorted { proj.depth($0.center) < proj.depth($1.center) }
        for box in nearFirst {
            if visibleFaces(box, proj: proj).contains(where: { $0.path.contains(point) }) {
                return box.id
            }
        }
        // Small markers are hard to hit: accept a click close to one.
        for box in nearFirst {
            guard let c = proj.project(box.center) else { continue }
            if hypot(c.x - point.x, c.y - point.y) < 12 { return box.id }
        }
        return nil
    }
}
