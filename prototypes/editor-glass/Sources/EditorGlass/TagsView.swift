import SwiftUI

// The Tags tool: a group tree with search on the left, the selected tag's
// fields in the middle, and its data and resources on the right. All made up.

struct MockTag: Identifiable, Hashable, Sendable {
    let group: String
    let path: String
    var mark: String?
    var edited = false
    var readOnly = false

    var id: String { "\(group):\(path)" }
    var name: String { path.split(separator: "\\").last.map(String.init) ?? path }

    static let all: [MockTag] = [
        MockTag(group: "weap", path: "objects\\weapons\\rifle\\battle_rifle\\battle_rifle", edited: true),
        MockTag(group: "weap", path: "objects\\weapons\\rifle\\sniper_rifle\\sniper_rifle"),
        MockTag(group: "weap", path: "objects\\weapons\\rifle\\shotgun\\shotgun"),
        MockTag(group: "weap", path: "objects\\weapons\\support_high\\rocket_launcher\\rocket_launcher"),
        MockTag(group: "weap", path: "objects\\weapons\\melee\\energy_blade\\energy_blade"),
        MockTag(group: "proj", path: "objects\\weapons\\rifle\\battle_rifle\\projectiles\\battle_rifle_bullet"),
        MockTag(group: "proj", path: "objects\\weapons\\support_high\\rocket_launcher\\projectiles\\rocket"),
        MockTag(group: "vehi", path: "objects\\vehicles\\warthog\\warthog"),
        MockTag(group: "vehi", path: "objects\\vehicles\\ghost\\ghost"),
        MockTag(group: "bitm", path: "objects\\weapons\\rifle\\battle_rifle\\bitmaps\\battle_rifle", mark: "[shared]"),
        MockTag(group: "bitm", path: "ui\\hud\\bitmaps\\new_hud\\crosshairs\\battle_rifle", mark: "[shared]"),
        MockTag(group: "bitm", path: "scenarios\\bitmaps\\multi\\lockout\\ice_wall"),
        MockTag(group: "mode", path: "objects\\weapons\\rifle\\battle_rifle\\battle_rifle", mark: "[shared]"),
        MockTag(group: "snd!", path: "sound\\weapons\\battle_rifle\\fire", mark: "[shared]"),
        MockTag(group: "nhdt", path: "ui\\hud\\masterchief", mark: "[in shared]"),
        MockTag(group: "scnr", path: "scenarios\\multi\\lockout\\lockout", readOnly: true),
        MockTag(group: "sbsp", path: "scenarios\\multi\\lockout\\lockout", readOnly: true),
    ]
}

enum MockFieldValue: Sendable {
    case real(Double)
    case integer(Int)
    case angle(Double)
    case choice(String, [String])
    case flags([String], Set<Int>)
    case reference(String, String)
    case color(Double)
    case stringID(String)
    case unknown(String)
}

struct MockField: Identifiable, Sendable {
    let offset: Int
    let name: String?
    var value: MockFieldValue
    var id: Int { offset }

    static func fields(for tag: MockTag) -> [MockField] {
        var rng = SeededRNG(seed: tag.id)
        var offset = 0
        var out: [MockField] = []
        func add(_ name: String?, _ value: MockFieldValue, size: Int = 4) {
            out.append(MockField(offset: offset, name: name, value: value))
            offset += size
        }
        add("flags", .flags(["must be readied", "does not depress firing point", "aim assists only when zoomed", "prevents melee attack"], [0, 2]))
        add("bounding radius", .real(Double.random(in: 0.2...1.5, using: &rng)))
        add("default model variant", .stringID("default"))
        add("model", .reference("hlmt", tag.path))
        add(nil, .unknown(String(format: "0x%08X", UInt32.random(in: 0...UInt32.max, using: &rng))))
        add("autoaim angle", .angle(Double.random(in: 1...8, using: &rng)))
        add("autoaim range", .real(Double.random(in: 10...120, using: &rng)))
        add("magnetism angle", .angle(Double.random(in: 1...12, using: &rng)))
        add("magnetism range", .real(Double.random(in: 5...60, using: &rng)))
        add("deviation angle", .angle(Double.random(in: 0...3, using: &rng)))
        add(nil, .unknown("\(Int.random(in: 0...400, using: &rng))"))
        add(nil, .unknown(String(format: "%.4f", Double.random(in: 0...1, using: &rng))))
        add("movement penalized", .choice("when zoomed", ["always", "when zoomed", "when zoomed or reloading"]))
        add("forward movement penalty", .real(Double.random(in: 0...0.4, using: &rng)))
        add("magazine rounds loaded", .integer(Int.random(in: 4...60, using: &rng)), size: 2)
        add("magazine rounds total", .integer(Int.random(in: 20...180, using: &rng)), size: 2)
        add("first person tint", .color(Double.random(in: 0...1, using: &rng)))
        add("HUD interface", .reference("nhdt", "ui\\hud\\\(tag.name)"))
        add("pickup sound", .reference("snd!", "sound\\weapons\\\(tag.name)\\pickup"))
        add(nil, .unknown(String(format: "0x%08X", UInt32.random(in: 0...UInt32.max, using: &rng))))
        add("heat recovery threshold", .real(Double.random(in: 0...1, using: &rng)))
        add("overheated threshold", .real(Double.random(in: 0...1, using: &rng)))
        return out
    }
}

