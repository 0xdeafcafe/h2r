import GameController
import QuartzCore
import UIKit

/// A view whose layer is the CAMetalLayer wgpu renders into.
final class MetalView: UIView {
    override class var layerClass: AnyClass { CAMetalLayer.self }
    var metalLayer: CAMetalLayer { layer as! CAMetalLayer }
}

/// Hosts the game full screen. A GCEventViewController with controller
/// interaction off sends every controller and remote button to the game,
/// so Back on the remote (and B or Menu on a pad) is the game's B and does
/// not leave the app. The TV/Home button still does.
final class GameViewController: GCEventViewController {
    private let metalView = MetalView()
    private var tv: OpaquePointer?
    private var link: CADisplayLink?
    private var drawableSize = CGSize.zero
    private let label = UILabel()

    override func loadView() {
        view = metalView
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        controllerUserInteractionEnabled = false
        view.backgroundColor = .black
        label.textColor = .white
        label.numberOfLines = 0
        label.font = .systemFont(ofSize: 28)
        label.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(label)
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 80),
            label.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -80),
            label.centerYAnchor.constraint(equalTo: view.centerYAnchor),
        ])
        let center = NotificationCenter.default
        center.addObserver(self, selector: #selector(pause), name: UIApplication.willResignActiveNotification, object: nil)
        center.addObserver(self, selector: #selector(resume), name: UIApplication.didBecomeActiveNotification, object: nil)
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        let scale = view.window?.screen.nativeScale ?? UIScreen.main.nativeScale
        let size = CGSize(width: view.bounds.width * scale, height: view.bounds.height * scale)
        guard size.width > 0, size.height > 0, size != drawableSize else { return }
        drawableSize = size
        metalView.contentScaleFactor = scale
        metalView.metalLayer.drawableSize = size
        if let tv {
            h2tv_resize(tv, UInt32(size.width), UInt32(size.height))
        } else {
            start(size)
        }
    }

    private func start(_ size: CGSize) {
        let bundle = Bundle.main.bundlePath
        let caches = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0].path
        let layer = Unmanaged.passUnretained(metalView.metalLayer).toOpaque()
        tv = h2tv_start(layer, UInt32(size.width), UInt32(size.height), bundle, caches)
        let link = CADisplayLink(target: self, selector: #selector(frame))
        link.preferredFramesPerSecond = 30
        link.add(to: .main, forMode: .common)
        self.link = link
    }

    @objc private func frame() {
        guard let tv else { return }
        if h2tv_status(tv) != 0 {
            link?.invalidate()
            link = nil
            label.text = "Halo 2 stopped.\n\n" + String(cString: h2tv_message(tv))
            return
        }
        h2tv_redraw(tv)
    }

    @objc private func pause() {
        if let tv { h2tv_set_paused(tv, true) }
    }

    @objc private func resume() {
        if let tv { h2tv_set_paused(tv, false) }
    }
}
