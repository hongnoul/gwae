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

/// The last osascript stderr, for diagnosable failure logs: a TCC denial
/// (-1743) reads completely differently from a transient window-reference
/// error (-1719), and the warning is useless without that distinction.
let lastErrLock = NSLock()
var lastErr = ""

/// Ask Ghostty to re-focus its front terminal via AppleScript. Returns true
/// when osascript exits 0.
@discardableResult
func ghosttyRefocus() -> Bool {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
    p.arguments = ["-e", "tell application id \"\(ghosttyBundleID)\" to focus (focused terminal of selected tab of front window)"]
    p.standardOutput = Pipe()
    let errPipe = Pipe()
    p.standardError = errPipe
    do { try p.run() } catch { return false }
    let errData = errPipe.fileHandleForReading.readDataToEndOfFile()
    p.waitUntilExit()
    if p.terminationStatus != 0 {
        let msg = String(data: errData, encoding: .utf8)?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        lastErrLock.lock()
        lastErr = msg
        lastErrLock.unlock()
    }
    return p.terminationStatus == 0
}

/// True when Ghostty is the frontmost app right now. Checked immediately
/// before every assert, not just at trigger time: the asserts run 100-250ms
/// after the notification, and Ghostty's `focus` verb raises its window.
/// Asserting after the user already moved to another app or Space would
/// yank them back to Ghostty's Space.
func ghosttyIsFrontmost() -> Bool {
    NSWorkspace.shared.frontmostApplication?.bundleIdentifier == ghosttyBundleID
}

/// True when a Ghostty window is on the *active* Space right now.
///
/// `frontmostApplication` is global, not per-Space: swiping away from
/// Ghostty's fullscreen Space leaves Ghostty frontmost until the user
/// clicks something, so frontmost alone cannot distinguish "returned to
/// Ghostty's Space" (repair) from "left it" (stand down). The on-screen
/// window list is per-Space and makes that call.
func ghosttyOnActiveSpace() -> Bool {
    guard let ghostty = NSRunningApplication.runningApplications(
        withBundleIdentifier: ghosttyBundleID
    ).first else { return false }
    guard let wins = CGWindowListCopyWindowInfo(
        [.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID
    ) as? [[String: Any]] else {
        // Cannot tell (no Screen Recording permission): err on repairing,
        // the frontmost gate still protects the common click-away case.
        return true
    }
    return wins.contains { win in
        (win[kCGWindowOwnerPID as String] as? Int32) == ghostty.processIdentifier
            && (win[kCGWindowLayer as String] as? Int ?? 1) == 0
    }
}

/// Re-assert focus after Ghostty becomes active.
///
/// This deliberately asserts unconditionally instead of probing key state
/// first: macOS settles key-window state asynchronously, so an early probe
/// often reports a transiently healthy window and the repair would be
/// skipped exactly when it was needed. `focus` on the already-focused
/// terminal is idempotent, so the correct move is to assert, wait out the
/// Space transition, and assert again.
///
/// Every assert is gated on Ghostty still being frontmost at that instant.
/// The repair exists to fix a window that is active yet not key; the moment
/// the user activates anything else, the premise is gone and the repair
/// must stand down instead of stealing the Space back.
///
/// Failures are always logged: a silent daemon with denied Automation
/// permission looks identical to a working one, and the log is the only
/// witness. Check `/tmp/gwae-focus-fix.log` when keys still drop.
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
        guard ghosttyIsFrontmost() && ghosttyOnActiveSpace() else {
            if verbose { log("stand down: ghostty not frontmost on the active space") }
            return
        }
        let first = ghosttyRefocus()
        Thread.sleep(forTimeInterval: 0.15)
        guard ghosttyIsFrontmost() && ghosttyOnActiveSpace() else { return }
        let second = ghosttyRefocus()
        if !first && !second {
            lastErrLock.lock()
            let detail = lastErr
            lastErrLock.unlock()
            log("warning: osascript focus failed twice (\(detail)); -1743 means grant Automation for gwae-focus-fix in System Settings > Privacy & Security > Automation")
        }
    }
}

// `--once`: run one repair synchronously and exit. Proves the primitive
// (Automation permission, scripting bridge) without waiting for a Space
// switch: `gwae-focus-fix --once; echo $?` should print 0.
if CommandLine.arguments.contains("--once") {
    Thread.sleep(forTimeInterval: 0.10)
    let ok = ghosttyRefocus() || ghosttyRefocus()
    if !ok {
        log("warning: --once repair failed (\(lastErr)); check Automation permission")
    }
    exit(ok ? 0 : 1)
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
