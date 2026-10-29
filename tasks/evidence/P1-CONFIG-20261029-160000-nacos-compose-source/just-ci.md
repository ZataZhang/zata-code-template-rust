# `SQLX_OFFLINE=true just ci`

- Exit status: `0`
- Scope: formatting, workspace clippy, regular workspace tests, and rustdoc.
- Four PostgreSQL/Redis integration tests remained intentionally ignored in the normal `just ci` target.

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
    Checking tpl-infrastructure v0.1.0 (<repo>/crates/infrastructure)
    Checking tpl-accounts v0.1.0 (<repo>/services/accounts)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.93s
   Compiling aws-lc-sys v0.45.0
   Compiling aws-lc-rs v1.18.1
   Compiling rustls v0.23.46
   Compiling rustls-webpki v0.103.16
   Compiling tokio-rustls v0.26.6
   Compiling rustls-platform-verifier v0.7.1
   Compiling hyper-rustls v0.27.10
   Compiling reqwest v0.13.5
   Compiling tpl-infrastructure v0.1.0 (<repo>/crates/infrastructure)
   Compiling tpl-accounts v0.1.0 (<repo>/services/accounts)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 11.70s
     Running unittests src/lib.rs (target/debug/deps/tpl_accounts-f9db0577a5e73bd4)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/main.rs (target/debug/deps/accounts_service-44f1b7252a908488)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/bin/compose_config.rs (target/debug/deps/compose_config-b80e017412e064bf)

running 4 tests
test tests::reserved_compose_file_and_project_overrides_are_rejected ... ok
test tests::legacy_env_file_is_rejected_when_set ... ok
test tests::service_selection_and_non_reserved_flags_are_allowed ... ok
test tests::config_output_and_arbitrary_command_execution_are_restricted ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/architecture.rs (target/debug/deps/architecture-af657b2ba0683d58)

running 2 tests
test checker_rejects_forbidden_and_unregistered_dependencies ... ok
test layer_dependencies_follow_the_allowed_direction ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

     Running tests/http_end_to_end.rs (target/debug/deps/http_end_to_end-00b855b6e61c4d0f)

running 5 tests
test admin_console_flow_against_real_postgres_and_redis ... ignored, 需要真实 PostgreSQL 与 Redis，设置 TEST_DATABASE_URL、TEST_REDIS_URL 后用 `just test real` 运行
test admin_login_throttling_is_stricter_and_separate_from_user_login ... ignored, 需要真实 PostgreSQL 与 Redis，设置 TEST_DATABASE_URL、TEST_REDIS_URL 后用 `just test real` 运行
test full_auth_flow_against_real_postgres_and_redis ... ignored, 需要真实 PostgreSQL 与 Redis，设置 TEST_DATABASE_URL、TEST_REDIS_URL 后用 `just test real` 运行
test login_throttling_uses_the_real_tcp_peer_address_and_ignores_a_spoofed_forwarded_header ... ignored, 需要真实 PostgreSQL 与 Redis，设置 TEST_DATABASE_URL、TEST_REDIS_URL 后用 `just test real` 运行
test probes_work_over_a_real_socket_without_any_external_service ... ok

test result: ok. 1 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 0.01s

     Running unittests src/lib.rs (target/debug/deps/tpl_api-c83d1b12886327ca)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/admin_routes.rs (target/debug/deps/admin_routes-3ce70a42e0bfe9cb)

running 11 tests
test every_admin_route_requires_an_admin_session ... ok
test state_changing_requests_must_be_json ... ok
test admin_login_is_throttled_without_locking_out_ordinary_users ... ok
test login_sets_an_httponly_admin_cookie_and_returns_the_admin ... ok
test unknown_username_and_wrong_password_look_identical ... ok
test admin_routes_answer_503_when_admin_is_not_configured ... ok
test logout_invalidates_the_session_and_is_idempotent ... ok
test disabling_takes_effect_immediately_and_enabling_restores_access ... ok
test listing_paginates_filters_and_validates_parameters ... ok
test unknown_and_malformed_user_ids_are_404 ... ok
test user_and_admin_sessions_are_not_interchangeable ... ok

