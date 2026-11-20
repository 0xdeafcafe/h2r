import SwiftUI

// The HUD tool, lightly: a 640 by 480 screen with placeholder widgets you can
// select and drag, a state strip under it, and a glass inspector card.
// The widgets are plain shapes; none of it is the game's art.

struct HUDWidget: Identifiable, Hashable, Sendable {
    enum Shape: Sendable { case meter, box, counter, ring, crosshair, text }
    let id: String
    var shape: Shape
    var anchor: String
    var origin: CGPoint
    var size: CGSize
    var isText = false

    static let all: [HUDWidget] = [
        HUDWidget(id: "shield_meter", shape: .meter, anchor: "top right", origin: CGPoint(x: 400, y: 34), size: CGSize(width: 200, height: 18)),
        HUDWidget(id: "health_ticks", shape: .box, anchor: "top right", origin: CGPoint(x: 400, y: 58), size: CGSize(width: 200, height: 8)),
        HUDWidget(id: "weapon_backdrop", shape: .box, anchor: "top left", origin: CGPoint(x: 40, y: 30), size: CGSize(width: 150, height: 44)),
        HUDWidget(id: "rounds_loaded", shape: .counter, anchor: "top left", origin: CGPoint(x: 48, y: 80), size: CGSize(width: 134, height: 22)),
        HUDWidget(id: "grenade_count", shape: .text, anchor: "top left", origin: CGPoint(x: 200, y: 36), size: CGSize(width: 48, height: 30), isText: true),
        HUDWidget(id: "motion_tracker", shape: .ring, anchor: "bottom left", origin: CGPoint(x: 36, y: 330), size: CGSize(width: 116, height: 116)),
        HUDWidget(id: "crosshair", shape: .crosshair, anchor: "centre", origin: CGPoint(x: 296, y: 216), size: CGSize(width: 48, height: 48)),
    ]
}

@MainActor @Observable
final class HUDModel {
    static let shared = HUDModel()
    var widgets = HUDWidget.all
    var selection: HUDWidget.ID? = "shield_meter"
    var shield = 0.7
    var health = 1.0
    var rounds = 24.0
    var grenades = 2.0
    var showRects = true
    var showFrame = true
    var zoom = 1.25
    var screen = "fullscreen"
}

/// The widget list, shown in the sidebar while the HUD tool is open.
struct HUDWidgetList: View {
    private var hud = HUDModel.shared

    var body: some View {
        Section("HUDs") {
            ForEach(["masterchief *", "battle_rifle", "warthog_driver"], id: \.self) { name in
                SidebarRow(isSelected: name.hasPrefix("masterchief")) {} label: {
                    Label(name, systemImage: "viewfinder")
                }
            }
        }
        Section("Widgets") {
            ForEach(hud.widgets) { widget in
                SidebarRow(isSelected: hud.selection == widget.id) {
                    hud.selection = widget.id
                } label: {
                    Label(widget.id, systemImage: widget.isText ? "doc.text" : "square")
                }
            }
        }
    }
}

struct HUDView: View {
    @Environment(\.colorScheme) private var colorScheme
    @Bindable private var hud = HUDModel.shared
    @State private var dragStart: CGPoint?

    var body: some View {
        ZStack {
            LinearGradient(
                colors: colorScheme == .dark
                    ? [Color(red: 0.10, green: 0.11, blue: 0.16), Color(red: 0.04, green: 0.04, blue: 0.06)]
                    : [Color(red: 0.92, green: 0.93, blue: 0.96), Color(red: 0.80, green: 0.82, blue: 0.87)],
                startPoint: .top, endPoint: .bottom
            )
            .ignoresSafeArea()
            .backgroundExtensionEffect()
            HStack(alignment: .top, spacing: 12) {
                VStack(spacing: 12) {
                    controls
                    Spacer(minLength: 0)
                    screen
                    Spacer(minLength: 0)
                    state
                }
                inspector
                    .frame(width: 292)
            }
            .padding(12)
        }
    }

