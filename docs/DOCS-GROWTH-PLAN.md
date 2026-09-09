# Documentation strategy for GitHub star growth

**Status:** research-backed draft, not an implementation checklist already completed.

**Research date:** September 8, 2026, US Eastern / September 9 UTC.

**Scope:** earn more genuine interest through documentation, demonstrations, and relevant discovery. No paid stars, engagement exchanges, unsolicited outreach, or growth guarantees.

## Recommendation in one minute

Make **“add more panes without making them smaller”** the thing people remember and share. Lead with the layout difference, demonstrate it immediately, and use parallel coding agents as the main example. Do not reposition gwae as another all-in-one agent orchestrator.

The next three work packages, in execution order, are:

1. **Correctness and first-run package:** finish the existing README/site edits, reconcile support/persistence claims in the comparison docs, and verify the exact release artifact and newcomer trial. A real-PTY follow-up found a primary-screen reflow failure in v1.3.1 that the development build passes. Drafting can proceed now, but broad promotion waits for a distributed fix to pass. Do not restart the redesign.
2. **Visual proof package:** create one short, unmistakable no-shrink demonstration and reuse its story in the GitHub social preview. Record from the verified release, not an unreleased build presented as current.
3. **Useful discovery package:** add the two workflow recipes, finish the share kit, and place one low-pressure star/save prompt alongside the try path. Installation should not be a prerequisite for expressing interest. Share only after the first two packages pass.

These packages group the individual rows in section 5. **Measure reach separately from interest** throughout. The observed traffic sample is too small to justify headline A/B testing or numerical uplift predictions.

Documentation can improve the likelihood that a visitor understands and stars gwae. It cannot manufacture an audience. Once the first-run and comprehension gates pass, spend the next effort on getting the demonstration in front of relevant people, not another round of adjective changes.

## 1. Baseline and what is already underway

Public GitHub data at research time: **9 stars, 0 forks, created August 25, 2026**, latest release **v1.3.1**, published September 6. The repository already has a homepage and 15 topics, including `terminal-multiplexer`, `niri`, `claude-code`, `tui`, and `rust`. Adding generic topics is not an unclaimed major opportunity. [S1]

The audit began at local commit `6d9db11`. Other work was actively changing `README.md`, `docs/index.html`, and source/tests. Findings below distinguish those drafts from the published site. Recheck before implementing because this is a moving working tree.

Read-only maintainer traffic was also inspected. It is a small sample and does not establish whether discovery or conversion is the dominant bottleneck. Exact nonpublic traffic data is deliberately excluded from this trackable document. Do not divide lifetime stars by the latest traffic window and call that a conversion rate.

### Preserve these strengths

- A concrete visual difference: scrolling rather than shrinking panes.
- A working set of demo assets, prebuilt releases, Homebrew/script install paths, and MIT licensing.
- An existing concise README rather than the 409-line version described in the August launch audit.
- Existing configuration examples, issue templates, and `.github/CONTRIBUTING.md`. Link and improve them rather than inventing parallel resources.
- In-progress README/site edits already add a clearer category, a quickstart, a tmux tradeoff, Windows caveats, an honest status-detection explanation, and star links.
- A concurrent `docs/LAUNCH-KIT.md` draft now contains channel guidance and post outlines. Refine that file rather than creating a second launch kit. Its bare-`gwae` shell trial needs the same first-run verification as the README.

### Remaining gaps with repository evidence

