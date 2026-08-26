# gwae launch checklist

Sequenced: each step feeds the next. Snapshot before and after every step
(`astrophile snapshot`) so you know what moved the needle.

## Phase 0: before any traffic — DONE 2026-08-25, audit 33/33
- [x] Demo GIF at the top of the README (gwae-demo.gif, mp4 linked)
- [x] Repo homepage URL set (https://hongnoul.github.io/gwae/)
- [x] 15 topics, keyword-rich 85-char description
- [x] Tagged release v1.0.0 with prebuilt binaries + checksums (5 platforms)
- [x] `astrophile audit` clean: 33/33
- [x] llms.txt committed (AI crawlers ingest it verbatim)
- [x] examples/ with doctor-validated gwae.toml configs
- [x] docs/agents.md paste-ready agent snippet
- [x] Baseline snapshot #1 taken (1 star, 2026-08-25)

## Phase 1: permanent surfaces (week 1, before launch spikes)
- [x] Homebrew tap live and tested 2026-08-25: repo hongnoul/homebrew-tap created,
      `brew install hongnoul/tap/gwae` installs 1.0.0, `brew test` (runs `gwae doctor`) passes
- [x] crates.io published 2026-08-25: gwae, gwae-layout, gwae-term, gwae-testkit all
      at 1.0.0 (cargo search confirms). Token `gwae-publish` (publish-new+publish-update)
      saved via cargo login.
- [x] niri Discussions thread posted 2026-08-25:
      https://github.com/niri-wm/niri/discussions/4473 (Show and tell). ANSWER EVERY REPLY.
- [x] AUR published 2026-08-25: gwae-bin 1.0.0 live (per-arch sources for
      x86_64 + aarch64, provides/conflicts gwae, checksums verified against
      release assets). https://aur.archlinux.org/packages/gwae-bin

## Phase 1b: pre-launch surfaces closed 2026-08-26
- [x] `docs/agents.md` written. llms.txt and this checklist both linked it and it
      was a 404: every AI crawler that ingested llms.txt hit a dead link, and the
      single highest-leverage GEO surface (harness AGENTS.md/CLAUDE.md context)
      had nothing to paste. Now live, with a README "For coding agents" section.
- [x] Social preview card (docs/assets/social-card.png, 1200x630). Every HN,
      Reddit, X, and Discord unfurl was bare before this. STILL MANUAL: upload it
      at Settings > Social preview (no REST API for this field).
- [x] Discussions enabled (support surface + crawlable long-tail Q&A).
- [x] ROADMAP de-staled: M0/M1/M2/M4 were all unchecked while v1.0.1 shipped
      them. A visitor read "make-or-break milestone: not started".

## Phase 1c: install channels actually serving v1.0.1 — 2026-08-26
The 683d81c commit bumped `packaging/` in-repo but nothing downstream: for a day
every channel served 1.0.0 while the README advertised v1.0.1. Verified by
querying each registry, not by reading the repo. Fixed and re-verified:
- [x] Homebrew tap pushed; `brew install hongnoul/tap/gwae` → 1.0.1, `brew test` passes
- [x] crates.io: gwae, gwae-layout, gwae-term, gwae-testkit all at 1.0.1
      (gwae itself published from a clean v1.0.1 worktree, NOT the dirty tree)
- [x] AUR gwae-bin 1.0.1 pushed, .SRCINFO regenerated; cgit confirms 1.0.1
      (the RPC endpoint lags a few hours, cgit is the source of truth)
- [x] All 4 SHA256s re-derived from the published tarballs before publishing

RELEASE RULE (this bit twice now): publishing is not `git push` on packaging/.
After every tag, query the registries themselves — brew/crates.io/AUR cgit —
and only then say a channel is live.

## GATE: Roadmap M2 exit — MET (v1.0.0)
Alt+; agent spawn, OSC 133 minimap, Alt+g smart-jump, Alt+f full-width all live
and dogfooded daily. (hwatu's rule: launch gate first, publicity second. It
worked: 78 stars.) Remaining pre-launch judgement call is only your own
confidence in a first-run experience under a 500-visitor spike.

## Phase 2: launch (pick ONE channel per day, answer every comment for 3h)
Order matters: HN first (highest ceiling, and the Reddit/lobste.rs posts can
cite the HN thread), then one Reddit sub per day. Never two channels in one day:
you cannot answer comments in two places, and unanswered threads convert ~0.

- [ ] Upload the social preview card BEFORE any post (unfurls are permanent)
- [ ] `astrophile snapshot` immediately before HN
- [ ] Show HN (drafts/show-hn.md) — Tue-Thu, 14:00-16:00 UTC
- [ ] lobste.rs (needs an invite; tag rust, unix)
- [ ] Reddit, one sub per day (drafts/reddit.md): agent subs → r/rust → r/commandline
- [ ] `snapshot` at 24h and 7d after each channel
- [ ] If HN flops: wait 2+ weeks, retry once with the panes-never-shrink title

## Phase 3: compounding (after first spike)
- [ ] Awesome-list PRs (drafts/awesome-lists.md) — most require >30 days age + traction
- [ ] Newsletter submissions (Console.dev, Terminal Trove)
- [ ] `astrophile geo --llm "claude -p" --runs 5` monthly: do assistants recommend gwae?

Measurement guardrail (from hwatu): the metric is qualified users who install,
run `gwae doctor`/`init`, and say whether it improves their agent loop — not
raw impressions.

Repo: https://github.com/hongnoul/gwae
