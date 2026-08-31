use std::error::Error;

#[path = "app_server_integration/cases.rs"]
mod cases;
#[path = "../src/app_server/endpoint_policy.rs"]
mod endpoint_policy;
#[path = "app_server_integration/live_harness.rs"]
mod live_harness;

#[test]
fn journal_grants_authority_only_after_durable_replace() {
    cases::journal_grants_authority_only_after_durable_replace();
}

#[test]
fn journal_rejects_unsafe_or_mismatched_authority() {
    cases::journal_rejects_unsafe_or_mismatched_authority();
}

#[test]
fn live_mode_matrix_is_total_and_recovery_is_fixed_authority() {
    cases::live_mode_matrix_is_total_and_recovery_is_fixed_authority();
}

#[test]
fn workspace_recovery_validates_all_pages_before_one_journal_write() {
    cases::workspace_recovery_validates_all_pages_before_one_journal_write();
}

#[test]
fn workspace_pagination_rejects_cycles_and_exhaustion() {
    cases::workspace_pagination_rejects_cycles_and_exhaustion();
}

#[test]
fn archive_classifier_accepts_only_exact_identity_and_storage() {
    cases::archive_classifier_accepts_only_exact_identity_and_storage();
}

#[tokio::test]
async fn archive_reconciliation_dispatches_at_most_once_after_exact_active_read() {
    cases::archive_reconciliation_dispatches_at_most_once_after_exact_active_read().await;
}

#[tokio::test]
async fn direct_cleanup_requires_safe_endpoint_and_exact_initialized_identity() {
    cases::direct_cleanup_requires_safe_endpoint_and_exact_initialized_identity().await;
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
