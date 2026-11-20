import SwiftUI

// Everything here is made-up data. Nothing is read from a disc or a map file.

enum Tool: String, CaseIterable, Identifiable, Sendable {
    case scenario, tags, variants, playlists, uiScreens, hud

    var id: String { rawValue }

    var title: String {
        switch self {
        case .scenario: "Scenario"
        case .tags: "Tags"
        case .variants: "Game variants"
        case .playlists: "Playlists"
        case .uiScreens: "UI Screens"
        case .hud: "HUD"
        }
    }

    var symbol: String {
        switch self {
        case .scenario: "cube.transparent"
        case .tags: "tag"
        case .variants: "slider.horizontal.3"
        case .playlists: "list.bullet.rectangle"
        case .uiScreens: "rectangle.on.rectangle"
        case .hud: "viewfinder"
        }
    }

    /// The tools this mock draws. The others show a placeholder.
    var isMocked: Bool { self == .scenario || self == .tags || self == .hud }
}

enum MapFamily: String, Sendable {
    case halo2 = "Halo 2"
    case halo1 = "Halo 1"
}

struct MapEntry: Identifiable, Hashable, Sendable {
    let id: String
    let name: String
    let file: String
    let family: MapFamily
    /// "H1 sandbox" or "H2 sandbox" for a converted Halo 1 map.
    let sandbox: String?

    var source: String {
        switch family {
        case .halo2: "\(file), on the Halo 2 disc"
        case .halo1: "\(file), converted from Halo 1"
        }
    }

    static let all: [MapEntry] = {
        let halo2 = [
            ("Lockout", "lockout"), ("Ascension", "ascension"), ("Midship", "midship"),
            ("Zanzibar", "zanzibar"), ("Beaver Creek", "beavercreek"), ("Headlong", "headlong"),
            ("Coagulation", "coagulation"), ("Ivory Tower", "cyclotron"),
        ].map { MapEntry(id: $0.1, name: $0.0, file: "\($0.1).map", family: .halo2, sandbox: nil) }
        let halo1 = [
            ("Blood Gulch", "bloodgulch"), ("Hang 'Em High", "hangemhigh"), ("Sidewinder", "sidewinder"),
        ].flatMap { map in
            ["H1", "H2"].map { sandbox in
                MapEntry(
                    id: "\(map.1)-\(sandbox.lowercased())",
                    name: map.0,
                    file: "\(map.1).map",
                    family: .halo1,
                    sandbox: "\(sandbox) sandbox"
                )
            }
        }
        return halo2 + halo1
    }()
}

enum PlacementKind: String, CaseIterable, Identifiable, Sendable {
    case scenery, vehicles, equipment, weapons, machines, crates
    case playerStarts, netgameFlags, netgameEquipment

    var id: String { rawValue }

    var title: String {
        switch self {
        case .scenery: "Scenery"
        case .vehicles: "Vehicles"
        case .equipment: "Equipment"
        case .weapons: "Weapons"
        case .machines: "Machines"
        case .crates: "Crates"
        case .playerStarts: "Player starting locations"
        case .netgameFlags: "Netgame flags"
        case .netgameEquipment: "Netgame equipment"
        }
    }

    var symbol: String {
        switch self {
        case .scenery: "mountain.2"
        case .vehicles: "car.side"
        case .equipment: "cross.case"
        case .weapons: "scope"
        case .machines: "gearshape.2"
        case .crates: "shippingbox"
        case .playerStarts: "figure.stand"
        case .netgameFlags: "flag"
        case .netgameEquipment: "arrow.triangle.2.circlepath"
        }
    }

    var color: Color {
        switch self {
        case .scenery: Color(red: 0.62, green: 0.60, blue: 0.56)
        case .vehicles: Color(red: 0.35, green: 0.62, blue: 0.95)
        case .equipment: Color(red: 0.45, green: 0.85, blue: 0.75)
        case .weapons: Color(red: 0.98, green: 0.78, blue: 0.30)
        case .machines: Color(red: 0.70, green: 0.55, blue: 0.95)
        case .crates: Color(red: 0.80, green: 0.60, blue: 0.40)
        case .playerStarts: Color(red: 0.45, green: 0.90, blue: 0.50)
        case .netgameFlags: Color(red: 0.98, green: 0.42, blue: 0.42)
        case .netgameEquipment: Color(red: 0.98, green: 0.60, blue: 0.25)
        }
    }

