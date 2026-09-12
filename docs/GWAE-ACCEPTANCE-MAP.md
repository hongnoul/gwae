# GWAE tdf acceptance map

This map separates **requirements traced to evidence** from **requirements
fully satisfied**. A blocked or failing row is not counted as passing. The
scope is the pane geometry/query repair (`ccda77a`), the native graphics
implementation (`39efea8`), and the actual Yazi-to-tdf workflow they enable.
It is not a claim of general Kitty support or native Ghostty parity.

## Evidence classes

- **A, actual application acceptance:** installed GWAE, actual Yazi and actual
  tdf binaries, ordinary opener/key input, real child PTYs and process checks.
  Python only drives input and records output. No viewer, renderer, file opener
  or terminal reply inside GWAE is stubbed. The outer PTY supplies geometry but
  has no GPU, so these observations stop at the emitted pixels and terminal
  stream. They cannot pass the separate Ghostty framebuffer requirement.
- **P, public executable integration:** actual GWAE executable and operating
  system PTYs, with a purpose-built child exercising terminal protocols. This
  is not a replacement for A.
- **U, project unit/component test:** the actual Cargo project implementation,
  not a copied implementation. Useful for boundaries and malformed input, but
  not a replacement for A or the missing GUI acceptance.
- **B, blocked acceptance:** the real GUI capture path was attempted and failed.

Private artifacts referenced below are under `$JCODE_SCRATCH_DIR`:

| Ref | Artifact and observer |
| --- | --- |
| A1 | `gwae-native-graphics-20260912/pty-workflow-results.json`, its per-step raw captures and `confidence-audit-results.json`. The independent audit matches all nine PNGs to their actual RGBA uploads, not just saved hash labels. |
| A2 | `gwae-native-graphics-20260912/lifecycle/run-1789179285580928000` and `run-1789179401883849000`. Independent host-packet parsing, per-cell frame replay, raw child image commands, process identities and recorder ioctl parity. |
| A3 | `gwae-native-graphics-20260912/acceptance-final-20260912T0226`. Fresh installed real Yazi/tdf workflow, completed in 7.8 seconds. Same full predicates as A1, separate artifacts. |
| A4 | `tdf-native-resize-verify-20260912T021356Z` and `repeat-with-cleanup/`. Direct real tdf child under installed GWAE, no nested recorder. Independently asserted native patched/unpatched comparison and final cleanup. |
| P1 | `gwae-native-graphics-20260912/installed-e2e.log`. The nine named executable cases below passed against the installed path. |
| U1 | `gwae-native-graphics-20260912/workspace-serial.log`. Individual named test outcomes, not only the aggregate count. |
| U2 | `gwae-native-graphics-20260912/traceability-term-tests.log` and `traceability-graphics-tests.log`. Fresh direct public-API/reset regressions and their containing target suites. |
| B1 | Background/off-Space Ghostty window capture and ScreenCaptureKit failed, including error -3811. The test window was not forced over the user's active app. See the [validation ledger](GWAE-GRAPHICS-VALIDATION.md). |

## User-facing requirements and observed improvement