| Observation | Why it matters | Proposed response |
|---|---|---|
| The draft README's “Try it in 30 seconds” says bare `gwae` is a no-account shell demo. `run_tui` routes the first pane through `agent_gateway_cmd()` unless an explicit command is provided. See `crates/gwae/src/main.rs`, `cli.rs`, and the initial-pane block in `tui.rs`. | The first action must match the promise. A picker or configured agent changes the experience. | Use an explicit shell for the no-agent recipe and test the released binary with a clean configuration. Keep the normal agent launch as a separate recipe. |
| The published site still had “Not sped up” and the old “No agent changes needed” status copy. The local draft fixes both. | Local improvements do not help public visitors until published. | After the owning changes are ready, verify the actual deployed page and README, not just local files. |
| GitHub GraphQL reports `usesCustomOpenGraphImage: false`. The site already has `og:image` and a large-image Twitter card. | GitHub links and website links currently have different previews. | Update the existing card if needed, then upload it in repository settings. Do not add duplicate site metadata. |
| `gwae-demo.gif` is **2,100,048 bytes** at 900×582. The existing hero MP4 is **7,120,633 bytes**. The attention GIF is **1,049,177 bytes**. | A new encoding is not automatically smaller. The old audit's 3.2 MB figure is stale. | Optimize for legibility and a short proof. Do not replace the GIF with the existing larger MP4 on the assumption that video is cheaper. |
| `docs/COMPARISON.md` ends by offering harness `--resume` as an alternative for detach/SSH persistence. `docs/WHY.md` mixes nesting advice and conversation/process persistence. | These are materially different guarantees. A caveat in the README does not repair contradictory downstream docs. | Say directly that conversation resume does not keep a process alive. Only document a tmux nesting recipe after testing its exact topology and shortcuts. |
| `COMPARISON.md` and the social-card SVG list Windows without the README's experimental qualification. `WHY.md` contains broad competitor claims, an old capability table, and unrelated product recommendations. | Overstated support and unfair comparisons undermine trust after the first click. | Qualify Windows everywhere. Replace universal competitive claims with dated, sourced tradeoffs. Move personal stack recommendations out of the main evaluation path. |
| `docs/KEYBINDS.md` is a design document, not a concise user reference. `ROADMAP.md` says both “v1.0.0 shipped” and “M6 - Stability to 1.0.” | Readers should not have to infer which instructions or milestones are current. | Provide a user-facing key reference and a short current-status section. Mark historical plans as historical. |

`docs/LAUNCH-READINESS.md` remains useful historical context, not a current backlog. Its later validation logs already record completed social metadata and GIF work. Reconcile its unchecked tasks instead of implementing them twice.

## 2. Research findings and limits

### What the evidence actually supports

- **A star can mean “save this for later,” not “I installed this.”** In Borges and Valente's survey of 791 developers, 51.1% reported bookmarking as a reason for starring. Responses were not mutually exclusive. This is older observational evidence, not a forecast for today's agent-tool audience. GitHub's current documentation also explicitly describes stars as saving interesting projects and showing appreciation. [S2, S3]
- **README structure is associated with popularity, not proven to cause it.** A study of 1,950 repositories across ten languages found associations involving organized lists/images, links, contribution guidance, and references. More established projects may have both better docs and more stars. This supports testing clarity and completeness, not adding badges or images indiscriminately. [S4]
- **The landing page has a narrow job.** GitHub recommends explaining what the project does, why it is useful, how to get started, where to get help, and who maintains it. Keep detailed architecture and design history one click away. [S5]
- **Promotion and documentation work together.** The star-practices study discusses promotion as a factor in growth. Neither it nor the peer audit provides a causal estimate for gwae's docs edits. [S2]

### Comparable projects, selected for fit rather than just star count

Counts below are public snapshots taken September 9 UTC, not targets. These projects differ in age, scope, audience, and distribution. The sample is intentionally useful for design patterns, not statistically representative.