    private var controls: some View {
        GlassEffectContainer(spacing: 10) {
            HStack(spacing: 10) {
                Picker("Screen", selection: $hud.screen) {
                    ForEach(["fullscreen", "half screen", "quarter screen"], id: \.self) { Text($0).tag($0) }
                }
                .labelsHidden()
                .fixedSize()
                .padding(.horizontal, 10)
                .frame(height: 38)
                .glassEffect(.regular, in: .capsule)
                HStack(spacing: 8) {
                    Image(systemName: "minus.magnifyingglass")
                    Slider(value: $hud.zoom, in: 0.75...1.6).frame(width: 110)
                    Image(systemName: "plus.magnifyingglass")
                }
                .padding(.horizontal, 12)
                .frame(height: 38)
                .glassEffect(.regular, in: .capsule)
                glassToggle("Rects", symbol: "rectangle.dashed", isOn: $hud.showRects)
                glassToggle("HUD frame", symbol: "viewfinder", isOn: $hud.showFrame)
            }
            .font(.system(size: 12, weight: .medium))
        }
    }

    private func glassToggle(_ title: String, symbol: String, isOn: Binding<Bool>) -> some View {
        Button {
            withAnimation(.snappy(duration: 0.25)) { isOn.wrappedValue.toggle() }
        } label: {
            Label(title, systemImage: symbol)
                .foregroundStyle(isOn.wrappedValue ? AnyShapeStyle(.white) : AnyShapeStyle(.primary))
                .padding(.horizontal, 14)
                .frame(height: 38)
                .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .glassEffect(isOn.wrappedValue ? .regular.tint(.accentColor).interactive() : .regular.interactive(), in: .capsule)
    }

    /// The 640 by 480 screen.
    private var screen: some View {
        ZStack(alignment: .topLeading) {
            LinearGradient(
                colors: [Color(red: 0.30, green: 0.42, blue: 0.62), Color(red: 0.62, green: 0.55, blue: 0.50), Color(red: 0.22, green: 0.24, blue: 0.22)],
                startPoint: .top, endPoint: .bottom
            )
            if hud.showFrame {
                Rectangle()
                    .strokeBorder(.green.opacity(0.8), style: StrokeStyle(lineWidth: 1, dash: [5, 4]))
                    .padding(24)
            }
            ForEach($hud.widgets) { $widget in
                let selected = hud.selection == widget.id
                WidgetShape(widget: widget, hud: hud)
                    .frame(width: widget.size.width, height: widget.size.height)
                    .overlay {
                        if hud.showRects || selected {
                            Rectangle().strokeBorder(selected ? Color.white : Color.blue.opacity(0.9), lineWidth: selected ? 1.5 : 1)
                        }
                    }
                    .contentShape(Rectangle())
                    .offset(x: widget.origin.x, y: widget.origin.y)
                    .gesture(
                        DragGesture(minimumDistance: 0)
                            .onChanged { drag in
                                hud.selection = widget.id
                                let start = dragStart ?? widget.origin
                                dragStart = start
                                widget.origin = CGPoint(
                                    x: (start.x + drag.translation.width / hud.zoom).rounded(),
                                    y: (start.y + drag.translation.height / hud.zoom).rounded()
                                )
                            }
                            .onEnded { _ in dragStart = nil }
                    )
            }
        }
        .frame(width: 640, height: 480)
        .clipShape(.rect(cornerRadius: 6))
        .shadow(color: .black.opacity(0.45), radius: 24, y: 10)
        .scaleEffect(hud.zoom)
        .frame(width: 640 * hud.zoom, height: 480 * hud.zoom)
    }

    private var state: some View {
        HStack(spacing: 18) {
            Text("State")
                .font(.system(size: 11, weight: .semibold))
                .textCase(.uppercase)
                .foregroundStyle(.secondary)
            stateSlider("Shield", value: $hud.shield, range: 0...1)
            stateSlider("Health", value: $hud.health, range: 0...1)
            stateSlider("Rounds", value: $hud.rounds, range: 0...36)
            stateSlider("Grenades", value: $hud.grenades, range: 0...4)
        }
        .font(.system(size: 12))
        .padding(.horizontal, 18)
        .frame(height: 48)
        .glassEffect(.regular, in: .capsule)
    }

    private func stateSlider(_ title: String, value: Binding<Double>, range: ClosedRange<Double>) -> some View {
        HStack(spacing: 6) {
            Text(title).foregroundStyle(.secondary)
            Slider(value: value, in: range)
                .controlSize(.small)
                .frame(width: 100)
        }
    }

    private var inspector: some View {
        GlassEffectContainer(spacing: 10) {
            VStack(spacing: 10) {
                if let index = hud.widgets.firstIndex(where: { $0.id == hud.selection }) {
                    let widget = $hud.widgets[index]
                    GlassCard(title: "Widget", symbol: "square.dashed") {
                        Text(widget.wrappedValue.id)
                            .font(.headline)
                        LabeledContent("Anchor") {
                            Picker("Anchor", selection: widget.anchor) {
                                ForEach(["top left", "top right", "bottom left", "bottom right", "centre"], id: \.self) { Text($0).tag($0) }
                            }
                            .labelsHidden()
                            .fixedSize()
                        }
                        VStack(alignment: .leading, spacing: 4) {
                            Text("Offset (\(hud.screen))")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            HStack(spacing: 6) {
                                NumberField(axis: "X", color: .red, value: widget.origin.x.double, digits: 0)
                                NumberField(axis: "Y", color: .green, value: widget.origin.y.double, digits: 0)
                            }
                        }
                    }
                }
                GlassCard(title: "Checks", symbol: "checkmark.seal") {
                    Label("grenade_count spills out of the art it sits on", systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.orange)
                    Label("No art is off the screen", systemImage: "checkmark.circle.fill")
                        .foregroundStyle(.green)
                    Label("No art half covers other art", systemImage: "checkmark.circle.fill")
                        .foregroundStyle(.green)
                }
                GlassCard {
                    Button("Save map as…", systemImage: "square.and.arrow.down") {}
                        .buttonStyle(.glassProminent)
                        .frame(maxWidth: .infinity)
                }
            }
        }
    }
}

private extension Binding where Value == CGFloat {
    var double: Binding<Double> {
        Binding<Double>(get: { Double(wrappedValue) }, set: { wrappedValue = CGFloat($0) })
    }
}

private struct WidgetShape: View {
    let widget: HUDWidget
    let hud: HUDModel
    private let ink = Color(red: 0.55, green: 0.85, blue: 1.0)

