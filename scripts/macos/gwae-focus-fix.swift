// gwae-focus-fix
//
// Works around a kitty/macOS bug: when a kitty OS window is in *native*
// fullscreen it lives on its own Space. Returning to that Space (swipe,
// ctrl+arrow, Mission Control, cmd+tab) activates the app and leaves the
// window AXMain, but macOS never makes it the *key* window, so GLFW thinks
// nothing is focused and every keystroke is dropped until you click the pane.
//
// kitty 0.48.2 has the applicationDidBecomeActive fix for the non-fullscreen
// case (issue #9665), but it is guarded on `!_glfw.focusedWindowId` and does
// not cover native fullscreen, which is the gwae setup.
//
// This daemon listens for kitty becoming the active app and asks kitty itself
// to re-focus its OS window over the remote-control socket. kitty's
// focus-window path calls makeKeyAndOrderFront: and syncs GLFW's focus state,
// which is exactly the missing step. No cursor movement, no synthetic clicks.

import AppKit
import Foundation

let kittenPath = "/Applications/kitty.app/Contents/MacOS/kitten"
let socket = ProcessInfo.processInfo.environment["GWAE_KITTY_SOCKET"] ?? "unix:/tmp/mykitty"
let verbose = ProcessInfo.processInfo.environment["GWAE_FOCUS_FIX_VERBOSE"] == "1"

func log(_ msg: String) {
    let ts = ISO8601DateFormatter().string(from: Date())
    FileHandle.standardError.write("[\(ts)] \(msg)\n".data(using: .utf8)!)
}

/// Run `kitten @ --to <socket> <args>` and return (exitCode, stdout).
@discardableResult
func kitten(_ args: [String]) -> (Int32, String) {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: kittenPath)
    p.arguments = ["@", "--to", socket] + args
    let out = Pipe()
    p.standardOutput = out
    p.standardError = Pipe()
    do { try p.run() } catch { return (-1, "") }
    let data = out.fileHandleForReading.readDataToEndOfFile()
    p.waitUntilExit()
    return (p.terminationStatus, String(data: data, encoding: .utf8) ?? "")
}

/// True when kitty reports an OS window that is *not* focused, i.e. the bug.
func hasUnfocusedWindow() -> Bool {
    let (code, out) = kitten(["ls"])
    guard code == 0, let data = out.data(using: .utf8),
          let wins = try? JSONSerialization.jsonObject(with: data) as? [[String: Any]]
    else { return false }
    // Bug state: kitty is the active app yet no OS window claims focus.
    return !wins.isEmpty && !wins.contains { ($0["is_focused"] as? Bool) == true }
}

/// Re-assert focus after kitty becomes active.
///
/// This deliberately does NOT gate on `hasUnfocusedWindow()` first. macOS
/// settles key-window state asynchronously, so an early probe often reports a
/// transiently *healthy* window that goes unfocused a moment later, and the
/// repair would be skipped exactly when it was needed. `kitty @ focus-window`
/// is idempotent and cheap on an already-focused window, so the correct move
/// is to just assert focus, then verify and retry while it has not stuck.
func repairFocus() {
    // Run off the main thread: this blocks on subprocesses and must not stall
    // the notification queue.
    DispatchQueue.global(qos: .userInitiated).async {
        if verbose { log("kitty activated -> asserting focus") }
        let deadline = Date().addingTimeInterval(1.5)
        var attempts = 0
        repeat {
            // Let macOS finish the Space transition before asserting focus,
            // otherwise the window is not yet on screen to become key.
            Thread.sleep(forTimeInterval: 0.10)
            kitten(["focus-window"])
            attempts += 1
            Thread.sleep(forTimeInterval: 0.15)
            if !hasUnfocusedWindow() {
                if attempts > 1 { log("focus asserted after \(attempts) attempts") }
                return
            }
        } while Date() < deadline
        log("warning: focus still unset after \(attempts) attempts")
    }
}

let nc = NSWorkspace.shared.notificationCenter
nc.addObserver(
    forName: NSWorkspace.didActivateApplicationNotification,
    object: nil, queue: .main
) { note in
    guard
        let app = note.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication,
        app.bundleIdentifier == "net.kovidgoyal.kitty"
    else { return }
    repairFocus()
}

// Space changes can arrive without an app-activation notification (kitty was
// already the active app), so cover that path too.
NSWorkspace.shared.notificationCenter.addObserver(
    forName: NSWorkspace.activeSpaceDidChangeNotification,
    object: nil, queue: .main
) { _ in
    guard NSWorkspace.shared.frontmostApplication?.bundleIdentifier == "net.kovidgoyal.kitty"
    else { return }
    repairFocus()
}

log("gwae-focus-fix watching kitty on \(socket)")
NSApplication.shared.setActivationPolicy(.prohibited)  // no Dock icon, never steals focus
NSApplication.shared.run()