| Project | Stars | Relevant observed documentation pattern | Borrow for gwae |
|---|---:|---|---|
| [niri][P1] | 27,562 | Clear category, screenshot, immediate explanation that opening windows does not resize existing ones, setup showcase, detailed status answers. | Explain the layout in ordinary language and show it. Credit niri without requiring prior knowledge of it. |
| [Zellij][P2] | 35,331 | Visual demo, dedicated screencasts, install guidance, a clearly labeled trial path, separation of release and development instructions. | Separate quick experience from reference material. Test examples against a release, not just `main`. Do not copy its installer behavior without implementing it. |
| [Claude Squad][P3] | 8,451 | Named agent use cases, screenshot/video, explicit prerequisites and workspace-isolation benefits. | Make agent compatibility and prerequisites concrete. Do not imply gwae supplies worktree isolation. |
| [cmux][P4] | 26,911 | Specific macOS/agent positioning, prominent download, feature-specific screenshots, personal explanation of the problem. | A short problem story plus visual proof. Distinguish an in-terminal multiplexer from replacing the terminal app. |
| [dmux][P5] | 1,767 | Direct “parallel agents with tmux and worktrees” promise, demo, short install/start sequence, explicit requirements. | Small, reproducible workflow recipes and clear boundaries. A closer task comparator than a general-purpose mega-project. |
| [lazygit][P6] | 82,147 | Simple product category and operation demonstrations, but also substantial sponsor material. | Show a real operation. Do **not** copy its sponsor-heavy opening or assume every choice of an established project caused its success. |

Common pattern worth testing: **recognizable problem → visual proof → concrete next step**. There is no universal winning README length, fold position, badge count, or star-request wording established by this research.

## 3. Positioning and proposed copy

### Audience order

1. **Primary:** developers running several CLI coding-agent sessions who want readable panes without replacing their terminal or adopting a full orchestration system.
2. **Secondary:** niri/scrolling-layout enthusiasts and terminal power users, including people who do not use agents.
3. **Not the lead audience yet:** people who require unattended SSH process persistence, integrated worktree/PR management, or production-verified native Windows support.

Lead with the unique layout, then name the agent use case. “Agent orchestration” invites comparison on automation, worktrees, and persistence that gwae deliberately does not provide. “Niri in your terminal” is a useful community shorthand, but should not be the only explanation.

### Recommended hero, building on the current draft

> **A scrolling terminal multiplexer. Panes never shrink.**
>
> Run Claude Code, Codex, shells, and TUIs in the terminal you already use. Add more panes and the viewport scrolls instead of squeezing them.
>
> Inspired by niri's scrolling layout. macOS and Linux, with experimental native Windows support.

Below the demonstration:

> **More panes. Same width.** Open another column, scroll to it, and keep the earlier panes at their chosen size.
>
> **Try the layout** · **Useful idea? Star gwae to save it for later.**

Use “Claude Code” and “Codex” as examples of separately installed CLIs, not bundled services or endorsements. Retain the no-agent path. For an audience that already knows niri, test a sharing headline such as **“niri-style scrolling panes, inside your terminal”**, without simultaneously changing the main README proposition.

“Never shrink” means adding panes does not squeeze the existing layout. Avoid suggesting that every pane can be simultaneously visible at full size, or that resizing the terminal cannot affect the viewport.

### README order

1. Small identity block, one promise, one explanatory sentence, existing release/CI/license badges.
2. One primary demo with a caption that explains exactly what changed.
3. Primary try/install navigation and one low-pressure star/save invitation.
4. Recommended install paths by OS, followed immediately by the no-agent trial.
5. Two short examples: parallel agents and a general terminal workspace.
6. “When to use gwae / when to keep tmux,” with a comparison link.
7. Five essential keys, support/limitations, docs/examples/contribution links.

Keep detailed config, all keybindings, architecture, benchmark methodology, and historical design debates out of this path. Do not impose a line-count target. Test how quickly a newcomer can answer the important questions.

### First-run copy to validate, not publish blindly

For the **macOS/Linux no-agent demo**, propose `gwae run /bin/sh`, which explicitly selects a shell rather than the agent gateway. Then add columns with `Alt+Enter` until one falls beyond the edge, move with `Alt+h` / `Alt+l`, and exit the disposable shells with `exit`. Explain Option versus Alt nearby. Keep deliberate resizing with `Alt+r` out of this minimal recipe until the distributed release passes the primary-screen reflow check documented in section 8.

