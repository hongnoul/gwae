# Show HN draft

Title: Show HN: Gwae – niri's scrolling tiling for CLI agents, in any terminal
URL: https://github.com/hongnoul/gwae

(72/80 title chars; fallback angle if this flops and a 2+ week
retry is warranted: "Show HN: A terminal multiplexer where panes never shrink")

First comment (post immediately after submitting):

> I run a lot of coding agents in parallel, and every multiplexer I tried
> divides a fixed screen: more agents means smaller panes, until nothing is
> readable. niri solved this on the Linux desktop with scrolling tiling, so I
> reimplemented that layout model inside the terminal.
>
> Columns keep a fixed width on an infinite 2D grid of strips and the viewport
> scrolls instead of cramming; the viewport only rests on column boundaries so
> the grid paints identically in every scroll state. It's one process hosting
> its own PTYs (no daemon, no socket), and panes that speak OSC 133 get a
> status-tinted minimap so Alt+g jumps to the agent that needs you.
>
> Honest limitation: no detach, by design. Sessions die with the process.
> Agent harnesses carry their own persistence via --resume, and for unattended
> long-running work you can nest tmux inside a pane, but if SSH session
> persistence is your main need, tmux is still the right tool.
>
> Happy to answer questions.

Timing: Tue-Thu, 14:00-16:00 UTC. Do not ask anyone to upvote (HN detects
voting rings). Answer every comment for the first 3 hours.

Gate: do not post until Roadmap M2 exit (daily dogfood of Claude Code + Jcode
inside gwae, Alt+a / Alt+g / Alt+f live).
