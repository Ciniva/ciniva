// swift-tools-version:6.0
import PackageDescription

let swift5: [SwiftSetting] = [.swiftLanguageMode(.v5)]

let package = Package(
    name: "Ciniva",
    platforms: [.macOS(.v15)],
    targets: [
        .systemLibrary(
            name: "CMPV",
            path: "Sources/CMPV",
            pkgConfig: "mpv",
            providers: [.brew(["mpv"])]
        ),
        .target(
            name: "CinivaKit",
            path: "Sources/CinivaKit",
            swiftSettings: swift5,
            linkerSettings: [.linkedFramework("Security")]
        ),
        .executableTarget(
            name: "Ciniva",
            dependencies: ["CMPV", "CinivaKit"],
            path: "Sources/Ciniva",
            swiftSettings: swift5 + [.unsafeFlags(["-Xcc", "-DGL_SILENCE_DEPRECATION"])],
            linkerSettings: [
                .linkedFramework("OpenGL"),
                .linkedFramework("AVFoundation"),
                .linkedFramework("QuartzCore"),
            ]
        ),
        .executableTarget(
            name: "ciniva-check",
            dependencies: ["CinivaKit"],
            path: "Sources/ciniva-check",
            swiftSettings: swift5
        ),
        .testTarget(
            name: "CinivaTests",
            dependencies: ["CinivaKit", "Ciniva"],
            path: "Tests/CinivaTests",
            resources: [.copy("Fixtures")],
            swiftSettings: swift5
        ),
    ]
)