test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

     Running tests/auth_routes.rs (target/debug/deps/auth_routes-777f9c4483ca9e7e)

running 21 tests
test logout_without_a_session_is_still_a_success ... ok
test login_sets_a_hardened_session_cookie ... ok
test auth_routes_answer_503_when_auth_is_not_configured_but_health_still_works ... ok
test an_unparseable_forwarded_header_falls_back_to_the_tcp_peer ... ok
test logging_in_again_replaces_the_previous_session ... ok
test logout_revokes_the_session_server_side_and_clears_the_cookie ... ok
test a_spoofed_x_forwarded_for_cannot_evade_the_ip_limit_when_it_is_not_trusted ... ok
test me_requires_a_valid_session ... ok
test behind_a_trusted_proxy_the_rightmost_forwarded_address_is_used_and_spoofed_entries_are_ignored ... ok
test registering_a_duplicate_email_is_409_regardless_of_case ... ok
test register_returns_the_normalised_user_and_does_not_log_in ... ok
test register_rejects_bad_input_with_specific_error_codes ... ok
test secure_attribute_follows_configuration ... ok
test registration_is_throttled_per_ip_with_429 ... ok
test write_endpoints_only_accept_json ... ok
test malformed_and_oversized_bodies_are_rejected_before_any_work ... ok
test too_many_failed_logins_get_429_with_retry_after_and_even_the_right_password_is_refused ... ok
test wrong_password_and_unknown_email_give_the_same_response ... ok
test responses_never_contain_passwords_or_hashes ... ok
test requests_without_a_peer_address_still_work_and_are_only_throttled_per_account ... ok
test the_throttled_response_does_not_reveal_whether_the_account_exists ... ok

test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

     Running tests/health_routes.rs (target/debug/deps/health_routes-6b730583c804ef7d)

running 5 tests
test unknown_path_is_404 ... ok
test health_reports_the_crate_version ... ok
test ready_is_200_when_every_dependency_is_up ... ok
test live_is_ok_even_when_dependencies_are_down ... ok
test ready_is_503_and_hides_the_failure_reason_when_a_dependency_is_down ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

     Running unittests src/lib.rs (target/debug/deps/tpl_core-07e4ead438649877)

