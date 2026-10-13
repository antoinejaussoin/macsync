// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "MacSync",
    platforms: [.macOS(.v13)],
    products: [
        .executable(name: "MacSync", targets: ["MacSync"]),
    ],
    targets: [
        .executableTarget(name: "MacSync", path: "Sources"),
        .testTarget(name: "MacSyncTests", dependencies: ["MacSync"]),
    ]
)