struct TagsView: View {
    @Environment(EditorModel.self) private var model
    @State private var search = ""
    @State private var byPaths = false
    @State private var selection: MockTag.ID? = MockTag.all[0].id
    @State private var withReferences = true
    @State private var withResources = false

    private var shown: [MockTag] {
        let words = search.lowercased().split(separator: " ")
        return MockTag.all.filter { tag in
            words.allSatisfy { tag.group.contains($0) || tag.path.lowercased().contains($0) }
        }
    }

    private var tag: MockTag? { MockTag.all.first { $0.id == selection } }

    var body: some View {
        HStack(spacing: 0) {
            tree
                .frame(width: 300)
            Divider()
            fields
                .frame(maxWidth: .infinity)
            Divider()
            dataColumn
                .frame(width: 320)
        }
        .background(.background)
    }

    // MARK: Tree

    private var tree: some View {
        VStack(spacing: 8) {
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                TextField("Search: weap rifle", text: $search)
                    .textFieldStyle(.plain)
            }
            .padding(.horizontal, 10)
            .frame(height: 30)
            .background(.quaternary.opacity(0.7), in: .capsule)
            Picker("Tree", selection: $byPaths) {
                Text("Groups").tag(false)
                Text("Paths").tag(true)
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            List(selection: $selection) {
                let keys = byPaths
                    ? orderedKeys(shown.map { String($0.path.split(separator: "\\").first ?? "") })
                    : orderedKeys(shown.map(\.group))
                ForEach(keys, id: \.self) { key in
                    let tags = shown.filter { byPaths ? $0.path.hasPrefix(key + "\\") : $0.group == key }
                    DisclosureGroup {
                        ForEach(tags) { tag in
                            HStack(spacing: 6) {
                                Text(tag.name + (tag.edited ? " *" : ""))
                                    .lineLimit(1)
                                Spacer(minLength: 0)
                                if byPaths {
                                    Text(tag.group).font(.caption.monospaced()).foregroundStyle(.secondary)
                                }
                                if let mark = tag.mark {
                                    Text(mark).font(.caption2).foregroundStyle(.secondary)
                                }
                                if tag.readOnly {
                                    Image(systemName: "lock.fill").font(.caption2).foregroundStyle(.secondary)
                                }
                            }
                            .tag(tag.id)
                            .help(tag.path)
                        }
                    } label: {
                        Label(key, systemImage: byPaths ? "folder" : "tag")
                            .font(byPaths ? .body : .body.monospaced())
                            .badge(tags.count)
                    }
                }
            }
            .listStyle(.sidebar)
            .scrollContentBackground(.hidden)
        }
        .padding(.horizontal, 10)
        .padding(.top, 10)
        .background(.regularMaterial)
    }

    private func orderedKeys(_ keys: [String]) -> [String] {
        var seen = Set<String>()
        return keys.filter { seen.insert($0).inserted }
    }

    // MARK: Fields

    private var fields: some View {
        VStack(spacing: 0) {
            if let tag {
                ScrollView {
                    VStack(spacing: 0) {
                        ForEach(MockField.fields(for: tag)) { field in
                            FieldLine(field: field, readOnly: tag.readOnly)
                            Divider().opacity(0.5)
                        }
                    }
                    .padding(.horizontal, 18)
                    .padding(.bottom, 18)
                }
                .scrollEdgeEffectStyle(.soft, for: .top)
                .safeAreaBar(edge: .top) {
                    header(tag)
                }
            } else {
                ContentUnavailableView("No tag selected", systemImage: "tag", description: Text("Pick a tag in the tree."))
            }
        }
    }