running 43 tests
test admin::tests::credentials_never_appear_in_debug_output ... ok
test admin::tests::bootstrap_rejects_bad_usernames_and_weak_passwords ... ok
test admin::tests::a_token_that_was_never_issued_does_not_authenticate ... ok
test admin::tests::listing_users_paginates_and_reports_the_total ... ok
test admin::tests::bootstrap_creates_the_first_admin_and_never_overwrites_an_existing_one ... ok
test admin::tests::listing_filters_by_status_and_by_a_case_insensitive_trimmed_keyword ... ok
test admin::tests::login_then_authenticate_then_logout ... ok
test admin::tests::enabling_and_disabling_is_idempotent_and_unknown_users_are_none ... ok
test admin::tests::usernames_are_trimmed_lowercased_and_restricted ... ok
test admin::tests::success_clears_the_admin_account_counter_but_not_the_ip_counter ... ok
test admin::tests::an_admin_account_is_throttled_and_even_the_right_password_is_refused ... ok
test admin::tests::admin_and_user_login_counters_never_affect_each_other ... ok
test admin::tests::unknown_username_and_wrong_password_are_indistinguishable_and_create_no_session ... ok
test auth::tests::a_disabled_user_cannot_log_in_but_only_learns_that_after_the_right_password ... ok
test auth::tests::an_account_is_throttled_across_ips_and_even_the_right_password_is_refused ... ok
test auth::tests::email_is_trimmed_and_lowercased ... ok
test auth::tests::disabling_a_user_invalidates_an_existing_session_on_the_very_next_check ... ok
test auth::tests::implausible_emails_are_rejected ... ok
test auth::tests::an_attacker_cannot_dodge_the_ip_limit_by_logging_into_their_own_account ... ok
test auth::tests::an_ip_is_throttled_across_accounts_but_other_ips_are_unaffected ... ok
test auth::tests::ipv6_addresses_in_one_slash_64_share_a_limit ... ok
test auth::tests::logging_in_twice_creates_independent_sessions ... ok
test auth::tests::password_length_rules_count_chars_for_min_and_bytes_for_max ... ok
test auth::tests::missing_ip_falls_back_to_account_limit_and_bad_email_falls_back_to_ip_limit ... ok
test auth::tests::register_validates_before_touching_storage ... ok
test auth::tests::register_then_login_then_authenticate_then_logout ... ok
test admin::tests::paging_inputs_are_clamped_instead_of_trusted ... ok
test auth::tests::secrets_never_appear_in_debug_output ... ok
test auth::tests::registering_the_same_email_twice_is_a_conflict_even_with_different_case ... ok
test auth::tests::registration_is_limited_per_ip_and_every_attempt_counts ... ok
test auth::tests::retry_after_is_the_longest_remaining_wait_among_exceeded_limits ... ok
test auth::tests::user_status_round_trips_and_rejects_unknown_values ... ok
test auth::throttle::tests::different_scopes_never_share_a_counter_key ... ok
test auth::tests::unknown_email_and_wrong_password_are_indistinguishable ... ok
test auth::throttle::tests::ipv4_addresses_are_keyed_individually ... ok
test auth::throttle::tests::ipv6_addresses_in_the_same_slash_64_share_a_key ... ok
test auth::tests::success_clears_the_account_counter_but_not_the_ip_counter ... ok
test auth::tests::unknown_emails_are_counted_so_throttling_does_not_reveal_which_accounts_exist ... ok
test auth::throttle::tests::limits_depend_on_what_is_known_about_the_request ... ok
test auth::throttle::tests::login_and_register_use_separate_counters_for_the_same_ip ... ok
test health::tests::not_ready_when_any_probe_fails_and_order_is_kept ... ok
test health::tests::ready_when_every_probe_passes ... ok
test health::tests::ready_when_there_are_no_dependencies ... ok

test result: ok. 43 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/lib.rs (target/debug/deps/tpl_engines-ce542e7b57ecbece)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/lib.rs (target/debug/deps/tpl_infrastructure-5d80cf33db7f077a)