The syntax is supported by source inspection and the released CLI's `run --help`. The project's real-PTY suite also successfully launches an explicit shell command, but it uses a test configuration, not the complete newcomer path. This research did **not** complete clean-machine installation or the default-configuration five-pane trial. Before advertising “30 seconds,” time the path **after installation**, with default configuration and no installed agent. Test a returning user's configured-agent setup too. Keep Windows separately labeled experimental rather than presenting `/bin/sh` as a universal command.

For agent users, preserve `gwae run "claude"` and `gwae run "codex"`. Explain that these are alternative launches, not a command block to execute sequentially inside nested gwae sessions. Document how `Alt+;` chooses the next agent and state that multiple agents editing one checkout are **not automatically isolated**.

## 4. Demonstrations and documentation that people can share

### Primary demo storyboard: roughly 12–18 seconds

| Time | Show | Viewer takeaway |
|---|---|---|
| 0–3 s | Two legible panes containing recognizable terminal work. | This is a terminal workspace, not a new chat application. |
| 3–8 s | Add enough columns to cross the screen edge. Highlight one existing pane's unchanged width. | The layout scrolls rather than squeezing. |
| 8–13 s | Move to the offscreen column and back, with visible key labels. | Offscreen panes are easy to reach. |
| 13–18 s | End on the same readable layout and a short caption. | The idea is memorable without sound or prior niri knowledge. |

Use actual behavior from the release. No fabricated terminal output presented as a live agent, hidden time compression, or unrelated content in the caption. A separate, labeled deterministic shell fixture is fine for showing layout mechanics. Review every frame for private paths, credentials, prompts, and conversations before publishing.

Keep the existing attention-jump demo as a **second** proof for agent users. Show the status caveat next to it: OSC 133 when available, otherwise activity/idle heuristics. Do not combine every feature into the hero.

Acceptance: legible at normal README width, meaningful first frame, visible offscreen transition, clear without sound. Aim for a compact GIF around **1–1.5 MiB if readable**, but treat that as a design budget, not a growth fact. On the website, consider a newly optimized video with a poster and accessible playback controls, including reduced-motion behavior. On GitHub, verify the actual supported rendering rather than relying on raw HTML video autoplay. Provide a static image or text explanation as a fallback.

### Two recipe pages, not a documentation platform migration

- **“Run Claude Code and Codex in scrolling panes.”** Prerequisites, launch, open a second agent, move between them, explain status, close safely. Include a real screenshot and link to `examples/agent-fleet.toml`. Explicitly distinguish layout management from worktree/task orchestration.
- **“A scrolling workspace for shell tools.”** Start without an agent account, use a shell/editor/monitor example, show why offscreen panes beat more splits for that task. Any extra TUI is optional and separately installed.

Each recipe needs one promised outcome, copyable steps, expected result, known limitations, and links back to install and the repository. Reuse existing example configs and explain how to merge them without overwriting a reader's configuration. Add a short user key reference rather than relabeling the design strategy as a manual.

### Comparison rewrite

Prefer a small decision table covering layout, whether a new terminal app is required, detach/process persistence, built-in worktree management, and platform maturity. Include tmux, Zellij, cmux, and an agent-worktree tool where useful. Link each competitor's own documentation and date every capability assertion.

Give alternatives their real strengths. Keep “choose tmux for detach/attach” prominent. Replace “every orchestrator,” “any OS,” “no layout at all,” and similar absolutes. Keep dated benchmark results with their methodology, not as current universal superiority claims. Refresh the old `WHY.md` narrative before promoting it as evidence.

## 5. Prioritized implementation plan

Effort ranges are planning estimates for one maintainer, not measured durations. Priority reflects likely user impact and risk, not an invented percentage increase in stars.

