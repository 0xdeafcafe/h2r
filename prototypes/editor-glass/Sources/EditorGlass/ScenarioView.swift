import SwiftUI

/// The Scenario tool: the viewport fills the window, and everything else
/// (toolbar, inspector cards, the Game / Log panel) floats over it as glass.
struct ScenarioView: View {
    @Environment(EditorModel.self) private var model

    var body: some View {
        ZStack {
            ViewportView()
            HStack(alignment: .top, spacing: 12) {
                VStack(spacing: 12) {
                    ViewportToolbar()
                    Spacer(minLength: 0)
                    BottomPanel()
                }
                if model.showInspector {
                    InspectorCards()
                        .frame(width: 292)
                        .transition(.move(edge: .trailing).combined(with: .opacity))
                }
            }
            .padding(12)
        }
    }
}

// MARK: - Viewport

struct ViewportView: View {
    @Environment(EditorModel.self) private var model
    @Environment(\.colorScheme) private var colorScheme
    @State private var dragStart: Camera?
    @State private var zoomStart: Double?
    @FocusState private var focused: Bool

    private var renderer: SceneRenderer {
        SceneRenderer(
            camera: model.camera,
            slabs: model.scene.slabs,
            items: model.scene.placements.map(\.box),
            selection: model.selection,
            selectionLabel: model.selected.map { "\($0.kind.title) \($0.label)" },
            gizmo: model.gizmo,
            localAxes: model.localAxes,
            fineGrid: model.gridSnap,
            h1Overlay: model.h1Overlay,
            paired: model.paired,
            dark: colorScheme == .dark
        )
    }

    var body: some View {
        ZStack {
            // The picture only: `backgroundExtensionEffect` mirrors it under
            // the sidebar, so it must not hold anything focusable.
            SceneCanvas(renderer: renderer)
                .backgroundExtensionEffect()
            GeometryReader { geo in
                Color.clear
                    .contentShape(Rectangle())
                    .gesture(
                        SpatialTapGesture().onEnded { tap in
                            focused = true
                            model.select(renderer.hit(tap.location, size: geo.size))
                        }
                    )
                    .simultaneousGesture(
                        DragGesture(minimumDistance: 4)
                            .onChanged { drag in
                                let start = dragStart ?? model.camera
                                dragStart = start
                                model.camera.yaw = start.yaw - drag.translation.width * 0.006
                                model.camera.pitch = min(max(start.pitch + drag.translation.height * 0.004, 0.08), 1.35)
                            }
                            .onEnded { _ in dragStart = nil }
                    )
                    .simultaneousGesture(
                        MagnifyGesture()
                            .onChanged { pinch in
                                let start = zoomStart ?? model.camera.distance
                                zoomStart = start
                                model.camera.distance = min(max(start / pinch.magnification, 4), 80)
                            }
                            .onEnded { _ in zoomStart = nil }
                    )
            }
            .focusable()
            .focusEffectDisabled()
            .focused($focused)
            .onKeyPress(characters: .letters) { press in
                switch press.characters.lowercased() {
                case "w": setGizmo(.move)
                case "e": setGizmo(.rotate)
                case "r": setGizmo(.scale)
                case "x": model.localAxes.toggle()
                case "f": withAnimation(.smooth(duration: 0.6)) { model.focusSelection() }
                default: return .ignored
                }
                return .handled
            }
        }
        .ignoresSafeArea(.container, edges: .vertical)
        .accessibilityLabel("Viewport")
    }

    private func setGizmo(_ mode: GizmoMode) {
        withAnimation(.bouncy(duration: 0.45)) { model.gizmo = mode }
    }
}

/// The Canvas, animatable by its camera so Frame and Focus ease in.
struct SceneCanvas: View, @MainActor Animatable {
    var renderer: SceneRenderer

    var animatableData: Camera {
        get { renderer.camera }
        set { renderer.camera = newValue }
    }

    var body: some View {
        let renderer = renderer
        Canvas(rendersAsynchronously: false) { context, size in
            renderer.draw(&context, size: size)
        }
    }
}

// MARK: - Floating toolbar

