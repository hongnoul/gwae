# Why gwae?

Extracted from the README so the front page stays skimmable.

I run a lot of coding agents in parallel, and every multiplexer I tried divides a fixed screen: more agents means smaller panes, until nothing is readable. niri solved this on the desktop with scrolling tiling — columns keep their size and the viewport moves instead. gwae brings that model into the terminal you already use.

### The narrative: why now

1. **GUI ADEs (Orca, Conductor, etc.) trade away your control over single agents** through opinionated orchestration surfaces, vendor gravity, and managed flows. You steer a fleet dashboard instead of talking to each agent directly at full size.[^1]
2. **The future is parallelization on performant, cheap, low-reasoning inference.** Cheap tokens mean more concurrent agents, so the bottleneck moves to *your* ability to see and steer them.[^2]
3. **Existing terminal orchestrators ship unnecessary features**, chiefly persistence/attach daemons. Frontier harnesses (Claude Code, Jcode) already carry their own persistence layer via `--resume`, which covers desk work: close the laptop, come back, resume the conversation. What it does not cover — unattended long-running work and non-agent panes — belongs in tmux, which you can run inside a gwae pane.[^3]
4. **Therefore the most important thing** is an ultraminimal **spatial canvas multiplexer** that spawns lightweight terminals fast and lets you prompt efficiently. No terminal-first orchestrator builds this: they all rent tmux for display and own no layout at all.[^4] The two projects that do own this layout model, Séance and tairi, are GUI-bound (GTK app, macOS app) — not in a terminal, not over SSH.[^5]

What gwae is deliberately *not*: an Orca-in-a-TTY. Feature parity on diff review, embedded browsers, PR boards, and mobile is a non-goal.[^6] And at 2 panes, plain kitty splits are fine; gwae's value is a function of agent count.[^7]

### The pitch

**macOS and Windows can have Linux niri-level native spatial canvas.**

- tmux is POSIX-bound: on Windows it requires WSL, Cygwin, or MSYS2, and it has no infinite canvas.[^8] The entire tmux-based orchestrator tier (Claude Squad, dmux, workmux, amux, uzi) inherits that ceiling.[^9] gwae hosts its own PTYs and builds natively for Windows via ConPTY (runtime verification lands in M3), giving you the full 2D plane: orchestrate freely on grids, the 2D is yours to draw.[^5]
- macOS is unfriendly to tiling WMs by design (real tiling via yabai requires partially disabling SIP) and niri itself is Wayland-only.[^10] gwae delivers the niri layout model inside any macOS or Windows terminal instead.
- GUI ADEs cannot copy the structural properties of a terminal tool: runs over SSH, inside any terminal, on the machine the agents already run on, no daemon, no Electron.[^1]
- gwae runs over SSH, including on headless machines, if you want to allocate every resource to your agents.
- **The only bottleneck should be your brain.** Between your keystroke and the pane there is one process and no daemon round-trip: 2.5 ms echo RTT measured, ~1/6 of a 60 Hz frame.[^11]

### Benchmarks: gwae vs the incumbents

Every mux runs headless in a real PTY, driven the way a terminal drives it, on the same machine in the same run: [`scripts/bench_vs_muxes.py`](../scripts/bench_vs_muxes.py), raw JSON in [`docs/bench-2026-08-25.json`](bench-2026-08-25.json).[^11]

#### Footprint and speed (measured 2026-08-25)

| Metric                               | **gwae**                   | tmux 3.7c             | Zellij 0.45.0     | bare `/bin/sh` (floor) |
| ------------------------------------ | -------------------------- | --------------------- | ----------------- | ---------------------- |
| First output after exec              | **10.7 ms**                | 15.9 ms               | 44.7 ms           | 6.6 ms                 |
| Interactive (first keystroke echoed) | **64.9 ms**                | 66.6 ms               | 99.9 ms           | 61.0 ms                |
| Echo RTT, median (150 samples)       | 2.50 ms                    | **0.24 ms**           | 12.48 ms          | 0.10 ms                |
| Echo RTT, p99                        | 2.86 ms                    | **0.91 ms**           | 13.10 ms          | 0.21 ms                |
| Mux RSS, 1 pane                      | **7.5 MB**                 | 9.5 MB                | 102 MB            | 0                      |
| Mux RSS, 4 panes                     | **7.9 MB**                 | 13.6 MB               | (n/a)[^12]        | 0                      |
| Idle CPU, 4 panes                    | 3.2%                       | **0.0%**              | (n/a)[^12]        | 0                      |
| Binary size                          | 2.6 MB                     | **1.0 MB**(+libevent) | 41.4 MB           | -                      |
| Processes per session                | **1** (+ 1 shell per pane) | 2 (client+server)     | 2 (client+server) | 1                      |