| ID | Requirement | Concrete acceptance observation | Result |
| --- | --- | --- | --- |
| R01 | Enter in Yazi must open the PDF instead of hanging in a pixel-size query. | A1/A3: normal selection and Enter start the actual tdf process for the selected path, reach `Rendered: 100%`, and emit a complete 560x736 RGBA page. Before the repair, the sampled actual tdf process was blocked in `get_font_size_through_stdio`. | **PASS**, actual application path. |
| R02 | The PDF must use readable image pixels rather than broken sizing or halfblocks. | A1/A3: native upload contains 560x736 RGBA pixels. The independently decoded PNG equals those exact bytes and shows readable PDF text. No claim is made about final GPU presentation. | **PASS** at actual application/wire boundary. R09 remains separate. |
| R03 | Native fit/fill and zoom must work, not merely advertise Kitty support. | A1/A3: Kitty help has no fallback warning. `z` changes fit560x736 to fill560x960. `o` changes the crop pixels, `O` restores the exact fill hash, and `z` restores the exact fit hash. | **PASS**, reversible actual image data. |
| R04 | Page navigation must render the requested page. | A1/A3: `l` produces a different actual page texture. `h` restores the original page hash. | **PASS**, actual viewer input and pixels. |
| R05 | Paths with spaces/Unicode, sequential selected PDFs, and return to Yazi must work. | A1/A3: two selected paths launch distinct tdf PIDs in sequence. After both `q` inputs no tdf remains, the original Yazi PID redraws both filenames and `NOR`, and `k` produces a fresh `1/2` selection. | **PASS**, actual installed opener and processes. |
| R06 | Pane geometry must track fullscreen/shrink and host/font changes without borrowing a sibling's size. | P1 named startup, host/font resize, independent-pane, new-pane and reload cases below check exact ioctl/reply values. A2 records exact child ioctl parity during quarter/full/quarter transitions. | **PASS** for GWAE geometry, not immediate upstream viewer fit. |
| R07 | Two real image viewers must remain isolated and clipped during navigation and pane lifecycle. | A2: both actual children use image ID1 but receive distinct host IDs. Images stay in their pane bounds, hide/reveal deletes/reuploads only owned textures without retransmitting the sibling source, page navigation leaves the sibling hash/ID unchanged, and both normal exits clean up. | **PASS**, actual two-viewer wire/grid path. |
| R08 | The page should refit immediately after resizing, without a compensating key. | A4: installed tdf still keeps its old280x368 page on enlargement and an8x800 clipped strip on shrink until `x`. The scratch patch instead produces624x800 and then280x368 with no extra input, restoring exact hashes. | **KNOWN FAILURE in installed upstream tdf**. Fix verified locally, not installed. |
| R09 | The final PDF should visibly render correctly in Ghostty's GPU framebuffer. | B1: actual background capture attempted but failed. Decoded uploads and project-grid replay are explicitly not substitutes. | **BLOCKED / DEFERRED, not passed**. |
| R10 | The tested GWAE repair must be the delivered executable. | A1/A3/P1 record the installed path. Installed and release SHA-256 both equal `90a7a810a90b72885dae643c9718a8728eb753764dbaaea7165f408e945f979d`. Code signature verifies and the previous binary is backed up. | **PASS**. Follow-up changes add tests/docs only. |

## Changed public outputs and integration boundaries

Names are exact Cargo test names or suffixes resolvable uniquely in the cited
logs. Each row states the observed behavior, not just a test count.