running 39 tests
test config::compose::tests::oversized_content_is_rejected ... ok
test config::compose::tests::malformed_lines_and_control_characters_are_rejected ... ok
test config::compose::tests::non_secret_compose_values_are_validated_before_process_launch ... ok
test config::compose::tests::unknown_keys_are_rejected_without_echoing_values ... ok
test config::compose::tests::duplicate_keys_are_rejected_without_echoing_values ... ok
test config::compose::tests::required_passwords_must_be_present_and_non_empty ... ok
test config::compose::tests::comments_and_empty_lines_are_ignored ... ok
test config::compose::tests::valid_ports_and_proxy_cidrs_are_accepted ... ok
test config::compose::tests::values_are_kept_literally_including_shell_syntax_equals_and_spaces ... ok
test config::tests::debug_output_never_contains_connection_string_passwords ... ok
test password::tests::corrupted_stored_hash_is_an_error_not_a_silent_false ... ok
test config::nacos::tests::locator_validation_errors_do_not_echo_invalid_values ... ok
test config::nacos::tests::locator_selects_distinct_application_and_compose_data_ids ... ok
test persistence::admin_repository::tests::create_find_and_reject_duplicates_against_a_real_postgres ... ignored, 需要真实 PostgreSQL，设置 TEST_DATABASE_URL 后用 `just test real` 运行
test persistence::tests::probe_passes_against_a_real_postgres ... ignored, 需要真实 PostgreSQL，设置 TEST_DATABASE_URL 后用 `just test real` 运行
test persistence::user_repository::tests::create_find_and_reject_duplicates_against_a_real_postgres ... ignored, 需要真实 PostgreSQL，设置 TEST_DATABASE_URL 后用 `just test real` 运行
test persistence::user_repository::tests::list_filter_paginate_and_toggle_status_against_a_real_postgres ... ignored, 需要真实 PostgreSQL，设置 TEST_DATABASE_URL 后用 `just test real` 运行
test redis_attempts::tests::concurrent_increments_are_not_lost ... ignored, 需要真实 Redis，设置 TEST_REDIS_URL 后用 `just test real` 运行
test redis_attempts::tests::counts_up_reports_the_window_and_resets ... ignored, 需要真实 Redis，设置 TEST_REDIS_URL 后用 `just test real` 运行
test config::tests::admin_bootstrap_needs_both_values_and_accepts_both_env_spellings ... ok
test redis_attempts::tests::redis_key_never_contains_the_logical_key ... ok
test redis_attempts::tests::the_window_is_fixed_and_not_extended_by_later_attempts ... ignored, 需要真实 Redis，设置 TEST_REDIS_URL 后用 `just test real` 运行
test redis_session::tests::hex_encoding_is_lowercase_and_zero_padded ... ok
test redis_session::tests::malformed_redis_url_is_rejected_up_front ... ok
test redis_session::tests::namespaces_isolate_user_and_admin_sessions ... ignored, 需要真实 Redis，设置 TEST_REDIS_URL 后用 `just test real` 运行
test redis_session::tests::probe_passes_against_a_real_redis ... ignored, 需要真实 Redis，设置 TEST_REDIS_URL 后用 `just test real` 运行
test config::tests::wrongly_typed_value_is_an_error_not_a_silent_default ... ok
test redis_session::tests::session_key_hides_the_token_and_is_deterministic ... ok
test redis_session::tests::session_lifecycle_against_a_real_redis ... ignored, 需要真实 Redis，设置 TEST_REDIS_URL 后用 `just test real` 运行
test redis_session::tests::sessions_expire_after_their_ttl ... ignored, 需要真实 Redis，设置 TEST_REDIS_URL 后用 `just test real` 运行
test config::tests::environment_overrides_toml_and_database_url_is_recognised ... ok
test config::tests::toml_file_overrides_defaults ... ok
test config::tests::defaults_apply_when_nothing_is_configured ... ok
test password::tests::unicode_passwords_round_trip ... ok
test password::tests::the_same_password_gets_a_different_salt_each_time ... ok
test password::tests::hashed_password_verifies_and_wrong_password_does_not ... ok
test redis_session::tests::probe_reports_failure_when_redis_is_unreachable ... ok
test persistence::tests::probe_reports_failure_when_database_is_unreachable ... ok
test redis_attempts::tests::unreachable_redis_is_an_error_not_a_silent_zero ... ok

test result: ok. 28 passed; 0 failed; 11 ignored; 0 measured; 0 filtered out; finished in 4.01s

     Running unittests src/lib.rs (target/debug/deps/xtask-56ae62821e41d8b0)

