// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "DiskMonitorTray",
    platforms: [.macOS(.v13)],
    targets: [
        .executableTarget(
            name: "DiskMonitorTray",
            resources: [.process("Resources")]
        )
    ]
)
