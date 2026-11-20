import CoreMotion
import QuartzCore
import UIKit

/// The on-screen touch controls (docs/runtime/ios.md, "Touch").
///
/// This view only tracks fingers and draws plain shapes. Each finger gets a
/// role when it lands: a button (held until it lifts), the move stick (a
/// finger landing on the left side away from a button; the stick appears
/// where it lands), or look (one landing on the right side). A finger on a
/// right-side button looks as well once it moves: Fire at once, the others
/// after a few points, so a tap stays a tap. What the fingers hold is sent
/// to Rust as an H2TouchFrame on every change and once per display frame
/// (h2host_touch_update); the look maths, the stick, the crouch toggle and
/// auto-fire are in crates/platform/input/src/touch*.rs.
///
/// Which buttons show, and where, follows player one's state: on foot,
/// driving, or in a passenger or gunner seat, and the held weapon (Rust's
/// input::touch_layout, read with h2host_touch_layout each frame). Changes
/// animate.
final class TouchControlsView: UIView {
    var host: OpaquePointer?

    /// Thumb travel for a full stick (from the layout: 56 points on foot,
    /// 80 driving).
    private var stickRadius: CGFloat = 56
    /// A non-Fire button's finger looks after moving this far.
    private let lookSlop: CGFloat = 8

    private struct PadButton {
        let bit: UInt32
        var label: String
        var center = CGPoint.zero
        var radius: CGFloat = 0
        /// A pill (Start, Back) rather than a circle.
        let pill: Bool
        var visible = true
        var dimmed = false
        let shape = CAShapeLayer()
        let text = CATextLayer()
    }

    private enum Role {
        case button(Int)
        case move(origin: CGPoint, at: CGPoint)
        /// `button`: the button the finger landed on, held until it lifts.
        /// `looking`: past the slop (from the start for Fire and open
        /// screen).
        case look(start: CGPoint, last: CGPoint, button: Int?, looking: Bool)
    }

    private var buttons: [PadButton] = []
    private var roles: [ObjectIdentifier: Role] = [:]
    private var pendingLook = CGPoint.zero
    /// Crouch is on (Rust holds the toggle; h2host_touch_crouched).
    private var crouchLit = false
    private var layoutInfo = H2TouchLayout()
    private var layoutKnown = false
    private let stickBase = CAShapeLayer()
    private let stickKnob = CAShapeLayer()
    private let stickHint = CAShapeLayer()
    private let motion = CMMotionManager()
    private var crouchIndex = 0
    private var fireIndex = 0

    override init(frame: CGRect) {
        super.init(frame: frame)
        isMultipleTouchEnabled = true
        backgroundColor = .clear
        // Halo 2's Default layout: what each pad control does.
        let spec: [(UInt32, String, Bool)] = [
            (UInt32(H2_TOUCH_RIGHT_TRIGGER), "FIRE", false),
            (UInt32(H2_TOUCH_A), "JUMP", false),
            (UInt32(H2_TOUCH_B), "MELEE", false),
            (UInt32(H2_TOUCH_X), "RELOAD", false),
            (UInt32(H2_TOUCH_Y), "SWITCH", false),
            (UInt32(H2_TOUCH_LEFT_TRIGGER), "GRENADE", false),
            (UInt32(H2_TOUCH_LEFT_THUMB), "CROUCH", false),
            (UInt32(H2_TOUCH_RIGHT_THUMB), "ZOOM", false),
            (UInt32(H2_TOUCH_BLACK), "SWAP\nGREN", false),
            (UInt32(H2_TOUCH_WHITE), "LIGHT", false),
            (UInt32(H2_TOUCH_BACK), "Back", true),
            (UInt32(H2_TOUCH_START), "Start", true),
        ]
        for (bit, label, pill) in spec {
            let b = PadButton(bit: bit, label: label, pill: pill)
            b.shape.lineWidth = 1.5
            b.text.string = label
            b.text.alignmentMode = .center
            b.text.foregroundColor = UIColor(white: 1, alpha: 0.85).cgColor
            b.text.contentsScale = UIScreen.main.scale
            b.text.isWrapped = true
            layer.addSublayer(b.shape)
            layer.addSublayer(b.text)
            if bit == UInt32(H2_TOUCH_LEFT_THUMB) { crouchIndex = buttons.count }
            if bit == UInt32(H2_TOUCH_RIGHT_TRIGGER) { fireIndex = buttons.count }
            buttons.append(b)
        }
        for (l, alpha) in [(stickHint, 0.10), (stickBase, 0.16), (stickKnob, 0.35)] {
            l.fillColor = UIColor(white: 1, alpha: alpha).cgColor
            l.strokeColor = UIColor(white: 1, alpha: 0.4).cgColor
            l.lineWidth = 1.5
            layer.addSublayer(l)
        }
        stickBase.isHidden = true
        stickKnob.isHidden = true
    }

