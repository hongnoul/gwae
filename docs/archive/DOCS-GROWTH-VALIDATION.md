# Documentation growth plan: validation and requirement mapping

This appendix records checks of the [research draft](DOCS-GROWTH-PLAN.md). The requested deliverable is a researched strategy and actionable draft, not an implemented redesign, a release, or proof of increased stars. Future experiments are explicitly distinguished from checks performed now.

## Actual-interface acceptance observations

The follow-up downloaded the official **v1.3.1 aarch64 macOS archive** into scratch storage and checked it against its published SHA-256 sidecar. Archive digest: `338f81179bf3e8c7dc235e0ae32a58e904a7d78d710d8c233b9d943f6d805139`. The extracted executable reported `gwae 1.3.1`. No installed binary or user configuration was replaced.

| Requirement or risk | Real interface exercised | Observed result and implication |
|---|---|---|
| The plan must be readable as repository documentation. | GitHub's `POST /markdown` endpoint, GFM mode with repository context. | The pre-appendix plan rendered six tables, nine level-two headings, 18 links, and all six pinned peer links. Final split-document checks are reported below. This checks the actual Markdown renderer, not full GitHub-page styling or mobile appearance. |
| The proposed explicit command must exist in the release, not only in source. | Verified release: `gwae run --help`, with isolated HOME/config and a PATH without agents. | Exit 0. Usage is `gwae run [OPTIONS] [COMMAND]`, with the command replacing the first-pane shell. |
| A no-agent gateway must not be mistaken for an immediate shell. | Verified release: `gwae agent --print`, under the same isolated environment. | Exit 0. It reports no harness on PATH and explicitly says **“Enter alone opens a shell.”** This supports documenting the gateway step rather than silently treating it as a plain shell. It does not establish the complete default-launch interaction. |
| Width changes must reach real shell/TUI processes. | Repository `resize_e2e` suite with `GWAE_E2E_BIN` set to the downloaded release. | Width/fullscreen cycling and host-resize tests passed. Inner PTY columns changed 29 → 39 → 59 → 29, full width reached 119, and actual WINCH/redraw behavior was observed. Host sizes included 96×24, 120×30, and 144×36. |
| Shell text must remain usable after a resize. | The same release suite's `primary_output_reflows_across_width_cycles_without_child_redraw`. | **Failed. Overall release result: 2 passed, 1 failed.** The shell received the new width, but existing primary-screen text retained stale wrapping after 29 → 39 columns. Do not interpret the passing resize-delivery tests as complete resize correctness. |
| Determine whether this needs a new implementation or release follow-through. | Same repository suite against the current development executable, at HEAD `9bdfd76`. | **All 3 passed.** Existing concurrent implementation work fixes this tested path. This plan's task did not modify runtime code. The remaining gate is checking a published artifact containing the fix, not claiming the release already includes it. |

Reproduce the release check with the actual extracted artifact path:

```sh
GWAE_E2E_BIN=/absolute/path/to/downloaded/gwae cargo test -p gwae --test resize_e2e -- --nocapture
```

For the development comparison, omit `GWAE_E2E_BIN`. These are the project's existing tests driving the real executable, shell, inner/outer PTYs, terminal resize events, and rendered output. The shell redraw fixture is controlled, so these checks still do not substitute for a human trying an ordinary terminal or an installed agent.

**Blocked or incomplete checks:** browser status reported ready, but opening both the generated local document and the public GitHub repository returned an unexpected error. The bridge could not validate visual/mobile presentation. A later background Safari check successfully loaded the GitHub-generated HTML and returned 31,106 characters of rendered document text, including the complete opening recommendation. Window capture failed because that background window was not available to the capture API. Safari JavaScript automation was disabled and was not enabled. No screenshot-based, mobile, or full GitHub-page styling result is claimed. An additional scratch-only Python PTY replay stalled, was stopped, and yielded no valid first-run result. It is not counted as evidence of either product success or failure. macOS testing does not validate Linux or Windows installation, package-manager behavior, or physical Option/Alt handling.

**Concrete improvement from the feedback loop:** the plan now adds an artifact-specific release gate, removes resizing from the minimal trial until that gate passes, and distinguishes CLI grammar/gateway evidence from the still-unverified newcomer experience. Increased comprehension and star growth remain unmeasured. Proving those requires the proposed human feedback and post-deployment observation, neither of which can honestly be replaced by automated checks.

## Explicit requirements and changed-output traceability

The original request was to strategize about editing documentation to maximize genuine GitHub stars, research the question, and draft a plan. These are the acceptance requirements of this deliverable. Future product tests and growth experiments in the plan are not falsely marked as already completed.

