# Clipboard: drag to copy, paste with `Cmd+V`

- **Copy:** left-drag inside a pane, then release. gwae copies the highlighted
  text and shows a `copied …` confirmation. Plain clicks and blank selections
  do not replace the clipboard. Dragging beyond the pane clamps to its edges,
  not neighboring panes. Unicode is preserved and grid padding is trimmed.
- **Mouse-aware programs:** vim, jcode, and other children requesting mouse
  reporting own ordinary drags. Hold **Shift** when starting a drag to select
  with gwae instead. Once that drag starts, releasing Shift before the mouse
  does not lose the selection. If the host terminal intercepts Shift-drag,
  its own native selection/copy behavior applies instead.
- **Clipboard transport:** native `pbcopy` (macOS), `wl-copy` / `xclip` / `xsel`
  (Linux), or `clip` (Windows) is preferred. Helpers have a bounded wait. Over
  SSH, or if native helpers fail, gwae sends an OSC 52 request to the host
  terminal. The toast says `copy sent to terminal`, not `copied`, because the
  terminal may reject clipboard writes. Enable OSC 52 in the host if necessary.
  Terminal-output failures show `clipboard unavailable`.
- **Copy shortcuts:** `⌥+c` and image-copy shortcuts remain removed. This
  restores drag-to-copy only. Native terminal copy remains available too.

- **Native paste (`Cmd+V`):** use your terminal's normal paste shortcut
  (`Ctrl+Shift+V` or `Ctrl+V` in other terminals). gwae requests bracketed
  paste from the host and forwards the text as one paste to the focused pane,
  with delimiters only if that child requested them. In supporting shells
  such as fish and zsh, multiline input stays editable until you press Enter.
  Blank lines and Unicode are preserved. No special keybinding is required,
  and this route also works over SSH without a remote clipboard helper.
- **One paste action:** multiline handling, pane-aware framing, and delivery
  feedback all run on native paste. gwae's HUD and hints advertise only
  `Cmd+V` (or the platform's native paste shortcut), not a second smart-paste
  key. A confirmation reports the line count after delivery, with a warning
  if the child does not support bracketed paste.
- **Programs without bracketed paste:** these receive plain text without
  escape markers. Their newlines can still execute commands, according to
  the program's own input handling. This includes macOS's bundled bash 3.2.
  Hosts must support bracketed paste too.
- **Pickers and confirmations:** native paste into the spawn-dir picker
  updates only its single-line filter. It cannot accept a picker or confirm
  a force quit. Pasting while the force-quit prompt is up cancels that prompt
  and discards the paste.
- **`⌥+v` is no longer a gwae shortcut:** gwae does not read the clipboard
  or synthesize a paste for it. Like any unbound chord, it belongs to the
  child, which may have its own binding. A literal `√` remains text.
- **Agent panes use the same native paste path:** `Cmd+V` forwards the
  terminal's supplied content once, not a synthetic `⌥+v` chord or a second
  clipboard read. The agent receives one paste event when it enables
  bracketed paste. Content-aware behavior stays with the agent. For example,
  Jcode's native paste handler can recognize image file paths and image URLs.
  gwae must not replace pasted text by probing unrelated clipboard formats.
- **Raw clipboard images:** ordinary terminal paste carries text or a file
  path, not arbitrary image bytes. Image-only paste requires host/agent
  support. Removing the extra gwae shortcut does not add a universal image
  transport, change an agent's own shortcuts, or merge dictation into paste.
- **Images:** the `image_clipboard` / `⌥+Shift+c` (PNG) flow remains removed.
  Capture screenshots with the OS or terminal, not gwae.

See [native-paste acceptance evidence](NATIVE-PASTE-ACCEPTANCE.md) and
[drag-copy acceptance evidence](DRAG-COPY-ACCEPTANCE.md) for before/after
replays, requirement-to-test mappings, observed results, and validation limits.