| Priority | Change and files/surface | Effort | Acceptance gate |
|---|---|---:|---|
| P0 | Verify a distributed artifact containing the current reflow fix before a broad docs launch. | Release-owner dependent | Run `resize_e2e` against that exact downloaded binary. All three tests must pass, including primary-screen reflow. Passing only the development build is insufficient. |
| P0 | Reconcile first-run copy and finish existing `README.md` / `docs/index.html` work. | 1–3 h plus platform testing | Pass the five-person comprehension and trial protocol in section 6, including both supported operating systems. Exact release commands work with a clean config and no agent. Live content matches approved copy. |
| P0 | Correct persistence, Windows maturity, and overbroad comparison claims in `COMPARISON.md`, `WHY.md`, and relevant card text. | 1–2 h | No page equates conversation resume with process survival. Support claims agree across entry points. |
| P0 | Produce the focused demo and card in `docs/assets/`. Upload the card as the GitHub social preview after review. | 3–6 h | Demo proves unchanged width. Both site and repository unfurls communicate the same promise. GitHub reports a custom preview after the settings change. |
| P1 | Put one save-for-later star prompt next to the demo/try path. Preserve the site's already-drafted GitHub CTA. | 30–60 min | Links go to the real repository. No duplicated pleading, auto-starring, fake count, or suggestion that a star subscribes to releases. |
| P1 | Add the two recipes, link existing examples, add a user key reference, and update the comparison. | 3–5 h | A tester completes each recipe without undocumented prerequisites or destructive config replacement. |
| P1 | Finish the concurrent `docs/LAUNCH-KIT.md`: short GIF/video, screenshot, 40-word description, 150-word explanation, canonical repo URL. | 1–2 h | Every claim is present in verified docs. Audience-specific framing changes, product facts do not. |
| P2 | Mark `LAUNCH-READINESS.md` historical, reconcile `ROADMAP.md`, surface `.github/CONTRIBUTING.md`, simplify the public docs index. | 1–2 h | Users can distinguish current instructions from design history and find a concrete contribution path. |

**Suggested sequence:** first finish P0 and run comprehension/first-run checks. Then publish the recipes and share kit. Keep the first public version stable long enough to collect feedback. A settings change, release, push, community submission, or public post is a separate action to review, not something performed by this plan.

### Distribution that follows from the docs

The docs work should yield something useful to share, not just a request for stars:

- For the niri/tiling audience: the unchanged-width demonstration and a precise explanation of how gwae differs from a compositor. niri's README already recognizes scrollable tiling in other environments. This suggests audience affinity, not permission to add promotional links. [P1]
- For Claude Code/Codex users: the multi-agent recipe and explicit tradeoffs against worktree orchestrators.
- For terminal-tool curators: a reproducible no-account trial, platform status, and a readable screenshot. Terminal Trove is a candidate to investigate, not a confirmed placement or an asserted missing listing. [S8]
- For a broader Show HN or similar post: wait until the no-agent trial and support caveats hold up. Lead with the distinctive interaction and ask for workflow feedback, not votes.

Read each community's current rules, disclose authorship, and avoid duplicate cross-posts or competitor-issue advertising. `awesome-niri` primarily catalogs niri resources and integrations, so do not assume an inspired terminal tool qualifies. No outreach or submissions were made during this research.

## 6. Measurement and decision rules

### Separate the paths

```text
Relevant exposure → repository visit → understands the idea → star/save
                                   ↘ tries it → useful experience → star/share
```

These are conceptual paths, not individually tracked users. GitHub does not provide a complete attribution funnel from README exposure to a star.

| Measure | How to collect | Interpretation and caveat |
|---|---|---|
| Net new stars over a fixed interval | Snapshot the public count at the start/end, or collect daily. | Primary outcome, but net count includes unstars and does not attribute a cause. |
| Repository views and unique visitors | GitHub Insights or the read-only traffic API. Archive privately at least weekly. | Reach proxy. GitHub reports a rolling 14-day window in UTC. Do not add overlapping window uniques. |
| Referrers and popular paths | Same traffic interface, with recorded date range. | Directional distribution evidence, not complete campaign attribution. |
| Comprehension | Show the opening to five target users for ten seconds. | Ask what it does, what changes when a pane is added, whether it replaces their terminal, and what they would click next. Proposed gate: four of five get the core distinction right. |
| First useful experience | Observe five clean-config trials on documented supported setups. | Proposed gate: four of five complete the post-install no-agent path without help. Testers must be told they need not star the project. |
| Trial failure themes | User feedback and issues that include OS, terminal, release, and failing step. | Helps identify support friction. A falling issue count by itself is not proof of success. |

