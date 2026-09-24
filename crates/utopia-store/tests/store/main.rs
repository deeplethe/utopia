//! store 的集成测试合成**一个**测试二进制，而不是每个文件一个。
//!
//! 从前 `tests/` 下 120 个文件就是 120 个二进制：每个都要单独编译、单独把整个依赖图
//! 链接一遍（CI 上占 test job 编译时间的大头），跑起来又是一个接一个串行执行，
//! 多数二进制只有一两个测试，机器大部分时间在等下一个进程起来。合成一个之后
//! 编一次、链一次，测试之间照常并行：本地 274 个测试从 128 秒到 13 秒。
//!
//! 代价是这里的测试**真的会同时跑**，而从前不同文件的测试从不同时运行。绝大多数
//! 测试自建自拆一个 kb，天然互不相干。碰**kb 之外**的状态的测试留在 `tests/` 顶层、
//! 各自仍是独立二进制（cargo 按顺序跑二进制，它们跑的时候没有别人）：
//!
//! - `a_queued_job_wakes_an_idle_worker`：守 `utopia_jobs` 通知频道，别的测试一入队就会把它叫醒。
//! - `a_vector_index_is_built_by_a_job`、`a_vector_build_uses_one_connection`：在 `chunks` 上
//!   `CREATE INDEX CONCURRENTLY`。两条 CONCURRENTLY 同时建必然互相等死（Postgres 的规矩，
//!   `vector_index::build` 的注释写了为什么设计上就是串行的），前者还数全库的构建任务。
//! - `human_phrase_materialization_delivery`：起真实的 `run_worker`，会认领库里**任何**排队
//!   任务，遇到不是自己那种 kind 的就报错标失败。
//! - `the_engine_redraws_only_what_it_drew`：把迁移 0057 的回填语句原样跑一遍，那条
//!   `UPDATE facts` 不按 kb 过滤，锁住全库的行，和正在重算时间线的测试互相等死——
//!   合并二进制的实验里 30% 的运行栽在这里。
//!
//! 新加测试默认放这个目录；只有碰上面那类全局状态时才放顶层，并在这里补一行为什么。
mod a_batch_decides_like_a_person;
mod a_batch_gathers_its_neighbours_in_order;
mod a_bound_statement_becomes_a_typed_fact;
mod a_chunk_says_where_its_words_came_from;
mod a_clash_needs_both_at_once;
mod a_contradiction_points_upstream;
mod a_cycle_holds_at_one_moment;
mod a_cycle_is_keyed_by_all_its_facts;
mod a_cycle_search_that_stops_says_so;
mod a_decision_records_why;
mod a_declaration_arrives_late;
mod a_declared_disjointness_keeps_names_apart;
mod a_deferred_job_does_not_spend_its_budget;
mod a_definition_can_be_written_by_hand;
mod a_deletion_is_an_event;
mod a_derivation_follows_the_second_clock;
mod a_described_thing_is_an_entity_without_a_name;
mod a_direction_is_judged_by_range_too;
mod a_dirty_ledger_stops_the_migration;
mod a_disambiguator_follows_the_ontology;
mod a_document_opening_is_its_first_live_chunk;
mod a_fact_awaits_a_nod;
mod a_failed_job_finds_its_way_back;
mod a_forward_reference_is_judged_at_commit;
mod a_governor_reads_the_ledger;
mod a_judged_entity_waits_its_turn;
mod a_kind_word_binds_to_a_class;
mod a_late_value_takes_its_place_in_history;
mod a_mapping_is_not_a_fact;
mod a_merge_rewinds_with_the_second_clock;
mod a_merged_target_stays_out_of_the_export;
mod a_name_created_twice_at_once_is_one_entity;
mod a_name_is_a_fact;
mod a_namesake_tie_goes_to_review_not_a_coin_flip;
mod a_number_is_one_number_however_written;
mod a_page_never_skips_a_row;
mod a_path_joins_two_entities;
mod a_pending_statement_keeps_the_documents_words;
mod a_plan_step_follows_its_premise;
mod a_proof_reaches_the_sentence;
mod a_purge_is_final;
mod a_purge_judges_its_blobs_once;
mod a_qualifier_is_not_the_edges_identity;
mod a_question_picks_its_definitions;
mod a_relation_points_only_inside_its_own_kb;
mod a_remembered_episode_strips_nul;
mod a_retired_account;
mod a_retraction_leaves_the_graph;
mod a_review_has_a_summary;
mod a_rule_computes_what_it_concludes;
mod a_rule_concludes_a_type;
mod a_rule_reads_what_a_rule_concluded;
mod a_schema_document_is_searched_not_extracted;
mod a_search_reads_the_base_as_it_was;
mod a_secret_is_sealed_at_rest;
mod a_signature_holds_on_every_path;
mod a_source_kind_is_listed_once;
mod a_source_reaches_only_where_it_was_granted;
mod a_time_mention_is_resolved_against_its_document;
mod a_time_mention_is_words_not_a_date;
mod a_timeline_holds_whatever_the_order;
mod a_timeline_is_recomputed_from_the_rows_it_has;
mod a_token_is_the_person_but_not_all_of_them;
mod a_trimmed_description_is_not_stale;
mod a_viewer_never_sees_a_credential;
mod a_wrong_time_can_be_corrected;
mod adopt_swap;
mod adopting_an_iri_adopts_the_shape;
mod an_agent_can_record;
mod an_amount_outlives_adoption;
mod an_automatic_merge_is_gated_by_what_it_can_undo;
mod an_end_date_closes_the_open_span;
mod an_event_holds_at_the_moment_it_names;
mod an_exploration_says_what_it_covered;
mod an_export_carries_the_whole_ledger;
mod an_open_statement_keeps_the_documents_words;
mod an_undeclared_name_is_looked_up;
mod an_unknown_date_is_not_an_open_one;
mod an_untyped_name_meets_its_namesake;
mod an_untyped_subject_does_not_stall_the_batch;
mod an_upload_has_no_date;
mod axioms_judge_the_ledger;
mod axioms_reach_the_database;
mod bench_100k;
mod blocked_for_entity_respects_as_of;
mod closing_an_unconfirmed_fact_clears_the_queue;
mod concurrent_chunk_replacement;
mod cross_kb_provenance_fails_closed;
mod derived_facts_are_second_class;
mod ended_when_unknown;
mod exploration_describes_the_data;
mod export_surfaces;
mod exported_references_never_cross_a_kb;
mod extraction_progress_counts_a_document_once;
mod facts_corroborate_an_identity;
mod graph_changes;
mod history_shows_the_merge_itself;
mod human_type_decisions;
mod materialization_is_serial;
mod migration_0070_runs_under_any_search_path;
mod miss_dismissal;
mod negative_binding_definition_edit;
mod no_predicate_still_shows;
mod phrase_signature_evidence;
mod proposal_counts;
mod review_stages;
mod rss_full_content;
mod rss_ledger_contract;
mod same_name_peers_respects_as_of;
mod search_entities_degree_respects_as_of;
mod the_backstop_can_be_raised;
mod the_floor_under_retrieval;
mod the_nearest_chunk_is_found_however_it_is_reached;
mod the_second_clock_can_be_rewound;
mod the_world_axis_reaches_the_second;