| ID | Public output / boundary | Concrete check and observed result |
| --- | --- | --- |
| O01 | Child startup `TIOCGWINSZ` cell and pixel dimensions. | P1 `initial_child_ioctl_and_fragmented_coalesced_queries_match_pane_pixels`: first child ioctl and split/coalesced replies match its drawable pane. U1 `measured_cells_exclude_fractional_window_padding`, `unknown_or_invalid_metrics_are_not_fabricated`, `extreme_dimensions_do_not_overflow_or_disagree_with_cell_metrics`: fractional padding excluded, unknown metrics stay zero, overflow clamped. **PASS**. |
| O02 | Updated dimensions after host resize, font-only change, spawning, fullscreen and reload. | P1 `host_resize_and_pixel_only_font_change_update_ioctl_and_query_replies`, `newly_spawned_pane_first_ioctl_uses_current_host_pixels`, `hot_reload_inherits_live_pty_and_preserves_pixel_query_parity`: exact changed values and inherited PTY parity. A2 also observes fullscreen child geometry. **PASS**. |
| O03 | CSI14/16/18 pixel/cell replies, DA/status/cursor replies, and per-child ordering. | P1 `differently_sized_panes_receive_only_their_own_query_responses`; U1 `query_replies_preserve_order_at_every_byte_boundary`, `geometry_replies_use_current_measurements_and_resize`, `dsr_uses_live_cursor_even_when_scrollback_is_visible`: exact ordered bytes, live cursor and no cross-pane reply. **PASS**. |
| O04 | Query parsing must not consume opaque payloads or alter title/synchronized-output behavior. | U1 `query_like_payload_in_osc_and_apc_does_not_reply`, `csi_16t_rejects_payloads_cancelled_and_nonplain_sequences`, `queries_do_not_change_titles_or_title_stack_behavior`, `synchronized_output_flush_preserves_reply_order`: no fabricated payload replies, preserved title stack and ordered replies. **PASS**. |
| O05 | Graphics enabled/disabled capability and error replies. | P1 `graphics_enabled_real_child_probe_errors_and_quiet_canonical_upload` observes direct query OK and unsupported transport/compression ENOTSUP. `graphics_disabled_real_child_probe_has_no_false_ok_or_host_forwarding` observes neither OK nor host upload. A1/A3 actually select Kitty mode. **PASS** for tested override/public protocol behavior. Host GPU support is still inferred, not negotiated. |
| O06 | Accepted RGB/RGBA transfer bytes, queries, atomic commits and quiet modes. | U1 `every_aligned_chunk_split_acks_only_final_and_uses_final_cursor`, `quiet_modes_and_final_chunk_quiet_override`, `strict_base64_padding_and_all_byte_values`, `query_same_id_never_replaces_source_or_changes_placements`, `rejects_invalid_sizes_formats_media_and_payloads_truthfully`: final-only commit/ack, exact quiet behavior, strict payload checks and no query replacement. A1/A3 exercise real RGBA uploads. **PASS**. |
| O07 | Image-number lookup, placement changes, crop/aspect/offset and cursor policy. | U1 `numbered_images_allocate_distinct_ids_and_resolve_newest`, `placement_limits_upserts_and_retransmission_remove_old_placements`, `crop_intersection_aspect_ratio_offsets_and_cursor_policy`, `graphics_cursor_advance_precedes_following_text_and_cursor_query`: exact selected sources, placement lifecycle and cursor/reply ordering. A1/A3 verify reversible real zoom crops. **PASS**. |
| O08 | Resource-limit rejection and failed replacement preservation. | U1 `image_and_total_budgets_count_limits_and_atomic_failed_replacement`, `image_placement_and_allocated_source_budgets_are_enforced`, `native_and_legacy_share_one_live_host_budget`, `stale_budget_is_reclaimed_before_replacement_admission`: native/legacy counts and byte budgets enforced without losing valid prior images. **PASS**, project boundary checks. |
| O09 | APC split/cancellation/overflow behavior and surrounding text. | U1 `mixed_control_streams_are_lossless_and_chunk_invariant`, `exactly_64_kib_including_framing_is_accepted_with_bounded_capacity`, `one_byte_over_limit_rejects_even_when_overflow_is_final_st_byte`, `unterminated_attack_cannot_grow_retained_memory_or_repeat_rejection_events`, `discarded_oversized_continuation_cannot_commit_a_partial_image`: exact boundary acceptance, bounded rejection and no partial commit. **PASS**. |
| O10 | Legacy U1 compatibility must not forward child media paths or invalid commands. | P1 `graphics_legacy_external_media_cannot_escape_via_visible_placeholders`: visible file/shared-memory attempts return ENOTSUP with zero host uploads/placeholders. U1 `forbidden_media_compression_and_controls_never_reach_host` and `nonquiet_virtual_starts_are_rejected_without_hanging_clients`: other forbidden transports/controls rejected and nonquiet callers receive errors. **PASS**. |
| O11 | Legacy PNG, raw chunks and sparse Unicode-placeholder coordinates. | U1 `all_aligned_raw_chunk_splits_are_atomic_and_canonical`, `png_is_validated_sanitized_and_transmitted_canonically`, `png_huge_headers_crc_critical_chunks_bad_order_and_truncation_rejected`, `png_actual_inflation_filter_bytes_checksum_and_trailing_streams_are_bounded`, `png_palette_transparency_16bit_and_adam7_are_supported`, `sparse_marks_inherit_row_column_high_byte_and_survive_clipping`: supported pixels preserved, malformed PNG rejected, sparse coordinates resolved before remapping. **PASS**, project boundary checks. |
| O12 | Shared native/legacy child image namespace. | U1 `mixed_mode_child_ids_switch_only_after_successful_source_commit` and `dispatcher_replaces_native_source_only_after_successful_legacy_commit`: pending/query/failed operations preserve the prior source, successful opposite-mode commit replaces it. **PASS**. |
| O13 | Host uploads, chunks, IDs and deletion ownership. | P1 `graphics_two_real_panes_own_same_client_id_and_survive_sibling_deletion`; U1 `canonical_upload_chunks_are_bounded_quiet_and_direct`, `overlapping_child_ids_are_isolated_and_only_owned_ids_are_deleted`: distinct owned IDs, bounded quiet direct uploads and no unowned deletion. A2 independently parses actual two-viewer traffic and cleanup. **PASS**. |
| O14 | Raster scale, clipping/padding, layer order and wide tile coordinates. | U1 `crop_scale_and_cell_padding_are_applied_locally`, `bilinear_rgb_scaling_has_exact_endpoints_and_opaque_alpha`, `clipped_tiles_emit_explicit_coordinates_and_never_touch_outside_cells`, `equal_depth_orders_by_child_image_id_not_transmission_order`, `wide_scenes_split_before_diacritic_index_overflow`: exact endpoints, pane-safe cells, defined equal-z order and bounded coordinates. A2 checks real clipped rectangles. **PASS** at wire/grid boundary, not GPU. |
| O15 | Host hide/reveal, revision refresh and local source reuse. | U1 `hidden_images_release_host_storage_and_reveal_reuploads_local_source`, `eviction_refresh_reuploads_without_changing_placeholder_ids`, `virtual_placement_revision_and_footprint_changes_upload_immediately`; A2: owned hide deletion, correct reveal hash and unchanged sibling child transmit count. **PASS**. |
| O16 | Pane-local source/placement deletes and pending cancellation. | U1 `deletion_is_pane_local_and_lowercase_keeps_sources`, `delete_aborts_pending_and_interleaving_cannot_splice_images`, `deletes_are_scoped_preserve_sources_and_respect_virtual_templates`, `every_delete_aborts_pending_including_physical_and_unsupported_deletes`: defined source retention and scoped deletion without splicing pending data. A2 normal exits preserve the sibling then clear all owned state. **PASS**. |
| O17 | Alternate-screen and RIS reset epoch/source invalidation. | U1 `alternate_screen_enter_and_leave_in_one_text_event_clear_graphics`; new U2 `screen_epoch_counts_alt_pairs_and_ris_at_every_split`, `ris_clears_visible_and_pending_graphics_at_every_split`: same-read alt enter/leave is detected, RIS increments epoch, both visible and partial sources clear at every split. **PASS**. |
| O18 | New public `Style.underline_color` mapping and reset semantics. | New U2 `sgr58_underline_color_and_resets_survive_every_split`: semicolon RGB, indexed color, colon RGB, SGR59 and SGR0 map/reset exactly at every split. A2 decoder observes the actual host placement ID carried by this field. **PASS**. |
| O19 | New public graphics cursor absolute placement, clamping and wrap state. | New U2 `graphics_cursor_is_absolute_clamped_and_clears_pending_wrap`: cursor positioning ignores DEC origin mode, clamps to the grid, clears pending wrap and subsequent text appears at the asserted cells. **PASS**. |
| O20 | Compositor style normalization and suppression of unmapped/no-host placeholders. | U1 `native_overlay_is_visible_over_styled_text_and_no_host_means_no_placeholders`: native image cells normalized and absent capability cannot leak placeholder glyphs. P1 external-media rejection and A2 actual frame replay check visible output. **PASS** at cell/wire boundary. |

## Explicit non-claims

- R08 and R09 remain unsatisfied in the delivered end-user environment. They
  are mapped, not hidden by successful unit counts or cancelled todo status.
- File/temp/shared-memory transport, traditional PNG, outer compression,
  animation and negative-z/general Kitty stacking are not supported features.
  Their rejection is the intended public output, mapped above.
- Alpha compositing replaces text cells, not the underlying glyphs. No full
  transparent-text parity is claimed. A binary reload requires the child to
  retransmit images. The reload acceptance above covers live PTY geometry,
  not preservation of image source maps.
- No live WezTerm/Kitty GPU acceptance was performed. Environment inference and
  `GWAE_KITTY_GRAPHICS=1` do not prove that an arbitrary host supports rendering.

The original request is demonstrably improved at its actual application
boundary, but **full end-user visual acceptance is not closed**. See the
[validation ledger](GWAE-GRAPHICS-VALIDATION.md) for source-level test details,
artifacts, failed measurement attempts and delivery hashes.
