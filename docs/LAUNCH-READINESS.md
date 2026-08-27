# Launch readiness: README skim test + install friction

Audit date: 2026-08-26. Repo age 2 days, 6 stars, crates.io downloads 22.
Scope: the two items that convert a visitor into a star or an install.

---

## 4. README passes the 10-second test

### What a visitor sees now

409 lines. Above the fold: logo, 4 badges, two bold taglines, a nav row, the
demo GIF, a caption, and a "NOT SPED UP" note. No install command is visible
before the fold. The first *content* section is a 6-item feature list, then a
~120-line essay ("Why gwae", "The narrative", benchmarks, capability axis,
recommended stack) before Quick Start.

Specific problems:

| # | Problem | Why it costs stars |
|---|---|---|
| 4.1 | Tagline "most tactile terminal-native multiplexer for agent orchestration" | Ungrammatical (missing "the"), and "tactile" + "orchestration" are both unfalsifiable. A visitor cannot picture the product. |
| 4.2 | Two competing taglines stacked | Two claims read as neither. One line only. |
| 4.3 | No install one-liner above the fold | The cheapest conversion on the page is buried at line ~57. |
| 4.4 | GIF caption is about solid-state chemistry and `btm` | The demo's job is "N agents, one needs me, ⌥+g jumps there". Caption currently points attention at the wrong thing. |
| 4.5 | 3.2 MB GIF | Slow first paint on mobile/slow links; GitHub may lazy-load. The hero asset must be under ~2 MB. |
| 4.6 | "The narrative: why now" 4-point essay + footnotes above Quick Start | This is a blog post inside a README. Great content, wrong position; it reads as defensive. |
| 4.7 | Benchmarks + capability table before "how do I use it" | Comparison tables convert the already-interested, not the newly-arrived. |
| 4.8 | Recommended-stack table promotes Jcode/Makora/OneTriangle | Reads as cross-promotion on a first visit. Belongs in docs. |
| 4.9 | 4 badges, one of which (Platforms) is static and non-informative | Badge rows dilute; keep release + CI + license. |

### Target above-the-fold structure

```
logo
# gwae
one sentence:  Run 6 coding agents side by side. Panes never shrink; the
               viewport scrolls. Any terminal, no daemon.
badges (release, CI, license)
DEMO GIF  <- recaptured for the agent story, < 2 MB
one-line install:  curl -fsSL .../install.sh | bash    (+ brew, + cargo)
nav row
```

Then, in order: Quick start (~10 lines) → Keyboard shortcuts → Features →
Why gwae (3 sentences) → link out to COMPARISON.md / benchmarks / narrative.

### Work items

- [ ] 4.a Rewrite hero: single sentence, drop "tactile", drop second tagline.
- [ ] 4.b Move the install one-liner above the fold (3 lines: curl / brew / cargo).
- [ ] 4.c Reorder: Quick start and Shortcuts before the essay.
- [ ] 4.d Move "The narrative: why now", the recommended-stack table, and the
      footnotes into `docs/WHY.md`; leave a 3-sentence summary + link.
- [ ] 4.e Keep the benchmark table but collapse it under `<details>` or move
      to `docs/BENCHMARKS.md` with a one-line TL;DR in README.
- [ ] 4.f Recapture the demo around the fleet story; re-encode under 2 MB
      (gifsicle `--lossy=80 -O3`, or use the mp4 with a GIF fallback).
- [ ] 4.g Rewrite the GIF caption to name what is being shown.
- [ ] 4.h Trim badges to 3.

**Acceptance:** a first-time visitor, 10 seconds, no scrolling, can answer
(1) what it does, (2) who it is for, (3) how to install it.

### Social preview (link unfurls)

`docs/assets/social-card.png` exists but is wired nowhere:

- GitHub repo has **no custom Open Graph image** set (unfurls fall back to the
  generic `opengraph.githubassets.com` card). Fix: Settings → Social preview →
  upload `docs/assets/social-card.png`. This is a UI-only action.
- `docs/index.html` has **no `og:image`** and uses `twitter:card=summary`
  (small, text-only). Fix: add `og:image` +
  `og:image:width/height` (1200×630) and switch to `summary_large_image`.

Every HN/Reddit/X link to the site currently unfurls without a picture. This is
one of the highest ratio-of-impact-to-effort fixes on the list.

- [ ] 4.i Add `og:image` + `summary_large_image` to `docs/index.html`.
- [ ] 4.j Upload the social card in repo settings.

---

## 5. Frictionless install

### Current state (verified live)

| Channel | State | Notes |
|---|---|---|
| `curl \| bash` (`scripts/install.sh`) | ✅ serving | checksum-verified, `~/.local/bin` |
| GitHub Releases | ✅ v1.0.1, 10 assets | 5 platforms + `.sha256` each |
| crates.io `cargo install gwae` | ✅ 1.0.1 published | 22 downloads |
| Homebrew tap `hongnoul/homebrew-tap` | ⚠️ exists, **macOS-only** | no `on_linux` branch |
| Homebrew core `brew install gwae` | ❌ | needs 30+ forks/stars & notability |
| AUR | ❌ scaffold only (`packaging/aur/PKGBUILD`), not submitted |
| Nix | ❌ scaffold only (`packaging/nix/flake.nix`), not in nixpkgs |
| Windows | ⚠️ manual zip + PATH | no scoop/winget |

