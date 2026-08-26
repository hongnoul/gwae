# Reddit drafts

One sub per day, never simultaneously. Answer every comment for the first 3
hours. Agent subs first (they have the pain), Rust second (implementation
interest), rice subs last (screenshot-driven).

## r/ClaudeCode / r/ChatGPTCoding (day 1-2, primary audience)
Title: I got tired of my parallel agents shrinking into unreadable slivers, so I built a multiplexer where panes never shrink
Body: the problem story (N agents, tmux crams), the demo GIF, then the link.
Lead with the minimap + Alt+g "jump to the agent that needs you" workflow.
Mention: no daemon because your harness already has --resume.

## r/rust (day 3)
Title: gwae: a daemon-free terminal multiplexer with niri's scrolling tiling (single process, ~8MB RSS)
Body: focus on the implementation; that sub stars implementations, not
products. Talking points: gwae-layout is a pure proptest'd crate, quantized
scroll invariant (pixel-identical paint at every scroll state, verified at
hostile widths), one process hosting its own PTYs via portable-pty, bench
methodology in scripts/bench_vs_muxes.py (2.5ms echo RTT, honest table where
tmux wins raw RTT).

## r/commandline (day 4)
Title: gwae – niri's scrolling tiling for your terminal: infinite strip grid, panes never shrink, no daemon
Body: general TUI audience. Demo GIF first, capability table second, honest
"when tmux is still right" paragraph third.

## r/unixporn (later, screenshot/video REQUIRED, lead with the rice not the tool)
Title: [any terminal] agent fleet on an infinite strip — catppuccin, minimap, no compositor needed
Body: link https://github.com/hongnoul/gwae in a comment, not the post.
Angle: niri layout without Wayland, works on macOS too.