    /// Half extents of the placeholder block, in world units.
    var half: Vec3 {
        switch self {
        case .scenery: Vec3(0.55, 0.55, 0.55)
        case .vehicles: Vec3(1.15, 0.65, 0.45)
        case .equipment: Vec3(0.22, 0.22, 0.22)
        case .weapons: Vec3(0.50, 0.12, 0.12)
        case .machines: Vec3(0.45, 0.45, 0.85)
        case .crates: Vec3(0.40, 0.40, 0.40)
        case .playerStarts: Vec3(0.22, 0.22, 0.50)
        case .netgameFlags: Vec3(0.12, 0.12, 0.75)
        case .netgameEquipment: Vec3(0.32, 0.32, 0.14)
        }
    }

    var palette: [String] {
        switch self {
        case .scenery: ["rock_large", "barricade_small", "light_fixture", "antenna_mast", "tree_pine"]
        case .vehicles: ["warthog", "ghost", "banshee", "scorpion"]
        case .equipment: ["frag_grenade", "plasma_grenade", "overshield", "active_camouflage"]
        case .weapons: ["battle_rifle", "sniper_rifle", "shotgun", "rocket_launcher", "energy_sword", "smg"]
        case .machines: ["door_blast", "lift_gravity", "switch_panel"]
        case .crates: ["crate_tech", "crate_supply", "barrel_fusion"]
        case .playerStarts: ["player start"]
        case .netgameFlags: ["CTF flag", "Hill", "Bomb spot", "Oddball spawn", "Territory", "Teleporter"]
        case .netgameEquipment: ["battle_rifle", "plasma_rifle", "needler", "sniper_rifle", "frag_grenade", "rocket_launcher"]
        }
    }

    var hasTeam: Bool { self == .playerStarts || self == .netgameFlags || self == .netgameEquipment }
    var hasScale: Bool { self == .scenery || self == .vehicles || self == .machines || self == .crates }
}

enum Team: String, CaseIterable, Identifiable, Sendable {
    case neutral = "Neutral", red = "Red", blue = "Blue", yellow = "Yellow", green = "Green"
    var id: String { rawValue }
    var color: Color {
        switch self {
        case .neutral: .gray
        case .red: .red
        case .blue: .blue
        case .yellow: .yellow
        case .green: .green
        }
    }
}

struct SpawnFlag: Identifiable, Hashable, Sendable {
    let id: String
    var isOn: Bool
}

struct Placement: Identifiable, Hashable, Sendable {
    let id: Int
    var kind: PlacementKind
    /// Index inside its group, as the outliner numbers it.
    var index: Int
    var entry: String
    var position: Vec3
    var yaw: Double
    var pitch: Double = 0
    var roll: Double = 0
    var scale: Double = 1
    var team: Team = .neutral
    var respawnSeconds: Double = 30
    var flags: [SpawnFlag]
    var scriptNamed = false

    var label: String { "\(index): \(entry)" }

    var box: SceneBox {
        let h = kind.half * scale
        return SceneBox(
            center: Vec3(position.x, position.y, position.z + h.z),
            half: h,
            yaw: yaw * .pi / 180,
            color: kind.color,
            id: id
        )
    }
}

struct MockScene: Sendable {
    var slabs: [SceneBox]
    var placements: [Placement]
    var triangles: Int