    var body: some View {
        switch widget.shape {
        case .meter:
            ZStack(alignment: .leading) {
                Capsule().fill(ink.opacity(0.18))
                GeometryReader { geo in
                    Capsule().fill(ink.opacity(0.9)).frame(width: geo.size.width * hud.shield)
                }
                Capsule().strokeBorder(ink, lineWidth: 1.5)
            }
        case .box:
            if widget.id == "health_ticks" {
                HStack(spacing: 3) {
                    ForEach(0..<10, id: \.self) { tick in
                        Rectangle().fill(Double(tick) / 10 < hud.health ? ink : ink.opacity(0.2))
                    }
                }
            } else {
                RoundedRectangle(cornerRadius: 6)
                    .fill(ink.opacity(0.15))
                    .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(ink.opacity(0.8), lineWidth: 1.5))
                    .overlay(Capsule().fill(ink.opacity(0.7)).frame(width: 96, height: 10))
            }
        case .counter:
            HStack(spacing: 2) {
                ForEach(0..<36, id: \.self) { round in
                    Capsule().fill(Double(round) < hud.rounds ? ink : ink.opacity(0.18))
                }
            }
        case .text:
            Text("\(Int(hud.grenades))")
                .font(.system(size: 26, weight: .bold, design: .rounded))
                .foregroundStyle(ink)
                .fixedSize()
        case .ring:
            ZStack {
                Circle().fill(ink.opacity(0.12))
                Circle().strokeBorder(ink.opacity(0.9), lineWidth: 2)
                Circle().strokeBorder(ink.opacity(0.4), lineWidth: 1).padding(22)
                Circle().fill(.yellow).frame(width: 6, height: 6)
                Circle().fill(.red).frame(width: 6, height: 6).offset(x: 22, y: -18)
            }
        case .crosshair:
            ZStack {
                Circle().strokeBorder(ink, lineWidth: 2)
                Circle().fill(ink).frame(width: 4, height: 4)
            }
        }
    }
}
