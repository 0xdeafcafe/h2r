import SwiftUI

struct RootView: View {
    @Environment(EditorModel.self) private var model

    var body: some View {
        @Bindable var model = model
        NavigationSplitView {
            SidebarView()
                .navigationSplitViewColumnWidth(min: 236, ideal: 268, max: 340)
        } detail: {
            detail
                .navigationTitle(model.map.name + (model.dirty ? " *" : ""))
                .navigationSubtitle("\(model.tool.title) · \(model.map.source)")
                .toolbar { toolbar }
        }
        .preferredColorScheme(model.appearance.scheme)
    }

    @ViewBuilder private var detail: some View {
        switch model.tool {
        case .scenario:
            ScenarioView()
        case .tags:
            TagsView()
        case .hud:
            HUDView()
        default:
            ContentUnavailableView(
                model.tool.title,
                systemImage: model.tool.symbol,
                description: Text("This mock draws the Scenario, Tags and HUD tools only.")
            )
        }
    }

    /// The window toolbar. On macOS 26 the system draws each group as glass;
    /// `ToolbarSpacer` splits the groups.
    @ToolbarContentBuilder private var toolbar: some ToolbarContent {
        @Bindable var model = model
        ToolbarItemGroup(placement: .navigation) {
            Button("Undo", systemImage: "arrow.uturn.backward") { model.say("undo") }
            Button("Redo", systemImage: "arrow.uturn.forward") { model.say("redo") }
        }
        ToolbarItem(placement: .principal) {
            Picker("Tool", selection: $model.tool.animation(.smooth(duration: 0.3))) {
                ForEach(Tool.allCases.filter(\.isMocked)) { tool in
                    Label(tool.title, systemImage: tool.symbol).tag(tool)
                }
            }
            .pickerStyle(.segmented)
        }
        ToolbarItemGroup(placement: .primaryAction) {
            Button("Save as package…", systemImage: "square.and.arrow.down") {
                model.dirty = false
                model.say("saved package \(model.map.id)-edit (nothing was written: this is a mock)")
            }
            Button("Play", systemImage: "play.fill") {
                model.playing = true
                model.paired = true
                model.say("play: started h2 play on \(model.map.id) with the editor's edits")
            }
            .keyboardShortcut(KeyEquivalent(Character(UnicodeScalar(NSF5FunctionKey)!)), modifiers: [])
            .help("Play the map with your edits (F5)")
        }
        ToolbarSpacer(.fixed, placement: .primaryAction)
        ToolbarItemGroup(placement: .primaryAction) {
            Menu("Appearance", systemImage: "circle.lefthalf.filled") {
                Picker("Appearance", selection: $model.appearance) {
                    ForEach(Appearance.allCases) { Text($0.title).tag($0) }
                }
                .pickerStyle(.inline)
            }
            Button("Inspector", systemImage: "sidebar.trailing") {
                withAnimation(.smooth(duration: 0.3)) { model.showInspector.toggle() }
            }
        }
    }
}

// MARK: - Sidebar

/// Tools, Maps and the Outliner in the window's (glass) sidebar.
struct SidebarView: View {
    @Environment(EditorModel.self) private var model

    var body: some View {
        List {
            Section("Tools") {
                ForEach(Tool.allCases) { tool in
                    SidebarRow(isSelected: model.tool == tool) {
                        withAnimation(.smooth(duration: 0.3)) { model.tool = tool }
                    } label: {
                        Label(tool.title + (tool == .scenario && model.dirty ? " *" : ""), systemImage: tool.symbol)
                    }
                }
            }
            ForEach([MapFamily.halo2, .halo1], id: \.self) { family in
                Section("Maps · \(family.rawValue)") {
                    ForEach(model.maps.filter { $0.family == family }) { map in
                        SidebarRow(isSelected: model.mapID == map.id) {
                            model.open(map)
                        } label: {
                            HStack(spacing: 8) {
                                MapSwatch(map: map)
                                Text(map.name)
                                Spacer(minLength: 0)
                                if let sandbox = map.sandbox {
                                    Text(sandbox)
                                        .font(.caption2.weight(.medium))
                                        .padding(.horizontal, 6)
                                        .padding(.vertical, 2)
                                        .background(.quaternary, in: .capsule)
                                }
                            }
                        }
                        .help(map.source)
                    }
                }
            }
            switch model.tool {
            case .hud: HUDWidgetList()
            case .tags: EmptyView()
            default: outliner
            }
        }
        .listStyle(.sidebar)
        .scrollEdgeEffectStyle(.soft, for: .top)
    }