    /// A deterministic made-up layout for a map name: a floor plate, two bases,
    /// a middle structure, and placements scattered over the plate.
    static func make(for map: MapEntry) -> MockScene {
        var rng = SeededRNG(seed: map.file)
        let stone = Color(red: 0.46, green: 0.49, blue: 0.55)
        let metal = Color(red: 0.36, green: 0.40, blue: 0.48)
        let floorTop = 0.3
        var slabs: [SceneBox] = [
            SceneBox(center: Vec3(0, 0, floorTop / 2), half: Vec3(15, 10, floorTop / 2), yaw: 0, color: stone, id: nil, isGround: true)
        ]
        let baseDepth = Double.random(in: 2.4...3.6, using: &rng)
        for side in [-1.0, 1.0] {
            slabs.append(SceneBox(center: Vec3(side * 12, 0, floorTop + 1.0), half: Vec3(2.2, baseDepth, 1.0), yaw: 0, color: metal, id: nil))
            slabs.append(SceneBox(center: Vec3(side * 12.6, 0, floorTop + 2.6), half: Vec3(1.2, baseDepth * 0.55, 0.6), yaw: 0, color: stone, id: nil))
        }
        let towerYaw = Double.random(in: 0...(Double.pi / 4), using: &rng)
        slabs.append(SceneBox(center: Vec3(0, 0, floorTop + 0.6), half: Vec3(2.4, 2.4, 0.6), yaw: towerYaw, color: metal, id: nil))
        slabs.append(SceneBox(center: Vec3(0, 0, floorTop + 2.0), half: Vec3(1.1, 1.1, 0.8), yaw: towerYaw, color: stone, id: nil))
        for _ in 0..<Int.random(in: 3...5, using: &rng) {
            let x = Double.random(in: -8...8, using: &rng)
            let y = (Bool.random(using: &rng) ? 1.0 : -1.0) * Double.random(in: 5...8.5, using: &rng)
            let long = Bool.random(using: &rng)
            slabs.append(SceneBox(
                center: Vec3(x, y, floorTop + 0.7),
                half: Vec3(long ? 1.8 : 0.3, long ? 0.3 : 1.4, 0.7),
                yaw: 0, color: stone, id: nil
            ))
        }

        func clear(_ p: Vec3) -> Bool {
            slabs.dropFirst().allSatisfy { slab in
                abs(p.x - slab.center.x) > slab.half.x + 1.4 || abs(p.y - slab.center.y) > slab.half.y + 1.4
            }
        }

        let counts: [(PlacementKind, Int)] = [
            (.scenery, Int.random(in: 5...8, using: &rng)),
            (.vehicles, map.family == .halo1 ? 4 : Int.random(in: 0...3, using: &rng)),
            (.equipment, Int.random(in: 2...4, using: &rng)),
            (.weapons, Int.random(in: 3...6, using: &rng)),
            (.machines, Int.random(in: 0...2, using: &rng)),
            (.crates, Int.random(in: 2...5, using: &rng)),
            (.playerStarts, Int.random(in: 8...12, using: &rng)),
            (.netgameFlags, Int.random(in: 4...7, using: &rng)),
            (.netgameEquipment, Int.random(in: 8...14, using: &rng)),
        ]
        var placements: [Placement] = []
        var nextID = 0
        for (kind, count) in counts {
            for index in 0..<count {
                var p = Vec3(0, 0, floorTop)
                for _ in 0..<40 {
                    p = Vec3(Double.random(in: -13.5...13.5, using: &rng), Double.random(in: -9...9, using: &rng), floorTop)
                    if clear(p) { break }
                }
                p.x = (p.x * 4).rounded() / 4
                p.y = (p.y * 4).rounded() / 4
                let team: Team = kind.hasTeam ? (p.x < -4 ? .red : p.x > 4 ? .blue : .neutral) : .neutral
                placements.append(Placement(
                    id: nextID,
                    kind: kind,
                    index: index,
                    entry: kind.palette.randomElement(using: &rng)!,
                    position: p,
                    yaw: Double(Int.random(in: 0..<24, using: &rng)) * 15,
                    team: team,
                    respawnSeconds: [15, 30, 45, 60, 120].randomElement(using: &rng)!,
                    flags: Self.flags(for: kind, using: &rng),
                    scriptNamed: kind == .vehicles && index == 0
                ))
                nextID += 1
            }
        }
        return MockScene(slabs: slabs, placements: placements, triangles: Int.random(in: 18_000...64_000, using: &rng))
    }