### Concrete P0 acceptance protocol for the implementing maintainer

The docs maintainer owns this check and the release maintainer supplies the exact artifact. Recruit five target readers, ideally three parallel-agent users and two general terminal users. This is a proposed qualitative test, not a completed study.

1. Show the opening for ten seconds, then hide it. Ask: what does this do, what happens when another pane is added, must you replace your terminal, and what would you click next? Record anonymous answers before explaining anything. Pass if at least four of five correctly describe scrolling without shrinking and can choose a sensible next step.
2. Observe five clean-config trials using the documented install method, recording OS, terminal/version, release, install route, elapsed time after installation, and the first confusing step. Include at least two macOS trials in different terminals and two Linux trials in different terminals. Use the fifth trial to repeat the riskiest combination. Do not claim a terminal-specific compatibility result without testing it.
3. Follow only the published instructions: launch the explicit shell, add enough panes to cross the edge, navigate back, use help, and exit all disposable shells. Pass if four of five finish unaided **and** each promoted OS/terminal combination has a successful full trial. An overall four-of-five result must not conceal a broken platform. Test the proposed command once more with an existing agent configured to ensure the shell path does not launch it.
4. Record PATH problems and Option/Alt misconfiguration as trial failures, then improve the relevant instructions and repeat the failing case. Do not call it a “30-second” trial until the post-install observations support that wording. Windows remains explicitly experimental and is not inferred from the macOS/Linux results.
5. Ask permission before recording screens, avoid credentials/private work, and tell participants they do not need to star the project. Keep detailed participant records private. Publish only the support claims and examples that these observations justify.

GitHub's traffic retention and update behavior are documented in [S6]. If using `net new stars / unique visitors` over the **same** period, label it a rough period ratio, not conversion probability. Stars can come from outside the measured visit path, and readers may return before starring. Clones and release downloads are not verified installs or active users. The unusually different clone/view totals seen in the audit should not be treated as adoption evidence.

### Lightweight experiment sequence

1. **Baseline:** record the public star count, available traffic interval, published documentation revision, releases, and known sharing events. Keep detailed traffic private.
2. **Comprehension first:** compare the existing presentation and proposed demo with target users. Fix misunderstandings before seeking more traffic. Five users are a qualitative gate, not statistical validation.
3. **Ship one coherent P0 version:** note its actual deployment time. Do not simultaneously rewrite the hero every day.
4. **Share one useful asset at a time:** log the audience, date, asset, and feedback. Compare similar windows cautiously. Releases, external mentions, and changing audience mix are confounders.
5. **Review after roughly two weeks:** if reach remains tiny, improve discovery rather than declaring the copy a winner or loser. If people arrive but misunderstand, revise the demo. If they understand but cannot try it, fix instructions or product friction. If they try it but do not value it, revisit the use case rather than increasing star requests.

Do not run a conventional README A/B test now. One canonical README is not naturally randomized, and the present sample cannot distinguish small effects. A website experiment becomes reasonable only after repeatable traffic exists, with a preselected outcome, realistic sample-size calculation, and a defined attribution limitation. No new analytics vendor or tracking script is required for the initial plan.

## 7. What not to prioritize

- A logo redesign, more badges, a star-history chart, or generic keyword stuffing.
- A new docs framework, mass translations, or a large “awesome” list before first-run clarity and audience demand are established.
- Headline benchmark wins without current, reproducible evidence.
- A full-featured agent-orchestrator story that implies worktree isolation, automation, or daemon persistence.
- Hiding experimental Windows support or removing tradeoffs to make the product sound stronger.
- Repeated star asks, giveaways for stars, manufactured testimonials, unsolicited direct messages, or copied promotional comments.
- Promising a fixed number of stars or a percentage uplift based on peer totals.