    /// The bar over the fields: the tag's name, then Export and Import as glass buttons.
    private func header(_ tag: MockTag) -> some View {
        HStack(spacing: 10) {
            Text(tag.group)
                .font(.system(size: 12, weight: .bold, design: .monospaced))
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .background(Color.accentColor.opacity(0.25), in: .rect(cornerRadius: 6))
            VStack(alignment: .leading, spacing: 1) {
                Text(tag.name).font(.headline)
                Text(tag.path)
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.head)
            }
            Spacer()
            GlassEffectContainer(spacing: 8) {
                HStack(spacing: 8) {
                    Toggle("references", isOn: $withReferences)
                    Toggle("resources", isOn: $withResources)
                    Button("Export…", systemImage: "square.and.arrow.up") {
                        model.say("export: \(tag.name).h2tag (nothing was written: this is a mock)")
                    }
                    .buttonStyle(.glassProminent)
                    Button("Import over this tag…", systemImage: "square.and.arrow.down") {
                        model.say("import over \(tag.group) \(tag.name): one undo step")
                    }
                    .buttonStyle(.glass)
                    .disabled(tag.readOnly)
                }
                .toggleStyle(.checkbox)
                .font(.system(size: 12))
            }
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 10)
    }

    // MARK: Data and bytes

    private var dataColumn: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                if let tag {
                    section("Data") {
                        DataRow(name: "function data", detail: "local", size: "20 B")
                        DataRow(name: "predicted resources", detail: "local", size: "64 B")
                    }
                    section("Resources") {
                        DataRow(name: tag.group == "snd!" ? "sound samples" : tag.group == "mode" ? "model geometry" : "pixels", detail: tag.mark == nil ? "local" : "shared", size: "87.4 KB")
                        DataRow(name: "mip chain", detail: tag.mark == nil ? "local" : "shared", size: "29.1 KB")
                        if tag.group == "bitm" {
                            BitmapPlaceholder(seed: tag.id)
                        }
                    }
                    section("Bytes") {
                        HexDump(seed: tag.id)
                    }
                }
            }
            .padding(14)
        }
        .background(.regularMaterial)
    }

    private func section(_ title: String, @ViewBuilder content: () -> some View) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title)
                .font(.system(size: 11, weight: .semibold))
                .textCase(.uppercase)
                .foregroundStyle(.secondary)
            content()
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.thinMaterial, in: .rect(cornerRadius: 14))
    }
}