    static func flags(for kind: PlacementKind, using rng: inout SeededRNG) -> [SpawnFlag] {
        let names: [String]
        switch kind {
        case .playerStarts: names = ["Slayer", "CTF", "Oddball", "King of the Hill", "Juggernaut", "Territories", "Assault"]
        case .netgameFlags: names = ["Multiple flag or bomb", "Single flag or bomb", "Neutral flag or bomb"]
        case .netgameEquipment: names = ["Levitate", "Destroy existing on new spawn"]
        default: names = ["Not automatically", "Not on easy", "Not on normal", "Not on hard"]
        }
        return names.map { SpawnFlag(id: $0, isOn: kind == .playerStarts ? Int.random(in: 0..<4, using: &rng) > 0 : Int.random(in: 0..<4, using: &rng) == 0) }
    }
}

enum GizmoMode: String, CaseIterable, Identifiable, Sendable {
    case move, rotate, scale
    var id: String { rawValue }
    var title: String { rawValue.capitalized }
    var key: String {
        switch self {
        case .move: "W"
        case .rotate: "E"
        case .scale: "R"
        }
    }
    var symbol: String {
        switch self {
        case .move: "arrow.up.and.down.and.arrow.left.and.right"
        case .rotate: "arrow.triangle.2.circlepath"
        case .scale: "arrow.up.left.and.arrow.down.right"
        }
    }
}

enum Appearance: String, CaseIterable, Identifiable, Sendable {
    case system, dark, light
    var id: String { rawValue }
    var title: String { rawValue.capitalized }
    var scheme: ColorScheme? {
        switch self {
        case .system: nil
        case .dark: .dark
        case .light: .light
        }
    }
}

enum BottomTab: String, CaseIterable, Identifiable, Sendable {
    case game = "Game", log = "Log"
    var id: String { rawValue }
}

struct LogLine: Identifiable, Sendable {
    enum Level: Sendable { case info, warning, error }
    let id: Int
    let time: String
    let text: String
    var level: Level = .info
}

struct LaunchOptions: Sendable {
    var snapshot: String?
    var tool: Tool = .scenario
    var appearance: Appearance = .dark
    var map: String?
    var gizmo: GizmoMode = .move
    var overlay = false

    static let shared = parse(Array(CommandLine.arguments.dropFirst()))

    static func parse(_ args: [String]) -> LaunchOptions {
        var options = LaunchOptions()
        var index = 0
        func value() -> String? {
            index += 1
            return index < args.count ? args[index] : nil
        }
        while index < args.count {
            switch args[index] {
            case "--snapshot": options.snapshot = value()
            case "--tool": options.tool = value().flatMap(Tool.init(rawValue:)) ?? options.tool
            case "--appearance": options.appearance = value().flatMap(Appearance.init(rawValue:)) ?? options.appearance
            case "--map": options.map = value()
            case "--gizmo": options.gizmo = value().flatMap(GizmoMode.init(rawValue:)) ?? options.gizmo
            case "--overlay": options.overlay = true
            default: break
            }
            index += 1
        }
        return options
    }
}

@MainActor @Observable
final class EditorModel {
    var tool: Tool
    var appearance: Appearance
    let maps = MapEntry.all
    var mapID: String
    var scene: MockScene {
        didSet { if !loading { dirty = true } }
    }
    private var loading = false
    var selection: Placement.ID?
    var filter = ""
    var expanded: Set<PlacementKind> = [.weapons, .netgameFlags]

    var gizmo: GizmoMode
    var localAxes = false
    var gridSnap = true
    var gridStep = 0.25
    var angleSnap = false
    var angleStep = 15.0
    var uniformScale = true
    var h1Overlay: Bool
    var camera = Camera.home

    var showInspector = true
    var bottomTab: BottomTab = .game
    var paired = true
    var autoPair = true
    var playing = true
    var tick = 812
    var spawnAt = "The level's own spawn"
    var dirty = false
    var log: [LogLine] = []
    var status = ""