    required init?(coder: NSCoder) { nil }

    // MARK: Layout

    private func has(_ flag: UInt32) -> Bool { layoutInfo.flags & flag != 0 }

    override func layoutSubviews() {
        super.layoutSubviews()
        place(animated: false)
    }

    /// Positions, sizes, labels and visibility for the current layout.
    private func place(animated: Bool) {
        let safe = bounds.inset(by: safeAreaInsets)
        guard safe.width > 0, safe.height > 0 else { return }
        // A button's radius: 26 points on a phone, up to 40 on an iPad.
        let r = min(max(bounds.height * 0.065, 26), 40)
        let m = r * 0.4
        let x = safe.maxX - m, y = safe.maxY - m
        let lx = safe.minX + m
        let kind = layoutKnown ? layoutInfo.layout : UInt32(H2_TOUCH_LAYOUT_ON_FOOT)
        let fire = CGPoint(x: x - 4.3 * r, y: y - 3.9 * r)
        // Right hand: Fire big in the middle of the thumb's reach, the rest
        // around it. Left hand: flashlight top left.
        var place: [UInt32: (CGPoint, CGFloat, String)] = [
            UInt32(H2_TOUCH_RIGHT_TRIGGER): (fire, 1.75 * r, "FIRE"),
            UInt32(H2_TOUCH_A): (CGPoint(x: x - r, y: y - r), r, "JUMP"),
            UInt32(H2_TOUCH_LEFT_THUMB): (CGPoint(x: x - 3.6 * r, y: y - r * 0.9), 0.85 * r, "CROUCH"),
            UInt32(H2_TOUCH_B): (CGPoint(x: x - r, y: y - 3.4 * r), r, "MELEE"),
            UInt32(H2_TOUCH_X): (CGPoint(x: x - r, y: y - 5.8 * r), r, "RELOAD"),
            UInt32(H2_TOUCH_Y): (CGPoint(x: x - 3.4 * r, y: y - 7.0 * r), r, "SWITCH"),
            UInt32(H2_TOUCH_LEFT_TRIGGER): (CGPoint(x: x - 7.4 * r, y: y - 2.4 * r), r, "GRENADE"),
            UInt32(H2_TOUCH_RIGHT_THUMB): (CGPoint(x: x - 7.0 * r, y: y - 5.6 * r), 0.85 * r, "ZOOM"),
            UInt32(H2_TOUCH_BLACK): (CGPoint(x: x - 9.3 * r, y: y - 1.0 * r), 0.75 * r, "SWAP\nGREN"),
            UInt32(H2_TOUCH_WHITE): (CGPoint(x: lx + 0.8 * r, y: safe.minY + m + 0.8 * r), 0.8 * r, "LIGHT"),
        ]
        if kind == UInt32(H2_TOUCH_LAYOUT_DRIVER) {
            // Driving: Boost under the right thumb in the corner, Exit high
            // and out of the way (hold it), Zoom for the tank.
            place[UInt32(H2_TOUCH_LEFT_TRIGGER)] = (CGPoint(x: x - 1.3 * r, y: y - 1.4 * r), 1.3 * r, "BOOST")
            place[UInt32(H2_TOUCH_X)] = (CGPoint(x: x - r, y: y - 7.0 * r), r, "EXIT\n(hold)")
        } else if kind == UInt32(H2_TOUCH_LAYOUT_TURRET) {
            place[UInt32(H2_TOUCH_X)] = (CGPoint(x: x - r, y: y - 7.0 * r), r, "EXIT\n(hold)")
            place[UInt32(H2_TOUCH_RIGHT_THUMB)] = (CGPoint(x: fire.x - 2.6 * r, y: fire.y + 1.2 * r), 1.1 * r, "ZOOM")
        } else {
            if has(UInt32(H2_TOUCH_FLAG_ZOOM_NEAR_FIRE)) {
                // A scope: Zoom large, up and left of Fire.
                place[UInt32(H2_TOUCH_RIGHT_THUMB)] = (CGPoint(x: fire.x - 2.9 * r, y: fire.y - 1.3 * r), 1.15 * r, "ZOOM")
            }
            if has(UInt32(H2_TOUCH_FLAG_DUAL)) {
                // Dual-wielding: the left gun's trigger under the left
                // thumb, above where the move stick usually lands.
                place[UInt32(H2_TOUCH_LEFT_TRIGGER)] = (CGPoint(x: lx + 2.0 * r, y: safe.midY - 0.6 * r), 1.4 * r, "FIRE L")
            }
        }
        stickRadius = layoutKnown ? CGFloat(max(layoutInfo.stick_radius, 30)) : 56

        CATransaction.begin()
        if animated {
            CATransaction.setAnimationDuration(0.25)
        } else {
            CATransaction.setDisableActions(true)
        }
        for i in buttons.indices {
            var b = buttons[i]
            b.visible = !layoutKnown || layoutInfo.visible & b.bit != 0
            b.dimmed = layoutKnown && layoutInfo.dimmed & b.bit != 0
            if b.pill {
                // Start and Back: two pills at the top centre.
                let w = 2.4 * r, h = 0.95 * r
                let cx = bounds.midX + (b.bit == UInt32(H2_TOUCH_START) ? 1 : -1) * (w / 2 + 6)
                b.center = CGPoint(x: cx, y: safe.minY + m + h / 2)
                b.radius = w / 2
                let rect = CGRect(x: cx - w / 2, y: b.center.y - h / 2, width: w, height: h)
                b.shape.path = UIBezierPath(roundedRect: rect, cornerRadius: h / 2).cgPath
                b.text.fontSize = 0.42 * r
                b.text.frame = rect.insetBy(dx: 2, dy: (h - 0.55 * r) / 2)
            } else if let (c, radius, label) = place[b.bit] {
                b.center = c
                b.radius = radius
                b.label = label
                b.text.string = label
                let rect = CGRect(x: c.x - radius, y: c.y - radius, width: 2 * radius, height: 2 * radius)
                b.shape.path = UIBezierPath(ovalIn: rect).cgPath
                let big = b.bit == UInt32(H2_TOUCH_RIGHT_TRIGGER) || label == "FIRE L"
                b.text.fontSize = big ? 0.5 * r : 0.34 * r
                let lines: CGFloat = label.contains("\n") ? 2 : 1
                let th = b.text.fontSize * 1.25 * lines
                b.text.frame = CGRect(x: c.x - radius, y: c.y - th / 2, width: 2 * radius, height: th)
            }
            let alpha: Float = b.visible ? (b.dimmed ? 0.35 : 1) : 0
            b.shape.opacity = alpha
            b.text.opacity = alpha
            buttons[i] = b
        }
        let hint = CGPoint(x: lx + 2.6 * r, y: safe.maxY - m - 2.6 * r)
        stickHint.path = UIBezierPath(ovalIn: CGRect(x: hint.x - stickRadius, y: hint.y - stickRadius,
                                                    width: 2 * stickRadius, height: 2 * stickRadius)).cgPath
        CATransaction.commit()
        redraw()
    }