TL;DR:
- **vs Zellij**: gwae is 5x lower echo latency, 13x less memory, 16x smaller binary, 4x faster to first paint. "Lightweight spatial canvas" is measured, not marketing.
- **vs tmux**: tmux wins raw echo RTT (event-driven C vs gwae's 1ms poll loop) and idle CPU. Both are invisible in practice (2.5 ms is ~1/6 of a 60 Hz frame) but tmux earns the row. gwae wins memory at 4 panes and needs no daemon.
- So **why gwae over tmux, if tmux echoes faster?** Because the comparison tmux cannot enter is the capability table below: layout and Windows. tmux crams N panes into one screen; gwae's panes never shrink. tmux needs WSL on Windows;[^8] gwae builds natively (ConPTY runtime verification is M3).[^5]

### The capability axis

| Capability | tmux/Zellij | Claude Squad | amux | dmux/workmux | Séance / tairi | **gwae** |
|---|---|---|---|---|---|---|
| 4+ agents visible at full size at once | ✗ crams to fit | ✗ one at a time | ✗ dashboard | ✗ | ✓ niri strips | **✓ no-shrink strips** |
| Layout scales past screen width | ✗ | ✗ | n/a | ✗ | ✓ | **✓ infinite 2D grid** |
| In a terminal / over SSH | ✓ | ✓ | ✓ | ✓ | ✗ GUI-bound | **✓** |
| At-a-glance fleet state | ✗ | session list | kanban + push | ✗ | ✗ | **✓ OSC 133 minimap** |
| Extra daemons/deps | daemon | tmux + gh | server + SQLite + tmux | tmux + git | GTK / macOS app | **none, single process** |
| Windows without WSL | tmux ✗ / Zellij ✓[^5] | ✗ | ✗ | ✗ | ✗ | **✓ builds (ConPTY = M3)** |
| License | ISC / MIT | AGPL | MIT | MIT | MIT / MIT | MIT |
| Survives disconnect | ✓ | ✓ tmux | ✓ server | ✓ tmux | socket / workspaces | ✗ by design → nest in tmux |

Every terminal-first orchestrator is a task manager renting tmux for display; none owns a pixel of layout.[^4] gwae is the mirror image, and the disconnect row is the honest cost of that trade. Deeper prose comparison in [`docs/COMPARISON.md`](COMPARISON.md).

### The recommended stack

gwae is general, but it pairs best with service layers that share the same vision. None of these are dependencies. Research, aggregate, and swap every layer for the latest open-source frontier at will, because gwae does not care what runs inside its panes.

| Layer                | Recommendation                                 | Alternatives                                                                              | Why                                                                                                                                                    |
| -------------------- | ---------------------------------------------- | ----------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Agent harness (TUI)  | **[Jcode](https://github.com/1jehuang/jcode)** | Claude Code, Codex CLI, OpenCode, Gemini CLI, Aider                                       | Extremely lightweight, 18.5k stars.[^13] Own persistence via `--resume`. Disclosure: same author as gwae.                                               |
| Inference (API)      | **Makora**                                     | [OneTriangle](https://onetriangle.ai), Baseten, CoreWeave, DeepInfra, official Meta, SiliconFlow, official DeepSeek | Best measured price/speed for DeepSeek V4 Flash: 300 tps decode, 0.71s TTFT, $0.13/1M blended, OpenAI-compatible.[^14] [OneTriangle](https://onetriangle.ai) is a solid failover (KV-cache-preserving, ~100-300 tps self-measured, no public benchmarks).[^15] |
| Knowledge base (MD)  | **Obsidian**                                   | Logseq, Foam, Dendron, plain git repo of markdown                                         | Plain markdown you version-control with git. High-level plan drafts, or just learning new stuff. Agents can read and edit it directly.                 |
| Editors and monitors | **neovim, btm**, and any other TUI             | helix, kakoune, htop, btop, yazi, lazygit, k9s                                            | Monitor compute overhead, occasionally make direct code and markdown changes at your discretion. The whole open-source TUI ecosystem is the app store. |

## References

[^1]: See [`docs/COMPARISON.md`](COMPARISON.md). Primary sources: onorca.dev, conductor.build, amux.io/guides, agentsroom.dev (retrieved 2026-08-25). The counter-narrative — that past ~4 agents the bottleneck is visibility and you should leave the terminal — is answered by the OSC 133 minimap and smart-jump.
[^2]: DeepSeek V4 Flash official API: $0.14 in / $0.28 out per 1M tokens, $0.0028 on cache hits. Prices as of 2026-08-25, verified against [v4flash.com/pricing](https://v4flash.com/pricing/) and [benchlm.ai](https://benchlm.ai/deepseek/api-pricing); they will drift.
[^3]: `--resume` restores *conversation* state, not *process* state. It covers desk work (close the laptop, come back later) but not unattended runs, non-agent panes, or layout. See the FAQ: a shell that must survive the terminal belongs in tmux, inside a gwae pane.
[^4]: Claude Squad, amux, dmux, workmux, and uzi all render through tmux or a single-agent TUI list. Their competition is on watchdogs, kanban boards, and YAML, not layout. Details in [`docs/COMPARISON.md`](COMPARISON.md).
[^5]: Honest scope of the Windows/layout moat: Zellij 0.44.0+ ships native Windows ([zellij.dev/news](https://zellij.dev/news/remote-sessions-windows-cli/)) and psmux is a native ConPTY tmux reimplementation ([github.com/psmux/psmux](https://github.com/psmux/psmux)), so the moat is against the tmux *orchestrators*, not all multiplexers. Séance (Linux/GTK) and tairi (macOS app) own the niri strip model but are GUI-bound. And compiling is not running: gwae's Windows build passes CI, ConPTY runtime verification is [M3](ROADMAP.md). Until then: "builds for Windows", not "Windows supported".
[^6]: Feature parity with GUI ADEs (browser, diff review, PR boards, mobile) is unwinnable for a sole developer and off-mission. See [Non-goals](ROADMAP.md).
[^7]: Against plain kitty splits, gwae's only argument is layout: no-shrink strips past the screen edge plus the minimap. That argument is worthless at 2 agents and decisive at 6.
[^8]: tmux is POSIX-bound (ptys, signals, termios). Windows install guides offer only WSL2, Cygwin, or MSYS2: [tmux.app/install/windows](https://tmux.app/install/windows/).
[^9]: The entire tmux-orchestrator tier is Unix-or-WSL-only; none can be Windows-native without replacing their whole runtime. See [`docs/COMPARISON.md`](COMPARISON.md).
[^10]: yabai requires partially disabling System Integrity Protection for full tiling (yabai wiki); niri is a Wayland compositor, Linux-only.
[^11]: Method: each mux spawned headless in a 120x30 PTY (`pty.fork`), macOS aarch64, same machine, same run, 2026-08-25. Startup = exec-to-first-byte and exec-to-first-echoed-keystroke. Echo RTT = write 1 char to the mux PTY, wait for it to appear in output; 150 samples after 20 warmup, through `/bin/sh` in the focused pane; the mux sits on this path twice (input in, echo out). RSS = mux's own processes only, shells excluded, via `ps` on the spawned process tree. Idle CPU = cputime delta over 10 s with no input. gwae ran with `input_poll_ms = 1`, tmux with `-f /dev/null`, Zellij with default config plus tips disabled. Script: [`scripts/bench_vs_muxes.py`](../scripts/bench_vs_muxes.py), raw output: [`docs/bench-2026-08-25.json`](bench-2026-08-25.json). Caveats: single machine, single run, release binaries as installed (no debug builds); tmux's echo RTT advantage is real and reproducible; the idle CPU row is gwae's poll loop and is fixable (event-driven wakeup is on the roadmap).
[^12]: Zellij's 4-pane layout session did not become interactive under the harness (it opened its session-resurrect screen instead). Rather than hand-tune it, the cells are n/a; its 1-pane numbers stand.
[^13]: 18,562 stars per GitHub API, 2026-08-25.
[^14]: Provider speed/price data as of 2026-08-25: [Artificial Analysis](https://artificialanalysis.ai/models/deepseek-v4-flash/providers) and [OpenRouter](https://openrouter.ai/deepseek/deepseek-v4-flash). Open items: Makora concurrency under swarm fan-out and cache-hit billing are unverified.
[^15]: OneTriangle throughput estimated from the author's own jcode session logs (589 assistant turns, wall-clock including TTFT and thinking). No public benchmarks exist. It remains a good failover because it sits on or near the Pareto frontier.