    init(options: LaunchOptions = .shared) {
        tool = options.tool
        appearance = options.appearance
        gizmo = options.gizmo
        let map = MapEntry.all.first { $0.id == options.map } ?? MapEntry.all[0]
        mapID = map.id
        h1Overlay = options.overlay && map.family == .halo1
        scene = MockScene.make(for: map)
        selection = scene.placements.first { $0.kind == .weapons }?.id
        say("h2-editor glass mock: no disc is read, every value is made up")
        say("found 1 game: h2 play pid 1234 (\(map.id))")
        say("paired to h2 play pid 1234")
        opened(map)
    }

    var map: MapEntry { maps.first { $0.id == mapID } ?? maps[0] }

    var selectedIndex: Int? {
        guard let selection else { return nil }
        return scene.placements.firstIndex { $0.id == selection }
    }

    var selected: Placement? { selectedIndex.map { scene.placements[$0] } }

    func placements(of kind: PlacementKind) -> [Placement] {
        let needle = filter.trimmingCharacters(in: .whitespaces).lowercased()
        return scene.placements.filter { $0.kind == kind && (needle.isEmpty || $0.label.lowercased().contains(needle)) }
    }

    func count(of kind: PlacementKind) -> Int {
        scene.placements.count { $0.kind == kind }
    }

    func say(_ text: String, level: LogLine.Level = .info) {
        let seconds = 41 + log.count * 3
        let time = String(format: "14:%02d:%02d", 2 + seconds / 60, seconds % 60)
        log.append(LogLine(id: log.count, time: time, text: text, level: level))
        status = text
    }

    private func opened(_ map: MapEntry) {
        say("opened \(map.id): \(scene.triangles.formatted()) triangles, \(scene.placements.count) placements, 100% of its art drawn")
        if map.family == .halo1 {
            say("\(map.id): no lightmaps yet, lit by a plain sun", level: .warning)
        }
    }

    func open(_ map: MapEntry) {
        mapID = map.id
        loading = true
        scene = MockScene.make(for: map)
        loading = false
        selection = nil
        dirty = false
        if map.family != .halo1 { h1Overlay = false }
        camera = .home
        opened(map)
    }

    func select(_ id: Placement.ID?) {
        selection = id
        if let placement = selected {
            expanded.insert(placement.kind)
        }
    }

    func focusSelection() {
        guard let placement = selected else { return }
        camera.tx = placement.position.x
        camera.ty = placement.position.y
        camera.tz = placement.position.z
        camera.distance = 9
    }

    func frameMap() {
        camera = .home
    }

    func duplicateSelection() {
        guard let index = selectedIndex else { return }
        var copy = scene.placements[index]
        copy = Placement(
            id: (scene.placements.map(\.id).max() ?? 0) + 1,
            kind: copy.kind,
            index: count(of: copy.kind),
            entry: copy.entry,
            position: copy.position + Vec3(1, 1, 0),
            yaw: copy.yaw,
            team: copy.team,
            respawnSeconds: copy.respawnSeconds,
            flags: copy.flags
        )
        scene.placements.append(copy)
        selection = copy.id
        say("duplicated \(copy.kind.title.lowercased()) \(copy.label)")
    }

    func deleteSelection() {
        guard let index = selectedIndex else { return }
        let gone = scene.placements.remove(at: index)
        selection = nil
        say("deleted \(gone.kind.title.lowercased()) \(gone.label)")
    }
}

struct SeededRNG: RandomNumberGenerator, Sendable {
    private var state: UInt64

    init(seed: String) {
        var hash: UInt64 = 1_469_598_103_934_665_603
        for byte in seed.utf8 {
            hash = (hash ^ UInt64(byte)) &* 1_099_511_628_211
        }
        state = hash
    }

    mutating func next() -> UInt64 {
        state &+= 0x9E37_79B9_7F4A_7C15
        var z = state
        z = (z ^ (z >> 30)) &* 0xBF58_476D_1CE4_E5B9
        z = (z ^ (z >> 27)) &* 0x94D0_49BB_1331_11EB
        return z ^ (z >> 31)
    }
}
