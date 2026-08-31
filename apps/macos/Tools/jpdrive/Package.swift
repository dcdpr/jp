// swift-tools-version: 6.0

import PackageDescription

// Mirrors the app's `project.yml`: an existential is spelled `any P`, and a
// warning that never fails a build is a warning nobody fixes.
//
// SwiftPM 6.0 has no first-class setting for warnings-as-errors, and
// `unsafeFlags` is rejected only for a package consumed as a dependency, which
// this one never is.
let strict: [SwiftSetting] = [
    .swiftLanguageMode(.v6),
    .enableUpcomingFeature("ExistentialAny"),
    .unsafeFlags(["-warnings-as-errors"]),
]

let package = Package(
    name: "jpdrive",
    platforms: [.macOS(.v15)],
    products: [
        .executable(name: "jpdrive", targets: ["jpdrive"])
    ],
    targets: [
        // The driver's logic, in a library so it can be tested. SwiftPM cannot
        // cleanly test an executable target, and the traversal is where the bugs
        // are.
        .target(name: "DriveKit", swiftSettings: strict),

        // One line, calling into the library.
        .executableTarget(name: "jpdrive", dependencies: ["DriveKit"], swiftSettings: strict),

        .testTarget(name: "DriveKitTests", dependencies: ["DriveKit"], swiftSettings: strict),
    ]
)
