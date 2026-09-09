# gwae launch kit

Drafts for maintainer review. Nothing here has been submitted or posted.
Channel guidance checked on 2026-09-09. Recheck rules immediately before posting.

## Lead with the visible difference

**A scrolling terminal multiplexer. Panes never shrink.**

Show a fifth pane opening beyond the screen edge, then moving back to an earlier
pane whose width has not changed. Follow with a real coding-agent workflow.
That demonstration is more distinctive than another list of agent integrations.

Keep the caveats close: no daemon means no detach/attach or surviving a terminal
disconnect. Windows is experimental. Agent attention detection can be heuristic.
Do not pitch gwae as a complete tmux replacement or promise performance numbers
that have not been measured against a named workload.

## Before sending traffic

- [ ] Publish the reviewed README and landing-page changes.
- [ ] Recheck the latest CI and PTY E2E runs. Resolve failures before a broad launch.
- [ ] Have a fresh user try installation, five `Alt+Enter` presses, focus movement,
  help, and exit. Check macOS and Linux separately. Do not infer Linux support
  from a macOS-only smoke test.
- [ ] Check Option/Alt behavior in the terminal used for the demo.
- [ ] Record a focused 15–25 second clip using a released version. Start with
  panes, not a logo. Use readable font sizes and captions. Show new panes
  preserving width, then focus movement. Keep shell-only reproduction possible.
- [ ] Check the clip for private paths, prompts, credentials, and conversations.
- [ ] Set aside time to answer questions and reproduce reported problems.

## Rollout order

1. **Small niri/terminal-user feedback round.** The scrolling mental model is
   already familiar to this audience. Ask what makes the workflow confusing,
   not for stars. Use an appropriate community showcase thread only after
   checking its current rules. niri's community links are listed in
   [awesome-niri](https://github.com/niri-wm/awesome-niri#help-and-discussion).
   Inspiration alone is not grounds for adding gwae to its integration list.
2. **Show HN after first-run problems are fixed.** Link directly to the usable
   repository. Lead with no-shrink tiling, not AI hype or a version announcement.
   Check whether gwae has already had a Show HN before submitting again.
3. **Rust community post with technical substance.** Explain one concrete
   tradeoff in the Rust PTY/layout implementation and include the clip.
   This Week in Rust editors monitor r/rust for project/tooling updates.
   Their current rules explicitly say not to submit project-update PRs.
4. **Follow up where users responded.** Share fixes in the original threads.
   Do not carpet-bomb communities with identical posts or repost to chase votes.

Reddit's rules endpoints returned 403 during preparation. Community permission
has not been verified. Review rules in the browser or ask moderators before use.

## Show HN draft

**Title:** Show HN: gwae, a scrolling terminal multiplexer where panes never shrink

**URL:** https://github.com/hongnoul/gwae

**Opening comment:**

> I'm working on gwae, which brings niri-style scrolling tiling into a terminal.
> Open another pane and the viewport scrolls instead of squeezing your existing
> panes. It works with shells, TUIs, and CLI coding agents.
>
> To see the difference without setting up an agent: install it, run `gwae`,
> finish the first-run setup (skip the agent choice), then press Alt+Enter five
> times. Alt+h/l moves between panes. On macOS use Option,
> with your terminal configured to send it as Alt/Meta if needed.
>
> It's Rust and MIT, with a single process and no daemon. That's also a tradeoff:
> it doesn't offer tmux-style detach/attach, and closing the terminal doesn't
> preserve running processes. Windows support is experimental.
>
> I'd especially like feedback on whether scrolling panes help your workflow,
> and which terminal/OS combinations have a rough first run.

Edit the personal framing to match your actual experience. Attach the existing
demo link in a comment if helpful. No requests for upvotes, coordinated comments,
or repeated submissions. See the official
[Show HN guidelines](https://news.ycombinator.com/showhn.html).

## Niri / terminal community draft

**Title:** I brought niri-style scrolling tiling to terminal panes

> gwae is a terminal multiplexer where opening more panes scrolls the viewport
> instead of shrinking the existing panes. It runs in your terminal and doesn't
> require niri or Wayland.
>
> [Attach the short no-shrink demo here.]
>
> Shell-only test: run `gwae`, finish the first-run setup (skip the agent choice),
> open five panes with Alt+Enter, then move with Alt+h/l. I'm interested in whether that layout feels useful for terminal work,
> and where it falls short of your current setup.
>
> Rust, MIT, no daemon. No detach/attach. Windows is experimental.
> https://github.com/hongnoul/gwae

Use only where project showcases are welcome. Disclose authorship and tailor
the question to that community rather than pasting the same message everywhere.

## Rust post outline

**Possible title:** Building a no-shrink terminal multiplexer in Rust

- Start with the demo and the layout invariant: adding panes does not divide
  the existing viewport into ever-smaller pieces.
- Explain PTY sizing versus the visible viewport using the actual implementation.
- Walk through one concrete rendering or terminal-compatibility problem,
  including the test that caught it. Avoid unsupported claims about speed.
- Explain the single-process design and the loss of session persistence.
- End with a specific technical question or a reproducible testing request.

This is an outline, not a claim that a technical article already exists.
[This Week in Rust's contribution guidance](https://github.com/rust-lang/this-week-in-rust#projects-tooling-updates)
describes the current project-update route. If developing an article with AI,
also follow its authorship disclosure guidance.

## Measure the experiment, not just the star count

Keep GitHub traffic snapshots and post URLs in a private local log, not in this
public repository. GitHub exposes a rolling traffic window, so save snapshots
at launch, after 24 hours, after 7 days, and after 14 days.

Record:

- Total stars and net change since the baseline.
- Unique repo visitors and the API's exact reporting dates.
- Referrers, plus where each post was actually published.
- First-run failures, platform/terminal details, and fixes shipped.
- Release downloads and clones only as noisy supporting signals, not installs
  or active users. Automation can dominate both.

Do not divide all-time stars by two weeks of visitors and call it conversion.
Referrers are incomplete, users overlap across channels, and star timing does
not prove attribution. Treat the first round as an experiment, not a forecast.

**Decision rule:** If qualified traffic stays low, improve distribution. If
traffic rises but users cannot reach the five-pane demo, fix onboarding. If
people try it successfully but do not return or recommend it, investigate the
workflow value before adding more promotion.
