# niri Discussions thread draft

Where: https://github.com/YaLTeR/niri/discussions (category: Show and tell)
When: Phase 1-2. Post yourself; it is their community, be a guest.

Title: gwae: bringing niri's scrolling-tiling layout model to plain terminals

Body:

> Hi! Long-time admirer of niri's layout model. I wanted scrollable no-shrink
> tiling for the fleet of CLI coding agents I run, but inside a terminal (and
> on macOS/Windows where niri can't go), so I built gwae:
> https://github.com/hongnoul/gwae
>
> It reimplements the strip/column semantics from niri's public docs and
> observed behavior only (gwae is MIT, so no GPL code was read or ported):
> fixed-width columns on an infinite grid of strips, viewport scrolls instead
> of cramming, focus-follows navigation, workspace-like strip creation at the
> edges. Column widths quantize to 1/4 / 1/3 / 1/2 fractions and the viewport
> only rests on column boundaries.
>
> Not trying to compete with niri (it's a TUI multiplexer, niri is a
> compositor); posting because the layout model deserves to exist outside
> Wayland too. Happy to answer questions, and thank you for the design writing
> in the niri wiki, it made the reimplementation possible.

Notes:
- Do NOT frame as an ad. Lead with gratitude + the reimplementation-from-docs
  detail (it preempts the GPL question, which their community will ask first).
- Answer every reply for 24h.
- `astrophile snapshot` before and 24h after posting.
