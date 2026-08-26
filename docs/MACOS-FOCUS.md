# macOS: keyboard focus lost when returning to gwae's Space

## Symptom

kitty runs in **native fullscreen**, so it owns its own macOS Space. You switch
away (another Space, cmd+tab, Mission Control) and switch back. kitty is the
frontmost app and the window is drawn, but the cursor renders **hollow** and
every keystroke is dropped, sometimes with an error beep. Typing only starts
working after you **click** inside the window.

This is not a gwae bug. gwae never receives the keys: they are dropped above
it, between macOS and kitty.

## Cause

macOS distinguishes the **main** window from the **key** window. Only the key
window receives keyboard input.

Leaving the Space sends `windowDidResignKey:`, so GLFW (kitty's windowing
layer) clears its focused-window id. Returning to a *native fullscreen* Space
re-activates the app and restores the window as `AXMain`, but macOS does not
always send `windowDidBecomeKey:` for the window that was already key. GLFW
therefore still believes nothing is focused and discards key events.

Confirmed on this machine with kitty's own remote-control state:

```
$ kitty @ ls | jq '.[0].is_focused'
false          # ...while kitty is the frontmost app and looks focused
```

Accessibility shows the same split, which is the fingerprint of this bug:

```
AXMain = true      # window is the app's main window
AXFocused = false  # but it is not the key window
```

kitty 0.48.2 contains the upstream fix for the non-fullscreen case
([#9665](https://github.com/kovidgoyal/kitty/issues/9665), commit `66ffb68`),
which adds `applicationDidBecomeActive:` to `glfw/cocoa_init.m`. That fix is
guarded on `!_glfw.focusedWindowId` and does **not** cover native fullscreen,
which is exactly the gwae setup.

## Fix

Two options. Pick one.

### Option A: traditional fullscreen (no daemon)

In `~/.config/kitty/kitty.conf`:

```conf
macos_traditional_fullscreen yes
```

Verified to make focus survive the switch with no helper process. The tradeoff
is that kitty no longer gets its **own Space**, so it is no longer a swipe
target in Mission Control. If you navigate to gwae by switching Spaces, this
changes your workflow and you probably want Option B.

### Option B: focus-repair agent (keeps your own Space)

Ask kitty to re-assert focus whenever it becomes the active app.
`kitty @ focus-window` calls `makeKeyAndOrderFront:` and syncs GLFW's focus
state, which is the step macOS skipped.

Enable remote control in `~/.config/kitty/kitty.conf`:

```conf
allow_remote_control yes
listen_on unix:/tmp/mykitty
```

`listen_on` requires a **kitty restart** to take effect; it is read at startup.

Then run `gwae-focus-fix` (source in
[`scripts/macos/`](../scripts/macos/)), a ~90-line Swift agent that observes
`NSWorkspace.didActivateApplicationNotification` plus
`activeSpaceDidChangeNotification` and re-asserts focus:

```sh
mkdir -p ~/.local/bin
cp scripts/macos/gwae-focus-fix.swift ~/.local/bin/
swiftc -O ~/.local/bin/gwae-focus-fix.swift -o ~/.local/bin/gwae-focus-fix
cp scripts/macos/com.gwae.focus-fix.plist ~/Library/LaunchAgents/
launchctl load -w ~/Library/LaunchAgents/com.gwae.focus-fix.plist
```

It sets `NSApplication.activationPolicy = .prohibited`, so it has no Dock icon
and never steals focus itself. It moves no cursor and synthesizes no clicks.

Two details that matter, both found the hard way:

* **Do not gate the repair on a "is it broken?" probe.** macOS settles
  key-window state asynchronously, so an early probe frequently reports a
  transiently healthy window that goes unfocused a moment later, and the repair
  gets skipped exactly when it was needed. `focus-window` is idempotent, so
  assert focus unconditionally, then verify and retry for up to 1.5s.
* **Do the work off the main thread.** It blocks on subprocesses and would
  otherwise stall the notification queue.

## Verifying

The honest test is whether characters actually reach the shell, not whether the
window merely looks focused:

```sh
kitty @ ls | jq '.[0].is_focused'   # must be true after switching back
kitty @ get-text | tail             # must contain what you typed
```

With the agent unloaded, `is_focused` stays `false` after returning to a
fullscreen kitty and typed characters never appear. With it loaded, focus is
restored and typing lands immediately, without a click.