| Requirement / output | Concrete check | Observed result |
|---|---|---|
| Research this repository rather than give generic growth advice. | Inspect public metadata, published site, working README, comparison/why docs, existing launch materials, CLI source, and asset sizes. | Recorded 9 stars, 15 existing topics, missing custom GitHub preview, distinct published/local copy, existing launch-kit work, and specific conflicting first-run/support claims. The plan links recommendations to named files and existing work. |
| Research comparable projects and credible evidence. | Fetch six peer READMEs at pinned revisions, compare their observed patterns, and read the two cited studies plus official GitHub guidance. Check each peer citation resolves. | All six pinned peer sources returned HTTP 200. Official starring/README/traffic/topic documentation links also returned 200. The plan distinguishes observational associations, design hypotheses, and measured local findings rather than promising a causal uplift. |
| Provide a strategy aimed at genuine star growth. | Independent read-only automated reviewer assessed positioning, discovery, star/save intent, tradeoffs, and measurement. | Reviewer found no major unsupported growth claims and judged the tradeoffs/measurement understandable. This is an automated document review, not a human audience experiment. |
| Give a maintainer an actionable draft and clear next actions. | Reviewer identified the next three actions and assessed whether the document explains how to execute the stated P0 gates. | Reviewer found inconsistent ordering and underspecified participant/platform coverage. The revision now gives three ordered work packages and an explicit five-person protocol with ownership, operating-system coverage, success criteria, and failure handling. |
| Supply concrete documentation edits and experiments, not just slogans. | Check the plan contains proposed hero copy, README ordering, demo storyboard, first-run command, two recipe briefs, comparison criteria, target files, effort estimates, acceptance gates, rollout, and decision rules. | Every element is present. The real release test finding changed the recommended minimal trial and the launch gate. These are draft instructions for implementation, not claims the overhaul has shipped. |
| Make measurement useful without misleading attribution or exposing private data. | Compare the proposed collection windows/metrics with GitHub traffic guidance, check no lifetime-star/recent-visitor conversion claim, and inspect the tracked diff for private snapshots. | The plan separates reach, comprehension, trials, and stars, warns about overlapping 14-day windows and confounders, and includes no exact nonpublic traffic or participant records. |
| Deliver readable, connected repository documentation. | Render both final Markdown files through GitHub's GFM API in the correct repository-relative context. Check table structure, reciprocal links, external research links, and the local browser's rendered document text. | Rendered-output checks and browser observations are recorded below. API rendering is the real repository Markdown interface. Local browser styling is only a preview, not proof of GitHub/mobile appearance. |
| Preserve the request's draft-only scope and unrelated concurrent work. | Inspect each task commit's file list and final working-tree diff. | Task commits contain only `DOCS-GROWTH-PLAN.md` and its validation appendix. No runtime change, push, public post, settings change, or release was performed by this task. |

## Independent review and improvement

An independent automated reviewer raised two medium-priority issues: the overview and priority table implied different next actions, and human/first-run acceptance lacked a directly linked coverage protocol. It also suggested moving detailed test narrative out of the main strategy. All three suggestions were applied: one ordered package sequence, a concrete P0 protocol, and this appendix. The reviewer explicitly found no major unsupported star-growth claim. This does not establish actual reader comprehension or growth.

## Final document integration checks

The plan and appendix are checked as a pair. Their reciprocal relative links must resolve under `docs/`, all tables must preserve their header/data column counts, and each pinned research URL must remain an external clickable link in the plan. Final GitHub render results are recorded after the revision below. Source/test logs and downloaded binaries remain in scratch storage, not in the repository.

- **Plan:** GitHub rendered five tables, nine level-two headings, and 19 links. All rows matched their table's header column count. Six unique pinned peer citations remained clickable, with the niri citation intentionally used twice.
- **Appendix:** GitHub rendered two tables, four level-two headings, and the backlink to the plan. Both tables retained three columns in every row.
- **File integration:** the rendered reciprocal links were present, and both relative targets resolved to the actual files under `docs/`. These are local, committed draft files, not a claim that unpushed GitHub URLs already exist.
- **Review closure:** the independent reviewer re-read the changed sections and confirmed that all three findings were resolved, with no further findings in that follow-up.
- **UI boundary:** the earlier combined plan successfully loaded as rendered text in background Safari. The final split documents passed GitHub rendering and file-link checks. No screenshot or final split-document visual/mobile approval is claimed. The temporary review tab was closed without changing browser security settings or activating the browser.

This closes the feedback loop for the requested research-and-draft deliverable: observed public-interface behavior and independent criticism changed the recommendation and execution instructions, then the revised artifacts were rechecked. Product release readiness, human comprehension, and actual star growth remain separate future gates, not unobserved successes attributed to this work.
