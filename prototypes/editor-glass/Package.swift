// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "EditorGlass",
    platforms: [.macOS(.v26)],
    targets: [
        .executableTarget(
            name: "EditorGlass",
            path: "Sources/EditorGlass"
        )
    ]
)
