import GameController
import QuartzCore
import UIKit

/// A view whose layer is the CAMetalLayer wgpu renders into.
final class MetalView: UIView {
    override class var layerClass: AnyClass { CAMetalLayer.self }
    var metalLayer: CAMetalLayer { layer as! CAMetalLayer }
}

/// Hosts the game full screen, landscape (docs/runtime/ios.md).
///
/// Input is GameController either way: a paired pad, or, with none, Apple's
/// on-screen controls (GCVirtualController), which the game sees as one more
/// extended gamepad. The overlay comes and goes as pads connect and
/// disconnect and as the system keyboard shows and hides; the rule itself is
/// in Rust (h2host_pad_counts, h2host_overlay_wanted). The overlay has no
/// Menu or Options (GCVirtualController refuses them), so Start and Back are
/// small buttons of our own (h2host_button).
final class GameViewController: UIViewController {
    private let metalView = MetalView()
    private let keyInput = GameKeyInput()
    private var host: OpaquePointer?
    private var link: CADisplayLink?
    private var drawableSize = CGSize.zero
    private let label = UILabel()

    private var overlay: GCVirtualController?
    private var keyboardShown = false
    private let startButton = OverlayButton(title: "Start", button: 0)
    private let backButton = OverlayButton(title: "Back", button: 1)

    override var prefersStatusBarHidden: Bool { true }
    override var prefersHomeIndicatorAutoHidden: Bool { true }
    // Edge swipes reach the game (and the controls) first; a second swipe
    // still opens Control Centre or goes home.
    override var preferredScreenEdgesDeferringSystemGestures: UIRectEdge { .all }
    override var supportedInterfaceOrientations: UIInterfaceOrientationMask { .landscape }