### Problems

| # | Problem | Fix |
|---|---|---|
| 5.1 | Tap formula has no `on_linux` block | Homebrew on Linux is common among the exact devs being targeted; musl assets already exist. Add `on_linux`/`Hardware::CPU.arm?` branches. |
| 5.2 | Formula header says tap `gwae/homebrew-tap`; README says `hongnoul/tap` | Stale comment. The live tap is `hongnoul/homebrew-tap`, so `brew install hongnoul/tap/gwae` is correct. Fix the comment. |
| 5.3 | Tap bumped **by hand** at release time; `release.yml` only undrafts | Guaranteed drift: a release will ship with a stale formula. Add a `bump-tap` job that computes SHA-256s from the published assets and commits to the tap via a PAT. |
| 5.4 | crates.io publish is also manual | Add a `cargo publish` job (workspace order: layout, term, testkit, gwae) gated on the tag. |
| 5.5 | Windows install is a 3-step manual chore | Ship a **Scoop** manifest (single JSON in a `scoop-bucket` repo, near-zero maintenance) and a PowerShell `irm ... | iex` installer. Defer winget until M3 runtime verification. |
| 5.6 | AUR/Nix scaffolds imply support that does not exist | Either publish `gwae-bin` to the AUR (cheap, high visibility with the tiling-WM crowd who are the natural audience for a niri-alike) or say "scaffolds, not yet published" in the README. |
| 5.7 | `install.sh` is a raw.githubusercontent URL | Serve it from the Pages site (`https://hongnoul.github.io/gwae/install.sh`) — shorter, brandable, survives a repo rename. |
| 5.8 | crates.io `homepage` is null | Set `homepage` + `documentation` in `Cargo.toml` so the crates.io page links the site. |

### Sequenced plan

**Phase 1 — before any launch post (must-have)**
1. 5.1 `on_linux` in the tap formula; verify `brew install` on Linux CI.
2. 5.2 fix stale tap comment; make README and formula agree.
3. 5.8 `homepage`/`documentation` in `Cargo.toml`.
4. 4.a–4.d, 4.g–4.j (hero, install-above-fold, reorder, social card).

**Phase 2 — launch week**
5. 5.3 automated tap bump in `release.yml`.
6. 5.4 automated `cargo publish`.
7. 5.5 Scoop bucket + PowerShell one-liner.
8. 4.e, 4.f (benchmark relocation, sub-2 MB demo).

**Phase 3 — after the first traffic wave**
9. 5.6 AUR `gwae-bin` (the niri/tiling crowd is the highest-affinity audience).
10. Nixpkgs PR.
11. Homebrew core once the notability bar is met (stars from launch make this
    possible; it is a lagging indicator, not a lever).

### Acceptance for section 5

On a clean machine per platform, one command installs a working `gwae` and
`gwae doctor` exits 0:

- macOS arm64/x86_64: `brew install hongnoul/tap/gwae`
- Linux x86_64/aarch64: `brew install hongnoul/tap/gwae` **and** `curl | bash`
- Windows: `scoop install gwae`
- Any: `cargo install gwae`

And a tagged release updates all of tap, crates.io, and scoop with no human
step other than pushing the tag.

---

## Validation log: social card / og:image (2026-08-26)

Markup and asset verified locally; **the fix is not live yet**.

Verified:
- All 12 og/twitter tags parse via `html.parser`, no duplicate keys, no missing
  required key, `twitter:card == summary_large_image` exactly once.
- Document is well-formed (no unclosed or mismatched tags).
- Card is 1200x630, ratio 1.905 (inside X's 1.91:1 tolerance), 56.4 KB
  (under X's 5 MB and Facebook's 8 MB caps, above the 200x200 minimum).
- PNG is colortype 6 (RGBA) but **every pixel has alpha 255**, and there is no
  `tRNS` chunk. Transparent cards are a common cause of cards rendering black
  in X/Slack; this one is safe.
- Card renders correctly on visual inspection (wordmark, tagline, three
  claims, repo URL).
- `og:image` and `twitter:image` are absolute URLs (crawlers reject relative),
  serve HTTP 200 as `image/png`, and the served bytes are SHA-256 identical to
  the local file.
- `og:url` and `canonical` both resolve 200.
- Every `assets/` path referenced by `index.html` exists in the repo.

**Blocked on a push.** GitHub Pages for this repo is `build_type: legacy`
serving from `main:/docs`, so the page only updates when `main` is pushed.
The live site still serves `twitter:card=summary` and no `og:image`:

```
curl -sL https://hongnoul.github.io/gwae/ | grep -o '<meta[^>]*og:[^>]*>'
```

`main` is 14 commits ahead of `origin/main`, and those include unrelated
in-flight work (hot reload). Pushing to publish the card would also publish
that. Decide deliberately: either push all of it, or cherry-pick the
`docs/index.html` change onto a branch off `origin/main` and merge that alone.

Re-run the curl above after pushing; it is the only check that confirms the
card is actually live. Then run the URL through X's Card Validator and
Facebook's Sharing Debugger to warm their caches before any launch post,
since both cache aggressively and a pre-launch miss can persist.