## 8. Research and validation record

Performed: local documentation/source audit, live site fetch, read-only GitHub metadata/traffic/social-preview inspection, six peer README reviews, and primary-source research on README guidance and starring practices. Peer links below are pinned to the revisions observed during research. Public star counts are timestamped snapshots, not immutable properties of those commits.

Not performed: clean-machine installs, human comprehension sessions, a fully rendered GitHub/mobile layout audit, new demo recording, public posting, repository-settings changes, or deployment. Recommendations above are hypotheses grounded in observed friction, not measured growth outcomes.

**Acceptance finding:** the official v1.3.1 macOS binary passed two of three real-PTY resize tests, but failed primary-screen text reflow. The current development build passed all three. This changed the plan: verify a distributed fix before broad promotion, and omit resizing from the minimal trial until that gate passes. The complete default-configuration newcomer trial remains unverified.

Detailed public-interface results, limitations, and requirement-to-evidence mapping are in [the validation appendix](DOCS-GROWTH-VALIDATION.md). Keeping the test record there preserves an auditable plan without making execution depend on reading test internals.

### Sources

- **[S1]** [gwae public repository metadata](https://api.github.com/repos/hongnoul/gwae), [latest observed release](https://github.com/hongnoul/gwae/releases/tag/v1.3.1), and [published site](https://hongnoul.github.io/gwae/). Read September 9, 2026 UTC. Social preview checked with GitHub GraphQL fields `openGraphImageUrl` and `usesCustomOpenGraphImage`.
- **[S2]** Borges and Valente, [What's in a GitHub Star? Understanding Repository Starring Practices in a Social Coding Platform](https://arxiv.org/html/1811.07643), 2018. Survey and observational growth research. See bookmarking results and limitations, not a causal copywriting experiment.
- **[S3]** GitHub Docs, [Saving repositories with stars](https://docs.github.com/en/get-started/exploring-projects-on-github/saving-repositories-with-stars).
- **[S4]** Venigalla and Chimalakonda, [An Empirical Study On Correlation between Readme Content and Project Popularity](https://arxiv.org/html/2206.10772), 2022. Abstract and results report 1,950 analyzed READMEs. Associations, not intervention effects.
- **[S5]** GitHub Docs, [About the repository README file](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/about-readmes).
- **[S6]** GitHub Docs, [Viewing traffic to a repository](https://docs.github.com/en/repositories/viewing-activity-and-data-for-your-repository/viewing-traffic-to-a-repository).
- **[S7]** GitHub Docs, [Classifying your repository with topics](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/classifying-your-repository-with-topics). Supports relevant topic discovery, not ranking guarantees.
- **[S8]** [Terminal Trove](https://terminaltrove.com/) and [awesome-niri](https://github.com/niri-wm/awesome-niri). Discovery candidates only. Review current scope and submission guidelines before any action.

[P1]: https://github.com/niri-wm/niri/blob/dd75865f547f0eac0e9b6c4d86d2cd00c0744252/README.md
[P2]: https://github.com/zellij-org/zellij/blob/af38660c5884f50bb3726682fb92961326c4268f/README.md
[P3]: https://github.com/smtg-ai/claude-squad/blob/ce1ffb4392b01f38e2c4599c7c84d2a93973b138/README.md
[P4]: https://github.com/manaflow-ai/cmux/blob/76b81fe75bc27338f5d0bd3b20fa2aa343243ea8/README.md
[P5]: https://github.com/standardagents/dmux/blob/8cb3d926631a9349ab67f7ece41d218427ac7e24/README.md
[P6]: https://github.com/jesseduffield/lazygit/blob/c07f4d381b90419583b7ce04f87379654d983ebc/README.md
