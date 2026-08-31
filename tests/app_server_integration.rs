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

#[tokio::test]
async fn child_is_owned_immediately_and_every_exit_path_reaps() {
    cases::child_is_owned_immediately_and_every_exit_path_reaps().await;
}

#[tokio::test]
async fn child_timeout_kills_and_confirms_reap() {
    cases::child_timeout_kills_and_confirms_reap().await;
}

#[tokio::test]
async fn deadline_scopes_are_bounded_and_do_not_extend_each_other() {
    cases::deadline_scopes_are_bounded_and_do_not_extend_each_other().await;
}

#[test]
fn live_codes_are_the_only_output_and_cleanup_has_precedence() {
    cases::live_codes_are_the_only_output_and_cleanup_has_precedence();
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
