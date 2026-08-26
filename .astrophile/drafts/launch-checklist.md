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
- [ ] AUR package publish — STILL BLOCKED: needs the AUR account password for
      hongnoul (account exists, maintains hwatu). New SSH key generated at
      ~/.ssh/id_ed25519.pub; log in at aur.archlinux.org -> My Account -> paste key,
      then run the hwatu aur-publish.sh pattern against packaging/aur/PKGBUILD
      (gwae-bin). Username is prefilled on the open Safari tab.

## GATE: do not launch until Roadmap M2 exit
Daily dogfood of Claude Code + Jcode inside gwae; Alt+a / Alt+g / Alt+f live.
(hwatu's rule: launch gate first, publicity second. It worked: 78 stars.)

## Phase 2: launch (pick ONE channel per day, answer every comment for 3h)
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