private struct FieldLine: View {
    let field: MockField
    let readOnly: Bool
    @State private var number = 0.0
    @State private var text = ""
    @State private var choice = ""
    @State private var bits: Set<Int> = []

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text(field.name ?? String(format: "+0x%02X", field.offset))
                .font(field.name == nil ? .system(size: 12, design: .monospaced) : .system(size: 12.5))
                .foregroundStyle(field.name == nil ? .secondary : .primary)
                .frame(width: 210, alignment: .leading)
                .help(String(format: "offset +0x%02X", field.offset))
            editor
            Spacer(minLength: 0)
        }
        .padding(.vertical, 7)
        .disabled(readOnly)
        .onAppear(perform: load)
        .onChange(of: field.id, load)
    }

    private func load() {
        switch field.value {
        case .real(let v), .angle(let v): number = v
        case .integer(let v): number = Double(v)
        case .choice(let v, _): choice = v
        case .flags(_, let on): bits = on
        case .stringID(let v), .unknown(let v): text = v
        case .reference(_, let path): text = path
        case .color: break
        }
    }

    @ViewBuilder private var editor: some View {
        switch field.value {
        case .real:
            boxed { TextField("", value: $number, format: .number.precision(.fractionLength(3))).frame(width: 90) }
        case .integer:
            boxed { TextField("", value: $number, format: .number.precision(.fractionLength(0))).frame(width: 90) }
        case .angle:
            boxed {
                TextField("", value: $number, format: .number.precision(.fractionLength(2))).frame(width: 78)
                Text("°").foregroundStyle(.secondary)
            }
        case .choice(_, let all):
            Picker("", selection: $choice) {
                ForEach(all, id: \.self) { Text($0).tag($0) }
            }
            .labelsHidden()
            .fixedSize()
        case .flags(let names, _):
            VStack(alignment: .leading, spacing: 4) {
                ForEach(Array(names.enumerated()), id: \.offset) { bit, name in
                    Toggle(name, isOn: Binding(
                        get: { bits.contains(bit) },
                        set: { on in
                            if on { bits.insert(bit) } else { bits.remove(bit) }
                        }
                    ))
                    .toggleStyle(.checkbox)
                }
            }
        case .reference(let group, _):
            HStack(spacing: 6) {
                Text(group)
                    .font(.system(size: 11, weight: .semibold, design: .monospaced))
                    .foregroundStyle(.secondary)
                boxed { TextField("none", text: $text).frame(minWidth: 220, maxWidth: 380) }
                Button("Jump to tag", systemImage: "arrow.right.circle") {}
                    .labelStyle(.iconOnly)
                    .buttonStyle(.borderless)
            }
        case .color(let hue):
            ColorPicker("", selection: .constant(Color(hue: hue, saturation: 0.6, brightness: 0.9)))
                .labelsHidden()
        case .stringID:
            boxed { TextField("", text: $text).frame(width: 160) }
        case .unknown:
            Text(text)
                .font(.system(size: 12, design: .monospaced))
                .foregroundStyle(.secondary)
        }
    }

    private func boxed(@ViewBuilder _ content: () -> some View) -> some View {
        HStack(spacing: 3) { content() }
            .textFieldStyle(.plain)
            .font(.system(size: 12).monospacedDigit())
            .padding(.horizontal, 8)
            .frame(height: 24)
            .background(.quaternary.opacity(0.7), in: .rect(cornerRadius: 6))
    }
}

private struct DataRow: View {
    let name: String
    let detail: String
    let size: String

    var body: some View {
        HStack {
            Text(name)
            Spacer()
            Text(detail)
                .font(.caption2.weight(.medium))
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(.quaternary, in: .capsule)
            Text(size)
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: 58, alignment: .trailing)
        }
        .font(.system(size: 12))
    }
}

/// Where a bitmap's Preview would go: a gradient over a checkerboard, never art.
private struct BitmapPlaceholder: View {
    let seed: String

    var body: some View {
        var rng = SeededRNG(seed: seed)
        let hue = Double.random(in: 0...1, using: &rng)
        return Canvas { context, size in
            let cell = 12.0
            for row in 0..<Int(size.height / cell + 1) {
                for column in 0..<Int(size.width / cell + 1) where (row + column) % 2 == 0 {
                    context.fill(
                        Path(CGRect(x: Double(column) * cell, y: Double(row) * cell, width: cell, height: cell)),
                        with: .color(.gray.opacity(0.25))
                    )
                }
            }
        }
        .overlay(
            LinearGradient(
                colors: [Color(hue: hue, saturation: 0.5, brightness: 0.9).opacity(0.8), Color(hue: hue, saturation: 0.7, brightness: 0.4).opacity(0.4)],
                startPoint: .topLeading, endPoint: .bottomTrailing
            )
        )
        .overlay {
            Label("Preview (placeholder)", systemImage: "photo")
                .font(.caption)
                .foregroundStyle(.white)
        }
        .frame(height: 120)
        .clipShape(.rect(cornerRadius: 10))
    }
}

private struct HexDump: View {
    let seed: String

    var body: some View {
        var rng = SeededRNG(seed: seed + "bytes")
        let rows = (0..<10).map { _ in (0..<8).map { _ in UInt8.random(in: 0...255, using: &rng) } }
        return VStack(alignment: .leading, spacing: 2) {
            ForEach(Array(rows.enumerated()), id: \.offset) { row, bytes in
                HStack(spacing: 5) {
                    Text(String(format: "%04X", row * 8))
                        .foregroundStyle(.tertiary)
                    ForEach(Array(bytes.enumerated()), id: \.offset) { column, byte in
                        Text(String(format: "%02X", byte))
                            .foregroundStyle(row == 2 && column < 4 ? AnyShapeStyle(.blue) : row == 5 && column == 6 ? AnyShapeStyle(.orange) : AnyShapeStyle(.primary))
                    }
                }
            }
        }
        .font(.system(size: 11, design: .monospaced))
    }
}
