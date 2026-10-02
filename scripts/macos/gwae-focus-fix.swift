// gwae-focus-fix (Ghostty)
//
// Works around a macOS/Ghostty bug: when Ghostty is in *native* fullscreen
// it lives on its own Space. Returning to that Space (swipe, ctrl+arrow,
// Mission Control, cmd+tab) activates the app and leaves the window AXMain,
// but macOS never makes it the *key* window, so Ghostty drops every
// keystroke until you click the pane. gwae never sees the keys; they die
// between macOS and the terminal.
//
// Two triggers, two rules:
//
// * App activation (cmd-tab): conservatively re-assert key focus once,
//   gated on Ghostty staying frontmost, so a fast app switch is never
//   stolen from.
// * Space arrival (swipe/ctrl-arrow INTO a Space): if the destination
//   Space's layer-0 windows belong exclusively to Ghostty — the signature
//   of its fullscreen Space — Ghostty gets focus *unconditionally*. A
//   fullscreen Space contains nothing else to interact with, so this can
//   never steal; and on every other Space the daemon is inert, so other
//   workspaces are never touched. Swiping into gwae means gwae has focus,
//   no click needed.
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
///
/// With `gated: true` the script re-checks frontmost *inside itself*,
/// immediately before the focus verb: the daemon's Swift-side gates run
/// ~50-150ms before osascript executes, and a fast app switch inside that
/// race would otherwise be stolen from. exit 0 with "skip" output counts
/// as success: standing down is a correct outcome, not a failure.
///
/// With `gated: false` (Space arrival onto Ghostty's own fullscreen
/// Space) the gate is deliberately absent: the previous app *is* still
/// frontmost there — that is the bug being fixed — and the destination
/// Space holds nothing else to steal from.
@discardableResult
func ghosttyRefocus(gated: Bool = true) -> Bool {
    let gate = gated ? """
    try
        tell application "System Events"
            set front_bid to bundle identifier of first process whose frontmost is true
        end tell
        if front_bid is not "\(ghosttyBundleID)" then return "skip"
    on error
        -- System Events unavailable (TCC denied for this context): the
        -- Swift-side frontmost gate already passed 0-150ms ago, which is
        -- the dominant protection. Proceed rather than silently killing
        -- every repair.
    end try

    """ : ""
    let script = gate + """
    tell application id "\(ghosttyBundleID)" to focus (focused terminal of selected tab of front window)
    """
    let p = Process()
    p.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
    p.arguments = ["-e", script]
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

/// True when the active Space is Ghostty's *fullscreen* Space: every
/// layer-0 on-screen window belongs to Ghostty (and there is at least
/// one). On such a Space there is nothing else to interact with, so
/// activating Ghostty cannot steal focus from anything the user wants.
/// A shared/tiled Space fails this test and gets no unconditional focus.
///
/// Returns nil when the window list is unavailable (no permission):
/// callers must then fall back to the conservative gated path.
func activeSpaceIsGhosttyFullscreen() -> Bool? {
    guard let ghostty = NSRunningApplication.runningApplications(
        withBundleIdentifier: ghosttyBundleID
    ).first else { return false }
    guard let wins = CGWindowListCopyWindowInfo(
        [.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID
    ) as? [[String: Any]] else { return nil }
    var sawGhostty = false
    for win in wins {
        guard (win[kCGWindowLayer as String] as? Int ?? 1) == 0 else { continue }
        if (win[kCGWindowOwnerPID as String] as? Int32) == ghostty.processIdentifier {
            sawGhostty = true
        } else {
            return false
        }
    }
    return sawGhostty
}

/// Re-assert focus after Ghostty becomes active.
///
/// Exactly one assert per activation, never repeats. The `focus` verb
/// *activates Ghostty*: every extra assert inside a window is another
/// chance to fight the user mid-app-switch and steal focus back. A
/// multi-assert poll loop did exactly that in practice. One late assert
/// is enough for the main-but-not-key bug; a miss costs one click, a
/// stolen focus costs trust.
///
/// The assert waits out the Space transition (up to 1.2s) but fires at
/// most once, and only when Ghostty is frontmost *and* on the active
/// Space at that instant. Any other app becoming frontmost aborts
/// permanently for this activation.
func repairFocus() {
    repairLock.lock()
    if Date().timeIntervalSince(lastRepair) < 0.5 {
        repairLock.unlock()
        return
    }
    lastRepair = Date()
    repairLock.unlock()

    // Run off the main thread: this blocks on subprocesses and must not
    // stall the notification queue.
    DispatchQueue.global(qos: .userInitiated).async {
        if verbose { log("ghostty activated -> repair window open") }
        let deadline = Date().addingTimeInterval(1.2)
        while Date() < deadline {
            Thread.sleep(forTimeInterval: 0.15)
            guard ghosttyIsFrontmost() else {
                if verbose { log("stand down: another app is frontmost") }
                return
            }
            guard ghosttyOnActiveSpace() else {
                // Transition still in flight; keep waiting.
                continue
            }
            // Settle pause, then re-check the user has not moved on in
            // the meantime: the assert below activates Ghostty, so this
            // is the last moment to stand down cheaply.
            Thread.sleep(forTimeInterval: 0.15)
            guard ghosttyIsFrontmost() && ghosttyOnActiveSpace() else {
                if verbose { log("stand down: user moved during settle") }
                return
            }
            if !ghosttyRefocus() {
                lastErrLock.lock()
                let detail = lastErr
                lastErrLock.unlock()
                log("warning: osascript focus failed (\(detail)); -1743 means grant Automation for gwae-focus-fix in System Settings > Privacy & Security > Automation")
            } else if verbose {
                log("focus asserted (single)")
            }
            return
        }
        if verbose { log("repair window closed without assert") }
    }
}

/// Space arrival: the user shifted workspaces. If the destination settles
/// as Ghostty's own fullscreen Space, give Ghostty focus unconditionally
/// (activate + focus verb): the previous app is still frontmost after a
/// swipe until a click lands, which is precisely the state the frontmost
/// gates can never repair. On any other Space, do nothing at all.
func spaceArrival() {
    repairLock.lock()
    if Date().timeIntervalSince(lastRepair) < 0.5 {
        repairLock.unlock()
        return
    }
    lastRepair = Date()
    repairLock.unlock()

    DispatchQueue.global(qos: .userInitiated).async {
        if verbose { log("space changed -> probing destination") }
        // Wait for the transition to settle: poll until the window list
        // reports a stable verdict twice in a row.
        let deadline = Date().addingTimeInterval(1.5)
        var last: Bool? = nil
        while Date() < deadline {
            Thread.sleep(forTimeInterval: 0.15)
            guard let fullscreen = activeSpaceIsGhosttyFullscreen() else {
                // No window-list permission: cannot prove the Space is
                // safe for unconditional focus. Fall back to the gated
                // repair, which requires Ghostty frontmost.
                if verbose { log("no window list; falling back to gated repair") }
                if ghosttyIsFrontmost() { ghosttyRefocus() }
                return
            }
            if fullscreen && last == true {
                if verbose { log("arrived on ghostty fullscreen space -> taking focus") }
                NSRunningApplication.runningApplications(
                    withBundleIdentifier: ghosttyBundleID
                ).first?.activate()
                if !ghosttyRefocus(gated: false) {
                    lastErrLock.lock()
                    let detail = lastErr
                    lastErrLock.unlock()
                    log("warning: arrival focus failed (\(detail))")
                } else if verbose {
                    log("focus taken on arrival")
                }
                return
            }
            if !fullscreen && last == false {
                if verbose { log("destination is not ghostty's space; inert") }
                return
            }
            last = fullscreen
        }
        if verbose { log("space probe window closed unsettled") }
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

// Space changes are the swipe-in path. No frontmost gate here: after a
// swipe the *previous* app is still frontmost until a click, which is
// exactly the state spaceArrival exists to fix. Destination safety is
// decided inside (only Ghostty's own fullscreen Space takes focus).
NSWorkspace.shared.notificationCenter.addObserver(
    forName: NSWorkspace.activeSpaceDidChangeNotification,
    object: nil, queue: .main
) { _ in
    spaceArrival()
}

log("gwae-focus-fix watching ghostty")
NSApplication.shared.setActivationPolicy(.prohibited)  // no Dock icon, never steals focus
NSApplication.shared.run()
