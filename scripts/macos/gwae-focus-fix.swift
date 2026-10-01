// gwae-focus-fix (Ghostty)
//
// Works around a macOS/Ghostty bug: when Ghostty is in *native* fullscreen
// it lives on its own Space. Returning to that Space (swipe, ctrl+arrow,
// Mission Control, cmd+tab) activates the app and leaves the window AXMain,
// but macOS never makes it the *key* window, so Ghostty drops every
// keystroke until you click the pane. gwae never sees the keys; they die
// between macOS and the terminal.
//
// This daemon listens for Ghostty becoming the active app (plus Space
// changes where Ghostty stays frontmost) and asks Ghostty itself to re-focus
// its front terminal over AppleScript. The `focus` verb runs in Ghostty's
// process and re-asserts key-window state, which is exactly the missing
// step. No cursor movement, no synthetic clicks.
//
// Successor to the kitty `kitten @ focus-window` daemon: same triggers,
// Ghostty's scripting bridge instead of kitty's remote-control socket, so
// there is no socket config to keep in sync.
//
// First launch prompts once for Automation permission ("control Ghostty");
// allow it and the daemon stays silent afterwards.

import AppKit
import Foundation

let ghosttyBundleID = "com.mitchellh.ghostty"
let verbose = ProcessInfo.processInfo.environment["GWAE_FOCUS_FIX_VERBOSE"] == "1"

// Debounce: a Space return often fires both didActivateApplication and
// activeSpaceDidChange within milliseconds. One repair per window.
var lastRepair = Date.distantPast
let repairLock = NSLock()

func log(_ msg: String) {
    let ts = ISO8601DateFormatter().string(from: Date())
    FileHandle.standardError.write("[\(ts)] \(msg)\n".data(using: .utf8)!)
}

/// Ask Ghostty to re-focus its front terminal via AppleScript. Returns true
/// when osascript exits 0.
@discardableResult
func ghosttyRefocus() -> Bool {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
    p.arguments = ["-e", "tell application id \"\(ghosttyBundleID)\" to focus (focused terminal of selected tab of front window)"]
    p.standardOutput = Pipe()
    p.standardError = Pipe()
    do { try p.run() } catch { return false }
    p.waitUntilExit()
    return p.terminationStatus == 0
}

/// Re-assert focus after Ghostty becomes active.
///
/// This deliberately asserts unconditionally instead of probing key state
/// first: macOS settles key-window state asynchronously, so an early probe
/// often reports a transiently healthy window and the repair would be
/// skipped exactly when it was needed. `focus` on the already-focused
/// terminal is idempotent, so the correct move is to assert, wait out the
/// Space transition, and assert again.
func repairFocus() {
    repairLock.lock()
    if Date().timeIntervalSince(lastRepair) < 0.5 {
        repairLock.unlock()
        return
    }
    lastRepair = Date()
    repairLock.unlock()

    // Run off the main thread: this blocks on a subprocess and must not
    // stall the notification queue.
    DispatchQueue.global(qos: .userInitiated).async {
        if verbose { log("ghostty activated -> asserting focus") }
        // Let macOS finish the Space transition before the first assert,
        // otherwise the window is not yet on screen to become key.
        Thread.sleep(forTimeInterval: 0.10)
        ghosttyRefocus()
        Thread.sleep(forTimeInterval: 0.15)
        ghosttyRefocus()
    }
}

let nc = NSWorkspace.shared.notificationCenter
nc.addObserver(
    forName: NSWorkspace.didActivateApplicationNotification,
    object: nil, queue: .main
) { note in
    guard
        let app = note.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication,
        app.bundleIdentifier == ghosttyBundleID
    else { return }
    repairFocus()
}

// Space changes can arrive without an app-activation notification (Ghostty
// was already the active app), so cover that path too.
NSWorkspace.shared.notificationCenter.addObserver(
    forName: NSWorkspace.activeSpaceDidChangeNotification,
    object: nil, queue: .main
) { _ in
    guard NSWorkspace.shared.frontmostApplication?.bundleIdentifier == ghosttyBundleID
    else { return }
    repairFocus()
}

log("gwae-focus-fix watching ghostty")
NSApplication.shared.setActivationPolicy(.prohibited)  // no Dock icon, never steals focus
NSApplication.shared.run()
