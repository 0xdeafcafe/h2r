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
/// Input is a paired pad through GameController, or, with none, our own
/// on-screen touch controls (TouchControlsView), which feed player one
/// through h2host_touch_update. The overlay comes and goes as pads connect
/// and disconnect and as the system keyboard shows and hides; the rule
/// itself is in Rust (h2host_pad_counts, h2host_overlay_wanted).
final class GameViewController: UIViewController {
    private let metalView = MetalView()
    private let keyInput = GameKeyInput()
    private var host: OpaquePointer?
    private var link: CADisplayLink?
    private var drawableSize = CGSize.zero
    private let label = UILabel()

    private let touchControls = TouchControlsView()
    private var overlayShown = false
    private var keyboardShown = false

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
        touchControls.frame = view.bounds
        touchControls.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        touchControls.isHidden = true
        view.addSubview(touchControls)
        let guide = view.safeAreaLayoutGuide
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: guide.leadingAnchor, constant: 40),
            label.trailingAnchor.constraint(equalTo: guide.trailingAnchor, constant: -40),
            label.centerYAnchor.constraint(equalTo: view.centerYAnchor),
        ])
        // A tap brings the keyboard back for an open field after the player
        // hid it (hiding it is not a cancel).
        let tap = UITapGestureRecognizer(target: keyInput, action: #selector(GameKeyInput.reshow))
        tap.cancelsTouchesInView = false
        // Lifting a finger off a touch control must reach it at once.
        tap.delaysTouchesEnded = false
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
        touchControls.host = host
        updateOverlay()
        if let host { h2host_touch_shown(host, overlayShown) }
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
        if overlayShown { touchControls.tick() }
    }

    @objc private func pause() {
        touchControls.releaseAll()
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

    /// Shows or hides the on-screen controls as the rule says.
    @objc private func updateOverlay() {
        let physical = GCController.controllers().filter {
            h2host_pad_counts(false, $0.extendedGamepad != nil)
        }.count
        let wanted = h2host_overlay_wanted(UInt32(physical), keyboardShown)
        if wanted != overlayShown {
            overlayShown = wanted
            if !wanted { touchControls.releaseAll() }
            touchControls.isHidden = !wanted
            if let host { h2host_touch_shown(host, wanted) }
        }
        view.bringSubviewToFront(touchControls)
    }
}