    @ViewBuilder private var outliner: some View {
        @Bindable var model = model
        Section("Outliner") {
            HStack(spacing: 6) {
                Image(systemName: "line.3.horizontal.decrease")
                    .foregroundStyle(.secondary)
                TextField("Filter placements", text: $model.filter)
                    .textFieldStyle(.plain)
                Button("Duplicate", systemImage: "plus.square.on.square") { model.duplicateSelection() }
                    .disabled(model.selection == nil)
                Button("Delete", systemImage: "trash") { model.deleteSelection() }
                    .disabled(model.selection == nil)
            }
            .labelStyle(.iconOnly)
            .buttonStyle(.borderless)
            .padding(.horizontal, 8)
            .frame(height: 28)
            .background(.quaternary.opacity(0.6), in: .rect(cornerRadius: 8))

            ForEach(PlacementKind.allCases.filter { model.count(of: $0) > 0 }) { kind in
                let shown = model.placements(of: kind)
                DisclosureGroup(isExpanded: Binding(
                    get: { model.expanded.contains(kind) || (!model.filter.isEmpty && !shown.isEmpty) },
                    set: { open in
                        if open { model.expanded.insert(kind) } else { model.expanded.remove(kind) }
                    }
                )) {
                    ForEach(shown) { placement in
                        SidebarRow(isSelected: model.selection == placement.id) {
                            model.select(placement.id)
                        } label: {
                            HStack(spacing: 6) {
                                Text("\(placement.index)")
                                    .font(.caption.monospacedDigit())
                                    .foregroundStyle(.secondary)
                                    .frame(width: 18, alignment: .trailing)
                                Text(placement.entry)
                                Spacer(minLength: 0)
                                if placement.team != .neutral {
                                    Circle().fill(placement.team.color).frame(width: 7, height: 7)
                                }
                            }
                        }
                    }
                } label: {
                    Label {
                        Text(kind.title)
                    } icon: {
                        Image(systemName: kind.symbol).foregroundStyle(kind.color)
                    }
                    .badge(model.count(of: kind))
                }
            }
        }
    }
}

struct SidebarRow<Label: View>: View {
    let isSelected: Bool
    let action: () -> Void
    @ViewBuilder var label: Label

    var body: some View {
        Button(action: action) {
            label
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .foregroundStyle(isSelected ? AnyShapeStyle(.white) : AnyShapeStyle(.primary))
        .listRowBackground(
            RoundedRectangle(cornerRadius: 9, style: .continuous)
                .fill(isSelected ? AnyShapeStyle(Color.accentColor.gradient) : AnyShapeStyle(.clear))
                .padding(.horizontal, 8)
        )
    }
}

/// A stand-in for a map's menu picture: a gradient picked from its name.
struct MapSwatch: View {
    let map: MapEntry

    var body: some View {
        var rng = SeededRNG(seed: map.file)
        let hue = Double.random(in: 0...1, using: &rng)
        return RoundedRectangle(cornerRadius: 5, style: .continuous)
            .fill(
                LinearGradient(
                    colors: [Color(hue: hue, saturation: 0.55, brightness: 0.85), Color(hue: (hue + 0.12).truncatingRemainder(dividingBy: 1), saturation: 0.7, brightness: 0.45)],
                    startPoint: .topLeading, endPoint: .bottomTrailing
                )
            )
            .overlay(alignment: .bottom) {
                Image(systemName: map.family == .halo1 ? "mountain.2.fill" : "building.2.fill")
                    .font(.system(size: 8))
                    .foregroundStyle(.white.opacity(0.75))
                    .padding(.bottom, 2)
            }
            .frame(width: 26, height: 18)
    }
}
