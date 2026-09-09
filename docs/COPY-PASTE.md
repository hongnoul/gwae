# Clipboard: drag to copy, paste with `⌥+v`

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

- **Paste (`⌥+v`):** gwae reads the system clipboard itself (`pbpaste` on
  macOS, `wl-paste`/`xclip`/`xsel` on Linux) and bracket-writes it to the
  focused pane, so a multi-line paste arrives as one block. This is the route
  that works in plain shell panes: fish binds `ESC+v` to `edit_command_buffer`
  (the "external editor requested" error when `$VISUAL`/`$EDITOR` is unset),
  so forwarding the key can never paste there.
- **Agent panes keep their own paste:** when the focused pane is an agent
  pane, `⌥+v` is forwarded to the inner jcode untouched (`ESC+v`), so its own
  smart paste (text vs image vs dictation) stays the authority.
- **Host native paste still works:** `Cmd+V` / `Ctrl+V` (or `Ctrl+Shift+V` in
  some terminals) arrives as the terminal's own paste and goes straight to
  the focused pane.
- **Images:** the `image_clipboard` / `⌥+Shift+c` (PNG) flow remains removed.
  Capture screenshots with the OS or terminal, not gwae.

See [drag-copy acceptance evidence](DRAG-COPY-ACCEPTANCE.md) for the before/after
replay, requirement-to-test mapping, observed results, and validation limits.