    override func loadView() {
        view = metalView
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        view.addSubview(keyInput)
        label.textColor = .white
        label.numberOfLines = 0
        label.font = .systemFont(ofSize: 20)
        label.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(label)
        for b in [backButton, startButton] {
            b.translatesAutoresizingMaskIntoConstraints = false
            b.isHidden = true
            view.addSubview(b)
        }
        let guide = view.safeAreaLayoutGuide
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: guide.leadingAnchor, constant: 40),
            label.trailingAnchor.constraint(equalTo: guide.trailingAnchor, constant: -40),
            label.centerYAnchor.constraint(equalTo: view.centerYAnchor),
            backButton.topAnchor.constraint(equalTo: guide.topAnchor, constant: 12),
            backButton.trailingAnchor.constraint(equalTo: view.centerXAnchor, constant: -8),
            startButton.topAnchor.constraint(equalTo: guide.topAnchor, constant: 12),
            startButton.leadingAnchor.constraint(equalTo: view.centerXAnchor, constant: 8),
        ])
        // A tap brings the keyboard back for an open field after the player
        // hid it (hiding it is not a cancel).
        let tap = UITapGestureRecognizer(target: keyInput, action: #selector(GameKeyInput.reshow))
        tap.cancelsTouchesInView = false
        view.addGestureRecognizer(tap)

        let center = NotificationCenter.default
        center.addObserver(self, selector: #selector(pause), name: UIApplication.willResignActiveNotification, object: nil)
        center.addObserver(self, selector: #selector(resume), name: UIApplication.didBecomeActiveNotification, object: nil)
        center.addObserver(self, selector: #selector(updateOverlay), name: .GCControllerDidConnect, object: nil)
        center.addObserver(self, selector: #selector(updateOverlay), name: .GCControllerDidDisconnect, object: nil)
        center.addObserver(self, selector: #selector(keyboardWillShow), name: UIResponder.keyboardWillShowNotification, object: nil)
        center.addObserver(self, selector: #selector(keyboardWillHide), name: UIResponder.keyboardWillHideNotification, object: nil)
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        setNeedsUpdateOfScreenEdgesDeferringSystemGestures()
        setNeedsUpdateOfHomeIndicatorAutoHidden()
        updateOverlay()
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        let scale = view.window?.screen.nativeScale ?? UIScreen.main.nativeScale
        let size = CGSize(width: view.bounds.width * scale, height: view.bounds.height * scale)
        guard size.width > 0, size.height > 0, size != drawableSize else { return }
        drawableSize = size
        metalView.contentScaleFactor = scale
        metalView.metalLayer.drawableSize = size
        if let host {
            h2host_resize(host, UInt32(size.width), UInt32(size.height))
        } else {
            start(size)
        }
    }

    private func start(_ size: CGSize) {
        let files = FileManager.default
        let bundle = Bundle.main.bundlePath
        let caches = files.urls(for: .cachesDirectory, in: .userDomainMask)[0].path
        // Saves, settings and the Xbox hard disk persist in Documents.
        let documents = files.urls(for: .documentDirectory, in: .userDomainMask)[0].path
        let layer = Unmanaged.passUnretained(metalView.metalLayer).toOpaque()
        host = h2host_start(layer, UInt32(size.width), UInt32(size.height), bundle, caches, documents)
        keyInput.host = host
        let link = CADisplayLink(target: self, selector: #selector(frame))
        link.preferredFramesPerSecond = 30
        link.add(to: .main, forMode: .common)
        self.link = link
    }

    @objc private func frame() {
        guard let host else { return }
        if h2host_status(host) != 0 {
            link?.invalidate()
            link = nil
            label.text = "Halo 2 stopped.\n\n" + String(cString: h2host_message(host))
            return
        }
        h2host_redraw(host)
        keyInput.follow()
    }

    @objc private func pause() {
        if let host { h2host_set_paused(host, true) }
    }

    @objc private func resume() {
        if let host { h2host_set_paused(host, false) }
    }

    @objc private func keyboardWillShow() {
        keyboardShown = true
        updateOverlay()
    }

    @objc private func keyboardWillHide() {
        keyboardShown = false
        updateOverlay()
    }

    /// Connects or disconnects the on-screen controls as the rule says.
    @objc private func updateOverlay() {
        let own = overlay?.controller
        let physical = GCController.controllers().filter {
            h2host_pad_counts($0 === own, $0.extendedGamepad != nil)
        }.count
        let wanted = h2host_overlay_wanted(UInt32(physical), keyboardShown)
        if wanted, overlay == nil {
            let config = GCVirtualController.Configuration()
            config.elements = [
                GCInputLeftThumbstick, GCInputRightThumbstick,
                GCInputButtonA, GCInputButtonB, GCInputButtonX, GCInputButtonY,
                // Shoulders are White (left) and Black (right), as on every pad.
                GCInputLeftShoulder, GCInputRightShoulder,
                GCInputLeftTrigger, GCInputRightTrigger,
                // No d-pad: the touch controller refuses it alongside the
                // left stick, and the game's menus move with the stick.
            ]
            let vc = GCVirtualController(configuration: config)
            vc.connect { error in
                if let error { NSLog("h2: on-screen controls failed to connect: \(error)") }
            }
            overlay = vc
        } else if !wanted, let vc = overlay {
            vc.disconnect()
            overlay = nil
        }
        for b in [startButton, backButton] {
            b.host = host
            b.isHidden = overlay == nil
            view.bringSubviewToFront(b)
        }
    }
}

/// A small pill that holds Start or Back down while touched.
final class OverlayButton: UIButton {
    var host: OpaquePointer?
    private let button: Int32

    init(title: String, button: Int32) {
        self.button = button
        super.init(frame: .zero)
        var config = UIButton.Configuration.filled()
        config.title = title
        config.cornerStyle = .capsule
        config.baseBackgroundColor = UIColor(white: 1, alpha: 0.18)
        config.baseForegroundColor = UIColor(white: 1, alpha: 0.85)
        config.contentInsets = NSDirectionalEdgeInsets(top: 6, leading: 16, bottom: 6, trailing: 16)
        configuration = config
        addTarget(self, action: #selector(down), for: [.touchDown, .touchDragEnter])
        addTarget(self, action: #selector(up), for: [.touchUpInside, .touchUpOutside, .touchDragExit, .touchCancel])
    }

    required init?(coder: NSCoder) { nil }

    @objc private func down() {
        if let host { _ = h2host_button(host, button, true) }
    }

    @objc private func up() {
        if let host { _ = h2host_button(host, button, false) }
    }
}
