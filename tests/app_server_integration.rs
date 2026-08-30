use std::error::Error;

#[path = "app_server_integration/cases.rs"]
mod cases;
#[path = "app_server_integration/live_harness.rs"]
mod live_harness;

#[test]
fn ledger_persists_each_owned_id_with_file_and_directory_fsync() {
    cases::ledger_persists_each_owned_id_with_file_and_directory_fsync();
}

#[test]
fn ledger_persists_workspace_before_first_creation() {
    cases::ledger_persists_workspace_before_first_creation();
}

#[test]
fn live_gate_requires_exact_opt_in_before_mutation() {
    cases::live_gate_requires_exact_opt_in_before_mutation();
}

#[test]
fn recovery_requires_exact_opt_in_and_absolute_ledger() {
    cases::recovery_requires_exact_opt_in_and_absolute_ledger();
}

#[test]
fn cleanup_retains_ledger_until_archive_proof() {
    cases::cleanup_retains_ledger_until_archive_proof();
}

#[test]
fn exact_workspace_list_is_source_complete_and_provider_unfiltered() {
    cases::exact_workspace_list_is_source_complete_and_provider_unfiltered();
}

#[tokio::test]
async fn already_archived_exact_ledger_target_skips_archive_and_converges() {
    cases::already_archived_exact_ledger_target_skips_archive_and_converges().await;
}

#[tokio::test]
async fn active_exact_ledger_target_archives_once_then_converges() {
    cases::active_exact_ledger_target_archives_once_then_converges().await;
}

#[tokio::test]
async fn invalid_exact_read_evidence_fails_closed_and_retains_ledger() {
    cases::invalid_exact_read_evidence_fails_closed_and_retains_ledger().await;
}

#[test]
fn cleanup_failure_keeps_normal_tool_run_error() {
    cases::cleanup_failure_keeps_normal_tool_run_error();
}

#[test]
fn cleanup_failure_classifies_tool_run_panic_without_payload() {
    cases::cleanup_failure_classifies_tool_run_panic_without_payload();
}

#[test]
fn mcp_json_rpc_tool_error_preserves_allowlisted_context_without_sensitive_data() {
    live_harness::mcp_json_rpc_tool_error_preserves_allowlisted_context_without_sensitive_data();
}

#[test]
fn mcp_tool_result_error_preserves_allowlisted_context_and_fixed_fallbacks() {
    live_harness::mcp_tool_result_error_preserves_allowlisted_context_and_fixed_fallbacks();
}

#[test]
fn caller_bound_tool_request_keeps_metadata_outside_public_arguments() {
    live_harness::caller_bound_tool_request_keeps_metadata_outside_public_arguments();
}

#[tokio::test]
#[ignore = "requires explicit disposable-task opt-in and a live Desktop authority"]
async fn live_desktop_authority_all_thirteen_tools_are_disposable() -> Result<(), Box<dyn Error>> {
    cases::live_desktop_authority_all_thirteen_tools_are_disposable().await
}