running 81 tests
test hooks::tests::guard_tests_are_recognised_by_their_header_marker_only ... ok
test hooks::tests::guard_decision_requires_an_explicit_acknowledgement ... ok
test git::tests::relative_path_walks_up_and_down ... ok
test hooks::tests::only_blank_lines_are_excluded_from_the_count ... ok
test hooks::tests::prd_stems_are_extracted_from_pending_and_evidence_paths_only ... ok
test hooks::tests::a_missing_path_is_an_error ... ok
test hooks::tests::files_over_the_limit_fail_unless_warn_only_and_the_boundary_is_inclusive ... ok
test hooks::tests::allow_listed_files_only_warn_and_a_missing_allow_list_is_an_error ... ok
test hooks::tests::lock_conflict_warns_only_about_other_sessions_fresh_locks ... ok
test hooks::tests::directories_expand_recursively_with_glob_and_skip_build_directories ... ok
test hooks::tests::glob_matching_handles_names_and_path_tails ... ok
test git::tests::main_repo_root_falls_back_to_the_nearest_tasks_directory_outside_git ... ok
test prd::check::tests::a_wrapped_oracle_block_is_rejected_by_shape_before_content ... ok
test prd::check::tests::executor_metadata_leaking_into_part_a_is_rejected ... ok
test prd::check::tests::executor_owed_unchecked_items_block_delivery_but_not_structure ... ok
test prd::check::tests::delivery_scalar_fields_use_closed_vocabularies ... ok
test prd::check::tests::human_reviewer_oracle_needs_a_presentation ... ok
test prd::check::tests::keeping_the_no_executable_behavior_sentence_does_not_waive_a_real_oracle_block ... ok
test prd::check::tests::interpretation_echo_needs_enough_example_rows_and_both_blocks ... ok
test prd::check::tests::invalid_tier_and_reviewer_values_are_rejected ... ok
test prd::check::tests::delivery_banner_must_exist_and_agree_with_section_8 ... ok
test prd::check::tests::missing_and_out_of_order_sections_are_reported ... ok
test prd::check::tests::functional_requirement_ids_must_be_sequential_from_one ... ok
test prd::check::tests::missing_part_headings_are_reported ... ok
test prd::check::tests::open_human_confirmed_items_never_block_delivery_or_archive ... ok
test prd::check::tests::oracle_missing_base_fields_are_named ... ok
test prd::contract::tests::banner_only_counts_inside_blockquotes ... ok
test prd::contract::tests::banner_takes_the_first_status_word_so_template_hints_do_not_flip_the_state ... ok
test prd::check::tests::resolved_marker_is_not_counted_as_owed ... ok
test prd::check::tests::oracle_depth_scales_with_tier ... ok
test prd::check::tests::untiered_oracle_is_treated_as_the_strictest_tier ... ok
test prd::contract::tests::checklist_counts_the_three_mark_kinds_and_ignores_fences_and_other_sections ... ok
test prd::check::tests::no_executable_behavior_marker_waives_the_oracle_block ... ok
test prd::contract::tests::inline_bold_emphasis_is_not_mistaken_for_a_group_label ... ok
test prd::contract::tests::missing_checklist_section_is_reported ... ok
test prd::check::tests::none_with_a_parenthetical_rationale_still_means_no_dependency ... ok
test prd::contract::tests::group_label_prefix_match_is_case_insensitive_and_bold_label_is_accepted ... ok
test prd::contract::tests::human_group_scope_includes_nested_subheadings ... ok
test prd::contract::tests::contract_version_matches_the_marker_in_the_skill ... ok
test prd::check::tests::archive_requires_a_banner_that_agrees_with_the_human_group ... ok
test prd::check::tests::the_unfilled_skill_template_is_rejected_for_the_right_reasons ... ok
test prd::contract::tests::change_log_names_the_missing_fields ... ok
test prd::deps::tests::parses_gate_and_nested_refs_and_drops_none ... ok
test prd::filename::tests::unparseable_name_falls_back_to_the_stem ... ok
test prd::filename::tests::legacy_name_without_priority_or_kind_still_yields_date_and_slug ... ok
test prd::filename::tests::full_name_is_split_into_priority_kind_date_and_slug ... ok
test prd::check::tests::the_baseline_fixture_passes_every_gate ... ok
test prd::deps::tests::soft_gate_only_hints_and_none_gate_is_quiet ... ok
test prd::deps::tests::dangling_and_issue_refs_are_unknown_not_silently_ok ... ok
test prd::filename::tests::only_prd_shaped_files_in_the_right_directory_are_recognised ... ok
test prd::deps::tests::hard_gate_is_blocked_by_a_pending_upstream_and_ok_once_it_is_archived ... ok
test prd::check::tests::archive_requires_a_filled_in_final_reconciliation ... ok
test prd::lock::tests::a_corrupt_lock_file_counts_as_stale_instead_of_blocking_forever ... ok
test prd::lock::tests::a_lock_just_inside_the_window_is_still_fresh ... ok
test prd::lock::tests::another_session_is_refused_while_the_lock_is_fresh_and_sees_who_holds_it ... ok
test prd::lock::tests::a_stale_lock_is_archived_and_taken_over ... ok
test prd::lock::tests::timestamps_without_a_timezone_or_unparseable_are_untrusted_and_therefore_stale ... ok
test prd::lock::tests::a_lock_taken_at_the_main_repo_is_handed_off_to_the_worktree_that_picks_it_up ... ok
test prd::lock::tests::a_stale_heartbeat_does_not_allow_takeover_while_the_holder_worktree_is_still_being_written ... ok
test prd::lock::tests::claiming_again_from_the_same_worktree_is_idempotent_and_renews_the_heartbeat ... ok
test prd::lock::tests::claiming_a_free_prd_creates_a_complete_lock_file ... ok
test prd::lock::tests::heartbeat_renews_only_for_the_owner ... ok
test prd::lock::tests::activity_in_ignored_directories_does_not_count_as_the_holder_being_alive ... ok
test prd::review::tests::a_prd_can_be_named_by_path_filename_stem_or_slug_in_either_directory ... ok
test prd::review::tests::interactive_checklist_beats_static_which_beats_the_evidence_report ... ok
test prd::lock::tests::release_requires_ownership_unless_forced ... ok
test prd::status::tests::long_intros_are_truncated_with_an_ellipsis ... ok
test prd::status::tests::missing_directories_render_as_empty_sections_instead_of_failing ... ok
test prd::status::tests::a_stale_awaiting_banner_in_pending_does_not_hide_the_prd_from_pending ... ok
test prd::status::tests::deps_column_reports_a_blocked_upstream ... ok
test template::tests::copy_excludes_local_state_and_secrets_but_keeps_example_env_files ... ok
test prd::status::tests::board_shows_progress_banner_and_sections ... ok
test prd::status::tests::non_prd_files_in_the_task_directories_are_ignored ... ok
test prd::status::tests::summary_skips_heading_lines ... ok
test prd::status::tests::detail_mode_adds_title_and_a_bounded_summary ... ok
test prd::status::tests::lock_column_shows_the_holder_and_flags_stale_locks ... ok
test template::tests::copy_refuses_to_follow_a_symlink_inside_the_destination ... ok
test prd::status::tests::same_stem_in_pending_and_archive_prefers_pending ... ok
test prd::lock::tests::exactly_one_of_many_simultaneous_stale_takeovers_wins ... ok
test prd::status::tests::evidence_column_and_verdict_use_the_last_explicit_verdict ... ok
test prd::lock::tests::exactly_one_of_many_simultaneous_claimants_wins ... ok