/// The glass toolbar over the viewport. Each cluster is its own glass shape in
/// one `GlassEffectContainer`, so neighbours blend and the options pill morphs
/// out of the toolbar when the gizmo mode changes.
struct ViewportToolbar: View {
    @Environment(EditorModel.self) private var model
    @Namespace private var glass

    var body: some View {
        @Bindable var model = model
        GlassEffectContainer(spacing: 14) {
            VStack(spacing: 10) {
                HStack(spacing: 10) {
                    modes
                    axes
                    snapping
                    framing
                    if model.map.family == .halo1 {
                        overlay
                    }
                }
                options
            }
        }
        .animation(.bouncy(duration: 0.45), value: model.gizmo)
        .animation(.smooth(duration: 0.3), value: model.h1Overlay)
    }

    /// Move / Rotate / Scale: one glass capsule, with a tinted glass lozenge
    /// that slides to the active mode (the same `glassEffectID` each time).
    private var modes: some View {
        HStack(spacing: 2) {
            ForEach(GizmoMode.allCases) { mode in
                let active = model.gizmo == mode
                Button {
                    withAnimation(.bouncy(duration: 0.45)) { model.gizmo = mode }
                } label: {
                    HStack(spacing: 6) {
                        Image(systemName: mode.symbol)
                        if active {
                            Text(mode.title)
                                .transition(.opacity.combined(with: .scale(scale: 0.8, anchor: .leading)))
                        }
                    }
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(active ? AnyShapeStyle(.white) : AnyShapeStyle(.primary))
                    .padding(.horizontal, 12)
                    .frame(height: 32)
                    .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .background {
                    if active {
                        Capsule()
                            .fill(.clear)
                            .glassEffect(.regular.tint(.accentColor).interactive(), in: .capsule)
                            .glassEffectID("active-mode", in: glass)
                    }
                }
                .help("\(mode.title) (\(mode.key))")
            }
        }
        .padding(4)
        .glassEffect(.regular, in: .capsule)
    }

    private var axes: some View {
        Button {
            model.localAxes.toggle()
        } label: {
            Label(model.localAxes ? "Local" : "World", systemImage: model.localAxes ? "cube" : "globe")
                .font(.system(size: 13, weight: .medium))
                .contentTransition(.symbolEffect(.replace))
                .padding(.horizontal, 14)
                .frame(height: 40)
                .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .glassEffect(.regular.interactive(), in: .capsule)
        .help("Gizmo in the object's own axes or the world's (X)")
    }

    private var snapping: some View {
        @Bindable var model = model
        return HStack(spacing: 2) {
            SnapToggle(title: "Grid", symbol: "grid", isOn: $model.gridSnap, namespace: glass)
                .contextMenu {
                    Picker("Grid step", selection: $model.gridStep) {
                        ForEach([0.05, 0.1, 0.25, 0.5, 1.0], id: \.self) { Text("\($0.formatted()) wu").tag($0) }
                    }
                }
            SnapToggle(title: "Snap", symbol: "angle", isOn: $model.angleSnap, namespace: glass)
                .contextMenu {
                    Picker("Angle step", selection: $model.angleStep) {
                        ForEach([5.0, 15, 30, 45, 90], id: \.self) { Text("\($0.formatted())°").tag($0) }
                    }
                }
        }
        .padding(4)
        .glassEffect(.regular, in: .capsule)
    }

    /// Frame and Focus are two glass shapes joined into one with
    /// `glassEffectUnion`. The glass sits on the labels: a union across two
    /// buttons themselves hung the first layout in the key-view loop here.
    private var framing: some View {
        HStack(spacing: 0) {
            Button {
                withAnimation(.smooth(duration: 0.7)) { model.frameMap() }
            } label: {
                Image(systemName: "arrow.down.right.and.arrow.up.left.rectangle")
                    .frame(width: 44, height: 40)
                    .contentShape(Rectangle())
                    .glassEffect(.regular.interactive(), in: .capsule)
                    .glassEffectUnion(id: "framing", namespace: glass)
            }
            .help("Frame map (Home)")
            .accessibilityLabel("Frame")
            Button {
                withAnimation(.smooth(duration: 0.7)) { model.focusSelection() }
            } label: {
                Image(systemName: "scope")
                    .frame(width: 44, height: 40)
                    .contentShape(Rectangle())
                    .glassEffect(.regular.interactive(), in: .capsule)
                    .glassEffectUnion(id: "framing", namespace: glass)
            }
            .disabled(model.selection == nil)
            .help("Focus selection (F)")
            .accessibilityLabel("Focus")
        }
        .buttonStyle(.plain)
        .font(.system(size: 14, weight: .medium))
    }

    private var overlay: some View {
        Button {
            model.h1Overlay.toggle()
        } label: {
            Label("Halo 1 overlay", systemImage: model.h1Overlay ? "square.2.layers.3d.fill" : "square.2.layers.3d")
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(model.h1Overlay ? AnyShapeStyle(.white) : AnyShapeStyle(.primary))
                .padding(.horizontal, 14)
                .frame(height: 40)
                .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .glassEffect(model.h1Overlay ? .regular.tint(.orange).interactive() : .regular.interactive(), in: .capsule)
        .help("Draw the original Halo 1 level in orange over the converted one")
    }

    /// The per-mode options. The three variants share one `glassEffectID`, so
    /// the pill morphs from one to the next instead of cross-fading.
    @ViewBuilder private var options: some View {
        @Bindable var model = model
        switch model.gizmo {
        case .move:
            OptionsPill {
                AxisDots()
                Text("Step")
                    .foregroundStyle(.secondary)
                Text("\(model.gridStep.formatted()) wu")
                    .monospacedDigit()
                Text(model.gridSnap ? "snapping to grid" : "free")
                    .foregroundStyle(.secondary)
            }
            .glassEffectID("options", in: glass)
            .glassEffectTransition(.matchedGeometry)
        case .rotate:
            OptionsPill {
                Image(systemName: "angle")
                Text("\(model.angleStep.formatted())°")
                    .monospacedDigit()
                Text(model.angleSnap ? "snapping" : "free")
                    .foregroundStyle(.secondary)
            }
            .glassEffectID("options", in: glass)
            .glassEffectTransition(.matchedGeometry)
        case .scale:
            OptionsPill {
                Toggle("Uniform", isOn: $model.uniformScale)
                    .toggleStyle(.switch)
                    .controlSize(.mini)
                Text("Scenery, vehicles, machines and crates only")
                    .foregroundStyle(.secondary)
            }
            .glassEffectID("options", in: glass)
            .glassEffectTransition(.matchedGeometry)
        }
    }
}

private struct SnapToggle: View {
    let title: String
    let symbol: String
    @Binding var isOn: Bool
    let namespace: Namespace.ID

    var body: some View {
        Button {
            withAnimation(.snappy(duration: 0.25)) { isOn.toggle() }
        } label: {
            Label(title, systemImage: symbol)
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(isOn ? AnyShapeStyle(.white) : AnyShapeStyle(.primary))
                .padding(.horizontal, 12)
                .frame(height: 32)
                .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .background {
            if isOn {
                Capsule()
                    .fill(.clear)
                    .glassEffect(.regular.tint(.accentColor.opacity(0.85)).interactive(), in: .capsule)
                    .glassEffectID("snap-\(title)", in: namespace)
                    .glassEffectTransition(.materialize)
            }
        }
        .help("\(title) snapping. Right-click to pick its step.")
    }
}

private struct OptionsPill<Content: View>: View {
    @ViewBuilder var content: Content

    var body: some View {
        HStack(spacing: 8) { content }
            .font(.system(size: 12))
            .padding(.horizontal, 14)
            .frame(height: 30)
            .glassEffect(.regular, in: .capsule)
    }
}

private struct AxisDots: View {
    var body: some View {
        HStack(spacing: 3) {
            ForEach([Color.red, .green, .blue], id: \.self) { color in
                Circle().fill(color).frame(width: 7, height: 7)
            }
        }
    }
}

// MARK: - Inspector

/// The inspector as glass cards floating over the right of the viewport.
struct InspectorCards: View {
    @Environment(EditorModel.self) private var model

    var body: some View {
        @Bindable var model = model
        ScrollView {
            GlassEffectContainer(spacing: 10) {
                VStack(spacing: 10) {
                    if let index = model.selectedIndex {
                        let placement = $model.scene.placements[index]
                        header(placement.wrappedValue)
                        transform(placement)
                        object(placement)
                        spawn(placement)
                    } else {
                        empty
                    }
                    mapCard
                }
            }
        }
        .scrollIndicators(.never)
        .scrollEdgeEffectStyle(.soft, for: .vertical)
    }

    private func header(_ placement: Placement) -> some View {
        GlassCard {
            HStack(spacing: 10) {
                Image(systemName: placement.kind.symbol)
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(.white)
                    .frame(width: 34, height: 34)
                    .background(placement.kind.color.gradient, in: .rect(cornerRadius: 9))
                VStack(alignment: .leading, spacing: 1) {
                    Text(placement.entry)
                        .font(.headline)
                    Text("\(placement.kind.title) \(placement.index)")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                Menu {
                    Button("Duplicate", systemImage: "plus.square.on.square") { model.duplicateSelection() }
                    Button("Delete", systemImage: "trash", role: .destructive) { model.deleteSelection() }
                } label: {
                    Image(systemName: "ellipsis")
                        .frame(width: 26, height: 26)
                }
                .menuStyle(.button)
                .buttonStyle(.glass)
                .buttonBorderShape(.circle)
                .menuIndicator(.hidden)
            }
            if placement.scriptNamed {
                Label("A script names this object: moving it is fine, deleting it is not.", systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundStyle(.orange)
            }
        }
    }

    private func transform(_ placement: Binding<Placement>) -> some View {
        GlassCard(title: "Transform", symbol: "move.3d") {
            FieldRow(label: "Position") {
                NumberField(axis: "X", color: .red, value: placement.position.x)
                NumberField(axis: "Y", color: .green, value: placement.position.y)
                NumberField(axis: "Z", color: .blue, value: placement.position.z)
            }
            FieldRow(label: "Rotation") {
                NumberField(axis: "Y", color: .blue, value: placement.yaw, digits: 0, suffix: "°")
                NumberField(axis: "P", color: .green, value: placement.pitch, digits: 0, suffix: "°")
                NumberField(axis: "R", color: .red, value: placement.roll, digits: 0, suffix: "°")
            }
            if placement.wrappedValue.kind.hasScale {
                FieldRow(label: "Scale") {
                    Slider(value: placement.scale, in: 0.25...3)
                        .controlSize(.small)
                    Text(placement.wrappedValue.scale.formatted(.number.precision(.fractionLength(2))))
                        .font(.caption.monospacedDigit())
                        .frame(width: 34, alignment: .trailing)
                }
            }
        }
    }

    private func object(_ placement: Binding<Placement>) -> some View {
        GlassCard(title: "Object", symbol: "shippingbox") {
            LabeledContent("Palette entry") {
                Picker("Palette entry", selection: placement.entry) {
                    ForEach(placement.wrappedValue.kind.palette, id: \.self) { Text($0).tag($0) }
                }
                .labelsHidden()
                .fixedSize()
            }
            LabeledContent("Group") {
                Text(placement.wrappedValue.kind.title)
                    .foregroundStyle(.secondary)
            }
            if placement.wrappedValue.kind == .netgameEquipment {
                LabeledContent("Respawn time") {
                    Stepper(value: placement.respawnSeconds, in: 0...300, step: 15) {
                        Text("\(Int(placement.wrappedValue.respawnSeconds)) s")
                            .monospacedDigit()
                    }
                }
            }
        }
    }

    private func spawn(_ placement: Binding<Placement>) -> some View {
        GlassCard(title: "Spawn", symbol: "flag.2.crossed") {
            LabeledContent("Team") {
                Picker("Team", selection: placement.team) {
                    ForEach(Team.allCases) { team in
                        Label {
                            Text(team.rawValue)
                        } icon: {
                            Image(systemName: "circle.fill").foregroundStyle(team.color)
                        }
                        .tag(team)
                    }
                }
                .labelsHidden()
                .fixedSize()
                .disabled(!placement.wrappedValue.kind.hasTeam)
            }
            Divider()
            VStack(alignment: .leading, spacing: 6) {
                Text("Spawn flags")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                ForEach(placement.flags) { flag in
                    Toggle(flag.wrappedValue.id, isOn: flag.isOn)
                        .toggleStyle(.checkbox)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private var empty: some View {
        GlassCard {
            VStack(spacing: 6) {
                Image(systemName: "cursorarrow.click.2")
                    .font(.title2)
                    .foregroundStyle(.secondary)
                Text("Nothing selected")
                    .font(.headline)
                Text("Click a block in the viewport or an entry in the outliner.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 12)
        }
    }

    private var mapCard: some View {
        GlassCard(title: "Map", symbol: "map") {
            LabeledContent("File") { Text(model.map.file).foregroundStyle(.secondary) }
            LabeledContent("Placements") { Text("\(model.scene.placements.count)").foregroundStyle(.secondary) }
            if model.map.family == .halo1 {
                Label("Converted from Halo 1's \(model.map.id.split(separator: "-").first ?? "")", systemImage: "arrow.triangle.branch")
                    .font(.caption)
                    .foregroundStyle(.orange)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
    }
}

struct GlassCard<Content: View>: View {
    var title: String?
    var symbol: String?
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let title {
                Label(title, systemImage: symbol ?? "circle")
                    .font(.system(size: 11, weight: .semibold))
                    .textCase(.uppercase)
                    .foregroundStyle(.secondary)
            }
            content
        }
        .font(.system(size: 12))
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .glassEffect(.regular, in: .rect(cornerRadius: 20))
    }
}

private struct FieldRow<Content: View>: View {
    let label: String
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(label)
                .font(.caption)
                .foregroundStyle(.secondary)
            HStack(spacing: 6) { content }
        }
    }
}

struct NumberField: View {
    let axis: String
    let color: Color
    @Binding var value: Double
    var digits = 2
    var suffix = ""

    var body: some View {
        HStack(spacing: 4) {
            Text(axis)
                .font(.system(size: 10, weight: .bold, design: .rounded))
                .foregroundStyle(color)
            TextField(axis, value: $value, format: .number.precision(.fractionLength(digits)))
                .textFieldStyle(.plain)
                .font(.system(size: 12).monospacedDigit())
                .multilineTextAlignment(.trailing)
            if !suffix.isEmpty {
                Text(suffix).foregroundStyle(.secondary)
            }
        }
        .padding(.horizontal, 7)
        .frame(height: 26)
        .background(.primary.opacity(0.07), in: .rect(cornerRadius: 7))
    }
}

// MARK: - Bottom panel

/// Game / Log, as one glass panel under the viewport.
struct BottomPanel: View {
    @Environment(EditorModel.self) private var model

    var body: some View {
        @Bindable var model = model
        VStack(spacing: 10) {
            HStack(spacing: 12) {
                Picker("Panel", selection: $model.bottomTab) {
                    ForEach(BottomTab.allCases) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
                PairingStatus()
                    .layoutPriority(1)
                Spacer(minLength: 0)
                if model.dirty {
                    Text("unsaved")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(.orange)
                        .fixedSize()
                }
            }
            switch model.bottomTab {
            case .game: GameTab()
            case .log: LogTab()
            }
        }
        .padding(12)
        .frame(height: 164, alignment: .top)
        .glassEffect(.regular, in: .rect(cornerRadius: 22))
        .task(id: model.playing && model.paired) {
            guard model.playing && model.paired, LaunchOptions.shared.snapshot == nil else { return }
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(500))
                model.tick += 15
            }
        }
    }
}

private struct PairingStatus: View {
    @Environment(EditorModel.self) private var model

    var body: some View {
        HStack(spacing: 7) {
            Circle()
                .fill(model.paired ? Color.green : Color.secondary)
                .frame(width: 8, height: 8)
                .shadow(color: model.paired ? .green.opacity(0.8) : .clear, radius: 4)
            if model.paired {
                Text("Game: paired to h2 play pid 1234")
                    .fontWeight(.medium)
                    .fixedSize()
                Text("\(model.map.id) \(model.playing ? "playing" : "paused") tick \(model.tick) (the open map)")
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
            } else {
                Text("Game: Halo 2 is running (\(model.map.id)), not paired")
                    .foregroundStyle(.secondary)
            }
        }
        .font(.system(size: 12))
        .lineLimit(1)
    }
}

private struct GameTab: View {
    @Environment(EditorModel.self) private var model

    var body: some View {
        @Bindable var model = model
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Button("Play", systemImage: "play.fill") {
                    model.playing = true
                    model.paired = true
                    model.say("play: started h2 play on \(model.map.id) with the editor's edits")
                }
                .buttonStyle(.glassProminent)
                .help("Play the map with your edits (F5)")
                Button(model.playing ? "Pause" : "Resume", systemImage: model.playing ? "pause.fill" : "play.fill") {
                    model.playing.toggle()
                }
                .labelStyle(.iconOnly)
                .disabled(!model.paired)
                .help(model.playing ? "Pause the game" : "Resume the game")
                Button("Step", systemImage: "forward.frame.fill") {
                    model.tick += 1
                }
                .labelStyle(.iconOnly)
                .disabled(!model.paired || model.playing)
                .help("Step one tick")
                Divider().frame(height: 18)
                Button("Go to player", systemImage: "figure.walk") {
                    withAnimation(.smooth(duration: 0.7)) {
                        model.camera.tx = -3.5
                        model.camera.ty = -2.5
                        model.camera.distance = 10
                    }
                }
                .disabled(!model.paired)
                Button("Teleport player here", systemImage: "location.fill") {
                    model.say("teleported the player to the spot the camera looks at")
                }
                .disabled(!model.paired)
            }
            .buttonStyle(.glass)
            .fixedSize()
            HStack(spacing: 8) {
                Text("Spawn")
                    .foregroundStyle(.secondary)
                Picker("Spawn", selection: $model.spawnAt) {
                    ForEach(["The level's own spawn", "Under the editor camera", "At the selected placement"], id: \.self) {
                        Text($0).tag($0)
                    }
                }
                .labelsHidden()
                .fixedSize()
                Spacer(minLength: 0)
                Toggle("Auto-pair", isOn: $model.autoPair)
                    .toggleStyle(.switch)
                    .controlSize(.small)
                    .fixedSize()
                Button(model.paired ? "Unpair" : "Pair") {
                    model.paired.toggle()
                    model.say(model.paired ? "paired to h2 play pid 1234" : "unpaired from h2 play pid 1234")
                }
                .buttonStyle(.glass)
            }
            .font(.system(size: 12))
            HStack(spacing: 10) {
                Text(model.status)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Spacer(minLength: 0)
                HStack(spacing: 8) {
                    Label("players", systemImage: "square.fill").foregroundStyle(.green)
                    Label("AI", systemImage: "square.fill").foregroundStyle(.red)
                    Label("vehicles", systemImage: "square.fill").foregroundStyle(.blue)
                    Text("120 fps").foregroundStyle(.secondary).monospacedDigit()
                }
                .fixedSize()
            }
            .font(.caption)
        }
    }
}

private struct LogTab: View {
    @Environment(EditorModel.self) private var model

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 3) {
                    ForEach(model.log) { line in
                        HStack(alignment: .firstTextBaseline, spacing: 10) {
                            Text(line.time).foregroundStyle(.tertiary)
                            Text(line.text)
                                .foregroundStyle(line.level == .error ? AnyShapeStyle(.red) : line.level == .warning ? AnyShapeStyle(.orange) : AnyShapeStyle(.primary))
                        }
                        .id(line.id)
                    }
                }
                .font(.system(size: 11.5, design: .monospaced))
                .frame(maxWidth: .infinity, alignment: .leading)
                .textSelection(.enabled)
            }
            .onChange(of: model.log.count, initial: true) {
                proxy.scrollTo(model.log.last?.id, anchor: .bottom)
            }
        }
    }
}
