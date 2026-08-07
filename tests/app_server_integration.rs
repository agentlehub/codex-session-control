use std::error::Error;

#[path = "app_server_integration/cases.rs"]
mod cases;
#[path = "app_server_integration/live_harness.rs"]
mod live_harness;
#[path = "app_server_integration/normal_home.rs"]
mod normal_home;
#[path = "app_server_integration/normal_home_paths.rs"]
mod normal_home_paths;
#[path = "app_server_integration/protocol_support.rs"]
mod protocol_support;
#[path = "support/private_tempdir.rs"]
mod test_support;

#[tokio::test]
#[ignore = "requires the configured supported Codex CLI version"]
async fn live_schema_digest_matches_committed_fixture() -> Result<(), Box<dyn Error>> {
    cases::live_schema_digest_matches_committed_fixture().await
}

#[tokio::test]
#[ignore = "requires the configured supported Codex CLI version"]
async fn live_read_list_fork_title_goal_interrupt_mappings() -> Result<(), Box<dyn Error>> {
    cases::live_read_list_fork_title_goal_interrupt_mappings().await
}

#[tokio::test]
#[ignore = "requires the configured supported Codex CLI version"]
async fn live_remote_cli_attaches_and_reconnects() -> Result<(), Box<dyn Error>> {
    cases::live_remote_cli_attaches_and_reconnects().await
}

#[tokio::test]
#[ignore = "requires the configured supported Codex CLI version"]
async fn live_restart_preserves_shared_home_sessions() -> Result<(), Box<dyn Error>> {
    cases::live_restart_preserves_shared_home_sessions().await
}

#[tokio::test]
#[ignore = "requires the configured supported Codex CLI version"]
async fn live_socket_removed_when_app_server_exits() -> Result<(), Box<dyn Error>> {
    cases::live_socket_removed_when_app_server_exits().await
}

#[tokio::test]
#[ignore = "requires the configured supported Codex CLI version"]
async fn live_projection_converges_on_new_task_without_restart() -> Result<(), Box<dyn Error>> {
    cases::live_projection_converges_on_new_task_without_restart().await
}

#[tokio::test]
#[ignore = "CI-owned: requires the explicitly opted-in disposable passwd user"]
async fn live_remote_cli_executes_projected_goal_tool_round_trip() -> Result<(), Box<dyn Error>> {
    cases::live_remote_cli_executes_projected_goal_tool_round_trip().await
}

#[test]
fn disposable_normal_home_contract() {
    cases::disposable_normal_home_contract();
}

#[tokio::test]
async fn goal_output_barrier_ignores_interleaved_responses_requests() -> Result<(), Box<dyn Error>>
{
    cases::goal_output_barrier_ignores_interleaved_responses_requests().await
}

#[test]
fn shutdown_receipts_require_the_operation_specific_stage() {
    cases::shutdown_receipts_require_the_operation_specific_stage();
}

#[test]
fn normal_home_ci_requires_each_exact_opt_in_before_mutation() {
    cases::normal_home_ci_requires_each_exact_opt_in_before_mutation();
}

#[test]
fn cleanup_combination_keeps_absence_verification_authoritative() {
    cases::cleanup_combination_keeps_absence_verification_authoritative();
}

#[tokio::test]
#[ignore = "CI-owned: requires the explicitly opted-in disposable passwd user"]
async fn live_normal_home_shared_authority() -> Result<(), Box<dyn Error>> {
    cases::live_normal_home_shared_authority().await
}

#[tokio::test]
#[ignore = "CI-owned: requires the explicitly opted-in disposable passwd user"]
async fn live_normal_home_restart_boundaries() -> Result<(), Box<dyn Error>> {
    cases::live_normal_home_restart_boundaries().await
}

#[tokio::test]
#[ignore = "CI-owned: requires the explicitly opted-in disposable passwd user"]
async fn live_normal_home_projection_preservation() -> Result<(), Box<dyn Error>> {
    cases::live_normal_home_projection_preservation().await
}

#[tokio::test]
#[ignore = "CI-owned: requires the explicitly opted-in disposable passwd user"]
async fn live_normal_home_uninstall_preservation() -> Result<(), Box<dyn Error>> {
    cases::live_normal_home_uninstall_preservation().await
}