test result: ok. 81 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.07s

     Running unittests src/main.rs (target/debug/deps/xtask-f7f623923874ef89)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/cli.rs (target/debug/deps/cli-ad6ede90519f3629)

running 10 tests
test max_lines_hook_fails_on_oversized_files_and_warns_when_asked ... ok
test review_prints_the_best_available_target_or_explains_what_is_missing ... ok
test starting_an_unknown_prd_fails_clearly ... ok
test a_prd_can_be_started_by_its_slug_and_progress_is_reported ... ok
test lock_conflict_hook_warns_but_never_blocks ... ok
test a_session_in_the_main_repo_hands_the_lock_to_the_worktree_that_continues_the_work ... ok
test check_passes_a_valid_prd_and_names_the_problems_of_an_invalid_one ... ok
test guard_test_hook_blocks_edits_to_marked_tests_unless_acknowledged ... ok
test archive_hook_gates_only_newly_archived_prds ... ok
test exactly_one_of_many_processes_in_different_worktrees_gets_the_lock ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.76s

   Doc-tests tpl_accounts

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests tpl_api

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests tpl_core

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests tpl_engines

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests tpl_infrastructure

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests xtask

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
    Checking tpl-infrastructure v0.1.0 (<repo>/crates/infrastructure)
 Documenting tpl-api v0.1.0 (<repo>/crates/api)
 Documenting tpl-infrastructure v0.1.0 (<repo>/crates/infrastructure)
 Documenting tpl-accounts v0.1.0 (<repo>/services/accounts)
    Checking tpl-accounts v0.1.0 (<repo>/services/accounts)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.55s
   Generated <repo>/target/doc/tpl_accounts/index.html and 7 other files

```
