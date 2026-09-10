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
- **Programs without bracketed paste:** these receive plain text without
  escape markers. Their newlines can still execute commands, according to
  the program's own input handling. This includes macOS's bundled bash 3.2.
  Hosts must support bracketed paste too.
- **Pickers and confirmations:** native paste into the spawn-dir picker
  updates only its single-line filter. It cannot accept a picker or confirm
  a force quit. Pasting while the force-quit prompt is up cancels that prompt
  and discards the paste.
- **Optional clipboard shortcut (`⌥+v`):** gwae reads the system clipboard
  itself (`pbpaste` on macOS, `wl-paste`/`xclip`/`xsel` on Linux) and
  bracket-writes it to the focused pane. This remains available alongside
  native paste. gwae handles the chord in plain panes because fish binds
  `ESC+v` to `edit_command_buffer` (the "external editor requested" error
  when `$VISUAL`/`$EDITOR` is unset), not to paste.
- **Agent panes keep their own paste:** when the focused pane is an agent
  pane, `⌥+v` is forwarded to the inner jcode untouched (`ESC+v`), so its own
  smart paste (text vs image vs dictation) stays the authority.
- **Native paste in agent panes:** `Cmd+V` forwards the pasted text, not an
  `⌥+v` chord or a second clipboard read. The agent receives one paste event
  when it enables bracketed paste.
- **Images:** the `image_clipboard` / `⌥+Shift+c` (PNG) flow remains removed.
  Capture screenshots with the OS or terminal, not gwae.

See [native-paste acceptance evidence](NATIVE-PASTE-ACCEPTANCE.md) and
[drag-copy acceptance evidence](DRAG-COPY-ACCEPTANCE.md) for before/after
replays, requirement-to-test mappings, observed results, and validation limits.
