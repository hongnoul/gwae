# Clipboard — paste only

gwae pastes text; it does not manage copy. Drag selection highlight remains
for visual feedback, but gwae does not write to the system clipboard. Use
your terminal's native selection/copy (or `pbcopy`/`wl-copy`/`xclip` directly
from the shell) if you need the host clipboard.

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

