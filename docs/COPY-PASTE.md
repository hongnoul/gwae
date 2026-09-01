# Clipboard — removed

gwae no longer manages the clipboard.

- **Paste:** Use the host terminal's native paste (`Cmd+V` / `Ctrl+V`, or `Ctrl+Shift+V` in some terminals). gwae does not enable bracketed paste or read the clipboard itself.
- **Copy / selection:** Drag selection highlight remains for visual feedback, but gwae does not write to the system clipboard. Use your terminal's native selection/copy (or `pbcopy`/`wl-copy`/`xclip` directly from the shell) if you need the host clipboard.
- **Images:** The `image_clipboard` / `⌥+Shift+c` (PNG) flow has been removed. Capture screenshots with the OS or terminal, not gwae.

This document is kept as a tombstone so links do not 404.