    // MARK: Touches

    private func button(at p: CGPoint) -> Int? {
        // Generous: a finger is wide. The nearest shown button within 1.2
        // radii.
        var best: (Int, CGFloat)?
        for (i, b) in buttons.enumerated() where b.visible {
            let d = hypot(p.x - b.center.x, p.y - b.center.y)
            let reach = b.pill ? b.radius : b.radius * 1.2
            if d <= reach, best == nil || d < best!.1 { best = (i, d) }
        }
        return best?.0
    }

    private var moveTouchActive: Bool {
        roles.values.contains { if case .move = $0 { return true } else { return false } }
    }

    /// The look side: the right part of the screen.
    private func onLookSide(_ p: CGPoint) -> Bool { p.x >= bounds.width * 0.45 }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        for t in touches {
            let p = t.location(in: self)
            let role: Role
            if let i = button(at: p) {
                if onLookSide(buttons[i].center), !buttons[i].pill {
                    // Fire looks at once; other buttons once they move.
                    role = .look(start: p, last: p, button: i, looking: i == fireIndex)
                } else {
                    role = .button(i)
                }
            } else if !onLookSide(p), !moveTouchActive {
                role = .move(origin: p, at: p)
            } else {
                role = .look(start: p, last: p, button: nil, looking: true)
            }
            roles[ObjectIdentifier(t)] = role
        }
        changed()
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
        for t in touches {
            let id = ObjectIdentifier(t)
            let p = t.location(in: self)
            switch roles[id] {
            case .move(var origin, _)?:
                // The stick's centre trails a thumb that runs past the
                // edge, so turning back responds at once.
                let d = CGPoint(x: p.x - origin.x, y: p.y - origin.y)
                let len = hypot(d.x, d.y)
                if len > stickRadius {
                    origin = CGPoint(x: p.x - d.x / len * stickRadius, y: p.y - d.y / len * stickRadius)
                }
                roles[id] = .move(origin: origin, at: p)
            case .look(let start, let last, let button, let looking)?:
                if looking {
                    pendingLook.x += p.x - last.x
                    pendingLook.y += p.y - last.y
                    roles[id] = .look(start: start, last: p, button: button, looking: true)
                } else {
                    // Past the slop: look from here on. The slop itself is
                    // not turned, so a tap that slid a little stays still.
                    let past = hypot(p.x - start.x, p.y - start.y) >= lookSlop
                    roles[id] = .look(start: start, last: p, button: button, looking: past)
                }
            default:
                break
            }
        }
        changed()
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
        for t in touches { roles[ObjectIdentifier(t)] = nil }
        changed()
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
        touchesEnded(touches, with: event)
    }

    /// Lets go of every finger (the controls hid, the app left the screen).
    func releaseAll() {
        roles.removeAll()
        pendingLook = .zero
        changed()
    }

    // MARK: Frames

    private var held: UInt32 {
        roles.values.reduce(0) { bits, role in
            switch role {
            case .button(let i): return bits | buttons[i].bit
            case .look(_, _, let i?, _): return bits | buttons[i].bit
            default: return bits
            }
        }
    }

    private func changed() {
        send()
        redraw()
    }

    /// Once per display frame: the gyro, the layout, and a frame even with
    /// no touch change so motion keeps flowing.
    func tick() {
        updateGyro()
        updateLayout()
        send()
        // A toggled crouch stays lit.
        if let host, h2host_touch_crouched(host) != crouchLit {
            crouchLit.toggle()
            redraw()
        }
    }

    /// Follows the layout Rust picks for player one's state; a change
    /// animates. Fingers keep what they hold.
    private func updateLayout() {
        guard let host else { return }
        var l = H2TouchLayout()
        guard h2host_touch_layout(host, &l) else { return }
        if layoutKnown, l.generation == layoutInfo.generation { return }
        let target = UInt32(H2_TOUCH_FLAG_ON_TARGET)
        let tintOnly = layoutKnown && l.layout == layoutInfo.layout && l.visible == layoutInfo.visible
            && l.dimmed == layoutInfo.dimmed && (l.flags & ~target) == (layoutInfo.flags & ~target)
        let animated = layoutKnown
        layoutInfo = l
        layoutKnown = true
        if tintOnly {
            redraw() // only the red reticle changed: Fire's tint
        } else {
            place(animated: animated)
        }
    }

    private func send() {
        guard let host else { return }
        var f = H2TouchFrame()
        f.buttons = held
        for role in roles.values {
            if case .move(let origin, let at) = role {
                f.move_active = true
                f.move_x = Float(at.x - origin.x)
                f.move_y = Float(at.y - origin.y)
            }
        }
        f.look_dx = Float(pendingLook.x)
        f.look_dy = Float(pendingLook.y)
        pendingLook = .zero
        if let m = motion.deviceMotion, motion.isDeviceMotionActive {
            // Device axes to screen terms (turn right +x, look up +y) for
            // the two landscape orientations.
            let left = window?.windowScene?.interfaceOrientation == .landscapeLeft
            let s: Double = left ? 1 : -1
            f.gyro_x = Float(s * m.rotationRate.x)
            f.gyro_y = Float(s * m.rotationRate.y)
        }
        _ = h2host_touch_update(host, &f)
    }

    private func updateGyro() {
        guard let host, motion.isDeviceMotionAvailable else { return }
        let on = !isHidden && h2host_touch_gyro(host) > 0
        if on, !motion.isDeviceMotionActive {
            motion.deviceMotionUpdateInterval = 1.0 / 60
            motion.startDeviceMotionUpdates()
        } else if !on, motion.isDeviceMotionActive {
            motion.stopDeviceMotionUpdates()
        }
    }

    // MARK: Drawing

    private func isHeld(_ i: Int) -> Bool {
        held & buttons[i].bit != 0
    }

    private func redraw() {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        let onTarget = has(UInt32(H2_TOUCH_FLAG_ON_TARGET))
        for i in buttons.indices {
            let b = buttons[i]
            let on = isHeld(i) || (i == crouchIndex && crouchLit)
            if i == fireIndex, onTarget {
                // The reticle is red: so is Fire.
                b.shape.fillColor = UIColor(red: 1, green: 0.25, blue: 0.2, alpha: on ? 0.5 : 0.3).cgColor
                b.shape.strokeColor = UIColor(red: 1, green: 0.4, blue: 0.35, alpha: 0.85).cgColor
            } else {
                b.shape.fillColor = UIColor(white: 1, alpha: on ? 0.4 : (i == fireIndex ? 0.18 : 0.12)).cgColor
                b.shape.strokeColor = UIColor(white: 1, alpha: on ? 0.8 : 0.45).cgColor
            }
        }
        var stick: (CGPoint, CGPoint)?
        for role in roles.values {
            if case .move(let origin, let at) = role { stick = (origin, at) }
        }
        stickHint.isHidden = stick != nil
        stickBase.isHidden = stick == nil
        stickKnob.isHidden = stick == nil
        if let (origin, at) = stick {
            stickBase.path = UIBezierPath(ovalIn: CGRect(x: origin.x - stickRadius, y: origin.y - stickRadius,
                                                         width: 2 * stickRadius, height: 2 * stickRadius)).cgPath
            var d = CGPoint(x: at.x - origin.x, y: at.y - origin.y)
            let len = hypot(d.x, d.y)
            if len > stickRadius { d = CGPoint(x: d.x / len * stickRadius, y: d.y / len * stickRadius) }
            let k: CGFloat = 0.45 * stickRadius
            stickKnob.path = UIBezierPath(ovalIn: CGRect(x: origin.x + d.x - k, y: origin.y + d.y - k,
                                                         width: 2 * k, height: 2 * k)).cgPath
        }
        CATransaction.commit()
    }
}
