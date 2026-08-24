import AppKit
import Foundation
import SwiftUI

/// The scratch directory a harness driving the app points it at.
///
/// With `JP_DEBUG_STATE_DIR` set, the app keeps the state it would otherwise share
/// with the rest of the system inside that directory, and records its process id
/// there. Unset, nothing under it is touched and the app behaves as it ships.
///
/// This exists because the alternatives do not work. The recent-workspace list is
/// keyed by bundle identifier and written on the app's behalf by a system daemon,
/// so it follows neither `HOME` nor anything else in the app's environment; and the
/// file holding it needs Full Disk Access to read, so a harness cannot inspect or
/// restore it either.
///
/// Window state saved by `@SceneStorage` is **not** covered by this directory. It
/// is keyed by bundle identifier, so isolating it is the launching harness's job
/// rather than something this variable can reach.
enum DebugState {
    /// The environment variable naming the directory.
    static let variable = "JP_DEBUG_STATE_DIR"

    /// The environment variable naming a pasteboard to copy to.
    static let pasteboardVariable = "JP_DEBUG_PASTEBOARD"

    /// The pasteboard the app copies to.
    ///
    /// The system one, unless a debug build was told otherwise. There is a
    /// single system pasteboard and it holds whatever the person at the
    /// keyboard last copied, so a driven run that copied into it would destroy
    /// their clipboard. Saving and restoring around the run is not a way out:
    /// a pasteboard item can be a promise its owner fulfils lazily, so what
    /// goes back is a degraded copy of what they had.
    ///
    /// A named pasteboard is a real one that simply nobody is looking at, so a
    /// test can read back exactly what the app wrote.
    ///
    /// Compiled out of a release build. An app that could be told at launch to
    /// copy somewhere nothing pastes from is a bug report waiting to happen,
    /// and that risk is not worth carrying to ship a test seam.
    static var pasteboard: NSPasteboard {
        #if DEBUG
            if let name = ProcessInfo.processInfo.environment[pasteboardVariable],
                !name.isEmpty
            {
                return NSPasteboard(name: NSPasteboard.Name(name))
            }
        #endif

        return .general
    }

    /// The environment variable that turns the app's animations off.
    static let animationVariable = "JP_DEBUG_DISABLE_ANIMATIONS"

    /// Whether the app should animate at all.
    ///
    /// A UI test driving the app waits for it to stop moving before each
    /// action, so every animation is time added to every test that triggers
    /// one. Turning them off is worth more than shortening them, and costs a
    /// test nothing it was checking: what an animation looks like is a question
    /// for a person, and `QA.md` keeps it.
    ///
    /// Compiled out of a release build, like ``pasteboard``, so an app someone
    /// installs cannot be talked into feeling broken.
    static var animationsDisabled: Bool {
        #if DEBUG
            guard let value = ProcessInfo.processInfo.environment[animationVariable] else {
                return false
            }

            return !value.isEmpty
        #else
            return false
        #endif
    }

    /// `animation` normally, and nothing when animations are off.
    ///
    /// Every animation in the app goes through this, so turning them off stays
    /// one decision rather than one per call site.
    static func animated(_ animation: Animation) -> Animation? {
        animationsDisabled ? nil : animation
    }

    /// The directory, or `nil` when the variable is unset or empty.
    static var directory: URL? {
        guard let value = ProcessInfo.processInfo.environment[variable], !value.isEmpty else {
            return nil
        }

        return URL(fileURLWithPath: value)
    }

    /// The recents store the app runs with.
    @MainActor
    static func defaultStore() -> any RecentsStore {
        guard let directory else {
            return DocumentControllerRecents()
        }

        return FileRecents(path: directory.appendingPathComponent("recents.json"))
    }

    /// Record this process's id at `<directory>/pid`.
    ///
    /// A harness launching the app through `open(1)` gets no process id back, and
    /// matching on the executable path cannot tell a driven instance from one the
    /// developer left running. A pid the app reports itself is unambiguous.
    static func recordProcessID() {
        guard let directory else {
            return
        }

        let file = directory.appendingPathComponent("pid")
        do {
            try FileManager.default.createDirectory(
                at: directory,
                withIntermediateDirectories: true
            )
            try "\(getpid())\n".write(to: file, atomically: true, encoding: .utf8)
        } catch {
            let path = file.path(percentEncoded: false)
            FileHandle.standardError.write(
                Data("debug state: could not write \(path): \(error)\n".utf8)
            )
        }

        recordImageSlide()
    }

    /// Record how far ASLR shifted this process's main image.
    ///
    /// A profiler resolves a sampled address by subtracting this from it, and the
    /// alternative is recovering it from the kernel's image-load events — which
    /// only exist in a trace that was already recording when dyld mapped the
    /// image. A recorder that attached to an already-running app has none of
    /// them, so without this every frame it samples stays a bare address.
    ///
    /// Index 0 is the main executable.
    private static func recordImageSlide() {
        guard let directory else {
            return
        }

        let file = directory.appendingPathComponent("slide")
        let slide = _dyld_get_image_vmaddr_slide(0)
        do {
            try "\(slide)\n".write(to: file, atomically: true, encoding: .utf8)
        } catch {
            let path = file.path(percentEncoded: false)
            FileHandle.standardError.write(
                Data("debug state: could not write \(path): \(error)\n".utf8)
            )
        }
    }
}
