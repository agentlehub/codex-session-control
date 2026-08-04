use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::Error as _};
use serde_json::Value;

use crate::error::ControllerError;

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ThreadStatus {
    NotLoaded,
    Idle,
    SystemError,
    Active { active_flags: Vec<ActiveFlag> },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ActiveFlag {
    WaitingOnApproval,
    WaitingOnUserInput,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Thread {
    pub id: String,
    pub name: Option<String>,
    pub preview: String,
    pub cwd: String,
    pub status: ThreadStatus,
    pub created_at: i64,
    pub updated_at: i64,
    pub forked_from_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnStatus {
    Completed,
    Interrupted,
    Failed,
    InProgress,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnItemsView {
    NotLoaded,
    Summary,
    Full,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Turn {
    pub id: String,
    pub status: TurnStatus,
    #[schemars(with = "Vec<std::collections::BTreeMap<String, Value>>")]
    pub items: Vec<Value>,
    pub items_view: TurnItemsView,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub duration_ms: Option<i64>,
    #[schemars(with = "Option<std::collections::BTreeMap<String, Value>>")]
    pub error: Option<Value>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ThreadGoalStatus {
    Active,
    Paused,
    Blocked,
    UsageLimited,
    BudgetLimited,
    Complete,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadGoal {
    pub thread_id: String,
    pub objective: String,
    pub status: ThreadGoalStatus,
    pub token_budget: Option<u64>,
    pub tokens_used: u64,
    pub time_used_seconds: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadSnapshot {
    pub thread_id: String,
    pub name: Option<String>,
    pub status: ThreadStatus,
    pub active_turn_id: Option<String>,
    pub active_turn_status: Option<TurnStatus>,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProductConfig {
    pub schema_version: u32,
    pub codex_executable: PathBuf,
    pub codex_home: PathBuf,
    pub socket_path: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProductConfigWire {
    schema_version: u32,
    codex_executable: PathBuf,
    codex_home: PathBuf,
    socket_path: PathBuf,
}

impl<'de> Deserialize<'de> for ProductConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = ProductConfigWire::deserialize(deserializer)?;
        if wire.schema_version != CURRENT_SCHEMA_VERSION {
            return Err(D::Error::custom("unsupported schema version"));
        }
        Ok(Self {
            schema_version: wire.schema_version,
            codex_executable: wire.codex_executable,
            codex_home: wire.codex_home,
            socket_path: wire.socket_path,
        })
    }
}

impl ProductConfig {
    pub fn validate(
        &self,
        expected_codex_home: &Path,
        expected_socket_path: &Path,
    ) -> Result<(), ControllerError> {
        require_schema_version(self.schema_version)?;
        require_absolute("codex_executable", &self.codex_executable)?;
        require_absolute("codex_home", &self.codex_home)?;
        require_absolute("socket_path", &self.socket_path)?;
        require_identity("codex_home", &self.codex_home, expected_codex_home)?;
        require_identity("socket_path", &self.socket_path, expected_socket_path)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopAttachmentIdentity {
    pub launcher_path: PathBuf,
    pub app_id: String,
    pub descriptor_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstalledRelease {
    pub schema_version: u32,
    pub product_version: String,
    pub target: String,
    pub binary_sha256: String,
    pub service_unit_sha256: String,
    pub projection_sha256: String,
    pub plugin_version: String,
    pub codex_executable: PathBuf,
    pub codex_version: String,
    pub codex_home: PathBuf,
    pub socket_path: PathBuf,
    pub desktop_attachment: Option<DesktopAttachmentIdentity>,
}

struct InstalledReleaseWire {
    schema_version: u32,
    product_version: String,
    target: String,
    binary_sha256: String,
    service_unit_sha256: String,
    projection_sha256: String,
    plugin_version: String,
    codex_executable: PathBuf,
    codex_version: String,
    codex_home: PathBuf,
    socket_path: PathBuf,
    desktop_attachment: PresentNullable<DesktopAttachmentIdentity>,
}

struct PresentNullable<T>(Option<T>);

impl<'de, T> Deserialize<'de> for PresentNullable<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Option::<T>::deserialize(deserializer).map(Self)
    }
}

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "camelCase")]
enum InstalledReleaseField {
    SchemaVersion,
    ProductVersion,
    Target,
    BinarySha256,
    ServiceUnitSha256,
    ProjectionSha256,
    PluginVersion,
    CodexExecutable,
    CodexVersion,
    CodexHome,
    SocketPath,
    DesktopAttachment,
}

impl<'de> Deserialize<'de> for InstalledReleaseWire {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct WireVisitor;

        impl<'de> serde::de::Visitor<'de> for WireVisitor {
            type Value = InstalledReleaseWire;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an installed-release schema-2 object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut schema_version = None;
                let mut product_version = None;
                let mut target = None;
                let mut binary_sha256 = None;
                let mut service_unit_sha256 = None;
                let mut projection_sha256 = None;
                let mut plugin_version = None;
                let mut codex_executable = None;
                let mut codex_version = None;
                let mut codex_home = None;
                let mut socket_path = None;
                let mut desktop_attachment = None;

                while let Some(field) = map.next_key::<InstalledReleaseField>()? {
                    macro_rules! set_once {
                        ($slot:ident, $name:literal) => {{
                            if $slot.is_some() {
                                return Err(serde::de::Error::duplicate_field($name));
                            }
                            $slot = Some(map.next_value()?);
                        }};
                    }

                    match field {
                        InstalledReleaseField::SchemaVersion => {
                            set_once!(schema_version, "schemaVersion")
                        }
                        InstalledReleaseField::ProductVersion => {
                            set_once!(product_version, "productVersion")
                        }
                        InstalledReleaseField::Target => set_once!(target, "target"),
                        InstalledReleaseField::BinarySha256 => {
                            set_once!(binary_sha256, "binarySha256")
                        }
                        InstalledReleaseField::ServiceUnitSha256 => {
                            set_once!(service_unit_sha256, "serviceUnitSha256")
                        }
                        InstalledReleaseField::ProjectionSha256 => {
                            set_once!(projection_sha256, "projectionSha256")
                        }
                        InstalledReleaseField::PluginVersion => {
                            set_once!(plugin_version, "pluginVersion")
                        }
                        InstalledReleaseField::CodexExecutable => {
                            set_once!(codex_executable, "codexExecutable")
                        }
                        InstalledReleaseField::CodexVersion => {
                            set_once!(codex_version, "codexVersion")
                        }
                        InstalledReleaseField::CodexHome => set_once!(codex_home, "codexHome"),
                        InstalledReleaseField::SocketPath => set_once!(socket_path, "socketPath"),
                        InstalledReleaseField::DesktopAttachment => {
                            set_once!(desktop_attachment, "desktopAttachment")
                        }
                    }
                }

                Ok(InstalledReleaseWire {
                    schema_version: schema_version
                        .ok_or_else(|| serde::de::Error::missing_field("schemaVersion"))?,
                    product_version: product_version
                        .ok_or_else(|| serde::de::Error::missing_field("productVersion"))?,
                    target: target.ok_or_else(|| serde::de::Error::missing_field("target"))?,
                    binary_sha256: binary_sha256
                        .ok_or_else(|| serde::de::Error::missing_field("binarySha256"))?,
                    service_unit_sha256: service_unit_sha256
                        .ok_or_else(|| serde::de::Error::missing_field("serviceUnitSha256"))?,
                    projection_sha256: projection_sha256
                        .ok_or_else(|| serde::de::Error::missing_field("projectionSha256"))?,
                    plugin_version: plugin_version
                        .ok_or_else(|| serde::de::Error::missing_field("pluginVersion"))?,
                    codex_executable: codex_executable
                        .ok_or_else(|| serde::de::Error::missing_field("codexExecutable"))?,
                    codex_version: codex_version
                        .ok_or_else(|| serde::de::Error::missing_field("codexVersion"))?,
                    codex_home: codex_home
                        .ok_or_else(|| serde::de::Error::missing_field("codexHome"))?,
                    socket_path: socket_path
                        .ok_or_else(|| serde::de::Error::missing_field("socketPath"))?,
                    desktop_attachment: desktop_attachment
                        .ok_or_else(|| serde::de::Error::missing_field("desktopAttachment"))?,
                })
            }
        }

        const FIELDS: &[&str] = &[
            "schemaVersion",
            "productVersion",
            "target",
            "binarySha256",
            "serviceUnitSha256",
            "projectionSha256",
            "pluginVersion",
            "codexExecutable",
            "codexVersion",
            "codexHome",
            "socketPath",
            "desktopAttachment",
        ];
        deserializer.deserialize_struct("InstalledRelease", FIELDS, WireVisitor)
    }
}

impl<'de> Deserialize<'de> for InstalledRelease {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = InstalledReleaseWire::deserialize(deserializer)?;
        if wire.schema_version != CURRENT_SCHEMA_VERSION {
            return Err(D::Error::custom("unsupported schema version"));
        }
        Ok(Self {
            schema_version: wire.schema_version,
            product_version: wire.product_version,
            target: wire.target,
            binary_sha256: wire.binary_sha256,
            service_unit_sha256: wire.service_unit_sha256,
            projection_sha256: wire.projection_sha256,
            plugin_version: wire.plugin_version,
            codex_executable: wire.codex_executable,
            codex_version: wire.codex_version,
            codex_home: wire.codex_home,
            socket_path: wire.socket_path,
            desktop_attachment: wire.desktop_attachment.0,
        })
    }
}

impl InstalledRelease {
    pub fn validate(
        &self,
        expected_codex_home: &Path,
        expected_socket_path: &Path,
    ) -> Result<(), ControllerError> {
        require_schema_version(self.schema_version)?;
        require_release_version("product_version", &self.product_version)?;
        require_release_target(&self.target)?;
        require_release_version("plugin_version", &self.plugin_version)?;
        require_release_version("codex_version", &self.codex_version)?;
        require_sha256("binary_sha256", &self.binary_sha256)?;
        require_sha256("service_unit_sha256", &self.service_unit_sha256)?;
        require_sha256("projection_sha256", &self.projection_sha256)?;
        require_absolute("codex_executable", &self.codex_executable)?;
        require_absolute("codex_home", &self.codex_home)?;
        require_absolute("socket_path", &self.socket_path)?;
        require_identity("codex_home", &self.codex_home, expected_codex_home)?;
        require_identity("socket_path", &self.socket_path, expected_socket_path)?;
        if let Some(attachment) = &self.desktop_attachment {
            attachment.validate()?;
        }
        Ok(())
    }
}

const CURRENT_SCHEMA_VERSION: u32 = 2;

fn require_schema_version(schema_version: u32) -> Result<(), ControllerError> {
    if schema_version == CURRENT_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(ControllerError::InvalidData {
            field: "schema_version",
            reason: "unsupported value",
        })
    }
}

fn require_release_version(field: &'static str, value: &str) -> Result<(), ControllerError> {
    semver::Version::parse(value).map_err(|_| ControllerError::InvalidData {
        field,
        reason: "must be a semantic version",
    })?;
    Ok(())
}

fn require_release_target(value: &str) -> Result<(), ControllerError> {
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        return Err(ControllerError::InvalidData {
            field: "target",
            reason: "must be a non-whitespace target triple",
        });
    }
    Ok(())
}

impl DesktopAttachmentIdentity {
    fn validate(&self) -> Result<(), ControllerError> {
        require_absolute("desktop_attachment.launcher_path", &self.launcher_path)?;
        require_absolute("desktop_attachment.descriptor_path", &self.descriptor_path)?;
        if self.app_id.is_empty()
            || self.app_id == "."
            || self.app_id == ".."
            || self.app_id.contains(['/', '\\'])
            || self.app_id.chars().any(char::is_control)
        {
            return Err(ControllerError::InvalidData {
                field: "desktop_attachment.app_id",
                reason: "must be one safe path component",
            });
        }
        Ok(())
    }
}

fn require_absolute(field: &'static str, path: &Path) -> Result<(), ControllerError> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(ControllerError::InvalidData {
            field,
            reason: "path must be absolute",
        })
    }
}

fn require_identity(
    field: &'static str,
    actual: &Path,
    expected: &Path,
) -> Result<(), ControllerError> {
    if actual == expected {
        Ok(())
    } else {
        Err(ControllerError::InvalidData {
            field,
            reason: "path does not match canonical identity",
        })
    }
}

fn require_sha256(field: &'static str, value: &str) -> Result<(), ControllerError> {
    let is_lowercase_sha256 = value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if is_lowercase_sha256 {
        Ok(())
    } else {
        Err(ControllerError::InvalidData {
            field,
            reason: "expected lowercase SHA-256",
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use schemars::schema_for;
    use serde_json::{Value, json};

    use super::*;

    const GOAL_STATUSES: [(&str, ThreadGoalStatus); 6] = [
        ("active", ThreadGoalStatus::Active),
        ("paused", ThreadGoalStatus::Paused),
        ("blocked", ThreadGoalStatus::Blocked),
        ("usageLimited", ThreadGoalStatus::UsageLimited),
        ("budgetLimited", ThreadGoalStatus::BudgetLimited),
        ("complete", ThreadGoalStatus::Complete),
    ];

    #[test]
    fn stable_enums_use_exact_public_names() {
        for (expected, status) in GOAL_STATUSES {
            assert_eq!(serde_json::to_value(status).unwrap(), json!(expected));
        }

        for (status, expected) in [
            (ThreadStatus::NotLoaded, json!({"type": "notLoaded"})),
            (ThreadStatus::Idle, json!({"type": "idle"})),
            (ThreadStatus::SystemError, json!({"type": "systemError"})),
        ] {
            assert_eq!(serde_json::to_value(status).unwrap(), expected);
        }

        for (flag, expected) in [
            (ActiveFlag::WaitingOnApproval, "waitingOnApproval"),
            (ActiveFlag::WaitingOnUserInput, "waitingOnUserInput"),
        ] {
            assert_eq!(serde_json::to_value(flag).unwrap(), json!(expected));
        }

        for (status, expected) in [
            (TurnStatus::Completed, "completed"),
            (TurnStatus::Interrupted, "interrupted"),
            (TurnStatus::Failed, "failed"),
            (TurnStatus::InProgress, "inProgress"),
        ] {
            assert_eq!(serde_json::to_value(status).unwrap(), json!(expected));
        }

        for (view, expected) in [
            (TurnItemsView::NotLoaded, "notLoaded"),
            (TurnItemsView::Summary, "summary"),
            (TurnItemsView::Full, "full"),
        ] {
            assert_eq!(serde_json::to_value(view).unwrap(), json!(expected));
        }
    }

    #[test]
    fn active_status_and_schema_use_active_flags_in_camel_case() {
        let status = ThreadStatus::Active {
            active_flags: vec![ActiveFlag::WaitingOnApproval],
        };
        assert_eq!(
            serde_json::to_value(status).unwrap(),
            json!({"type": "active", "activeFlags": ["waitingOnApproval"]})
        );

        let schema = serde_json::to_value(schema_for!(ThreadStatus)).unwrap();
        let encoded = serde_json::to_string(&schema).unwrap();
        assert!(encoded.contains("\"activeFlags\""));
        assert!(!encoded.contains("active_flags"));
    }

    #[test]
    fn missing_optional_native_fields_normalize_to_json_null() {
        let thread = Thread {
            id: "thread-1".to_owned(),
            name: None,
            preview: String::new(),
            cwd: "/tmp".to_owned(),
            status: ThreadStatus::Idle,
            created_at: 1,
            updated_at: 2,
            forked_from_id: None,
        };
        assert_eq!(
            serde_json::to_value(thread).unwrap(),
            json!({
                "id": "thread-1",
                "name": null,
                "preview": "",
                "cwd": "/tmp",
                "status": {"type": "idle"},
                "createdAt": 1,
                "updatedAt": 2,
                "forkedFromId": null
            })
        );

        let turn = Turn {
            id: "turn-1".to_owned(),
            status: TurnStatus::Completed,
            items: Vec::<Value>::new(),
            items_view: TurnItemsView::NotLoaded,
            started_at: None,
            completed_at: None,
            duration_ms: None,
            error: None,
        };
        assert_eq!(
            serde_json::to_value(turn).unwrap(),
            json!({
                "id": "turn-1",
                "status": "completed",
                "items": [],
                "itemsView": "notLoaded",
                "startedAt": null,
                "completedAt": null,
                "durationMs": null,
                "error": null
            })
        );

        let goal = ThreadGoal {
            thread_id: "thread-1".to_owned(),
            objective: "ship".to_owned(),
            status: ThreadGoalStatus::Active,
            token_budget: None,
            tokens_used: 3,
            time_used_seconds: 4,
            created_at: 5,
            updated_at: 6,
        };
        assert_eq!(
            serde_json::to_value(goal).unwrap(),
            json!({
                "threadId": "thread-1",
                "objective": "ship",
                "status": "active",
                "tokenBudget": null,
                "tokensUsed": 3,
                "timeUsedSeconds": 4,
                "createdAt": 5,
                "updatedAt": 6
            })
        );

        let snapshot = ThreadSnapshot {
            thread_id: "thread-1".to_owned(),
            name: None,
            status: ThreadStatus::Idle,
            active_turn_id: None,
            active_turn_status: None,
            updated_at: 7,
        };
        assert_eq!(
            serde_json::to_value(snapshot).unwrap(),
            json!({
                "threadId": "thread-1",
                "name": null,
                "status": {"type": "idle"},
                "activeTurnId": null,
                "activeTurnStatus": null,
                "updatedAt": 7
            })
        );
    }

    #[test]
    fn configuration_is_strict_and_bound_to_canonical_identity() {
        let config: ProductConfig = toml::from_str(
            r#"
schema_version = 2
codex_executable = "/usr/bin/codex"
codex_home = "/home/test/.codex"
socket_path = "/run/user/1000/codex-session-control/app-server.sock"
"#,
        )
        .unwrap();
        config
            .validate(
                Path::new("/home/test/.codex"),
                Path::new("/run/user/1000/codex-session-control/app-server.sock"),
            )
            .unwrap();

        for invalid in [
            r#"
                schema_version = 1
codex_executable = "/usr/bin/codex"
codex_home = "/home/test/.codex"
socket_path = "/run/user/1000/codex-session-control/app-server.sock"
"#,
            r#"
                schema_version = 3
codex_executable = "/usr/bin/codex"
codex_home = "/home/test/.codex"
socket_path = "/run/user/1000/codex-session-control/app-server.sock"
"#,
            r#"
schema_version = 2
codex_executable = "relative/codex"
codex_home = "/home/test/.codex"
socket_path = "/run/user/1000/codex-session-control/app-server.sock"
"#,
        ] {
            if let Ok(invalid) = toml::from_str::<ProductConfig>(invalid) {
                assert!(
                    invalid
                        .validate(
                            Path::new("/home/test/.codex"),
                            Path::new("/run/user/1000/codex-session-control/app-server.sock"),
                        )
                        .is_err()
                );
            }
        }

        assert!(
            toml::from_str::<ProductConfig>(
                r#"
schema_version = 2
codex_executable = "/usr/bin/codex"
codex_home = "/home/test/.codex"
socket_path = "/run/user/1000/codex-session-control/app-server.sock"
unknown = true
"#
            )
            .is_err()
        );
    }

    #[test]
    fn installed_release_is_strict_and_validates_hashes_and_identity() {
        let valid = json!({
            "schemaVersion": 2,
            "productVersion": "0.1.0",
            "target": "x86_64-unknown-linux-gnu",
            "binarySha256": "a".repeat(64),
            "serviceUnitSha256": "b".repeat(64),
            "projectionSha256": "c".repeat(64),
            "pluginVersion": "0.1.0",
            "codexExecutable": "/usr/bin/codex",
            "codexVersion": "0.145.0",
            "codexHome": "/home/test/.codex",
            "socketPath": "/run/user/1000/codex-session-control/app-server.sock",
            "desktopAttachment": null
        });
        let release: InstalledRelease = serde_json::from_value(valid.clone()).unwrap();
        release
            .validate(
                Path::new("/home/test/.codex"),
                Path::new("/run/user/1000/codex-session-control/app-server.sock"),
            )
            .unwrap();

        for (field, value) in [
            ("schemaVersion", json!(1)),
            ("schemaVersion", json!(3)),
            ("binarySha256", json!("A".repeat(64))),
            ("serviceUnitSha256", json!("short")),
            ("codexExecutable", json!("relative/codex")),
            ("codexHome", json!("/unexpected/home")),
        ] {
            let mut invalid = valid.clone();
            invalid[field] = value;
            if let Ok(invalid) = serde_json::from_value::<InstalledRelease>(invalid) {
                assert!(
                    invalid
                        .validate(
                            Path::new("/home/test/.codex"),
                            Path::new("/run/user/1000/codex-session-control/app-server.sock"),
                        )
                        .is_err(),
                    "{field} should fail validation"
                );
            }
        }

        let mut unknown = valid;
        unknown["unknown"] = json!(true);
        assert!(serde_json::from_value::<InstalledRelease>(unknown).is_err());
    }

    #[test]
    fn installed_release_rejects_malformed_retained_release_identity() {
        let valid = json!({
            "schemaVersion": 2,
            "productVersion": "0.2.0",
            "target": "x86_64-unknown-linux-gnu",
            "binarySha256": "a".repeat(64),
            "serviceUnitSha256": "b".repeat(64),
            "projectionSha256": "c".repeat(64),
            "pluginVersion": "0.2.0",
            "codexExecutable": "/usr/bin/codex",
            "codexVersion": "0.145.0",
            "codexHome": "/home/test/.codex",
            "socketPath": "/run/user/1000/codex-session-control/app-server.sock",
            "desktopAttachment": null
        });

        for (field, value) in [
            ("productVersion", json!("")),
            ("productVersion", json!("not-a-version")),
            ("target", json!("")),
            ("target", json!("target with space")),
            ("pluginVersion", json!("")),
            ("pluginVersion", json!("not-a-version")),
            ("codexVersion", json!("")),
            ("codexVersion", json!("not-a-version")),
        ] {
            let mut invalid = valid.clone();
            invalid[field] = value;
            let parsed: InstalledRelease = serde_json::from_value(invalid).unwrap();
            assert!(
                parsed
                    .validate(
                        Path::new("/home/test/.codex"),
                        Path::new("/run/user/1000/codex-session-control/app-server.sock"),
                    )
                    .is_err(),
                "{field} must retain a valid release identity"
            );
        }
    }

    mod normal_home_schema {
        use super::*;

        const SELECTED_HOME: &str = "/home/test/.codex";
        const SOCKET_PATH: &str = "/run/user/1000/codex-session-control/app-server.sock";

        #[test]
        fn schema_two_configuration_is_exact_and_bound_to_the_selected_home() {
            let config: ProductConfig = toml::from_str(&format!(
                "schema_version = 2\ncodex_executable = \"/usr/bin/codex\"\n\
                 codex_home = \"{SELECTED_HOME}\"\nsocket_path = \"{SOCKET_PATH}\"\n"
            ))
            .unwrap();

            config
                .validate(Path::new(SELECTED_HOME), Path::new(SOCKET_PATH))
                .unwrap();
            assert_eq!(
                toml::to_string(&config).unwrap(),
                format!(
                    "schema_version = 2\ncodex_executable = \"/usr/bin/codex\"\n\
                     codex_home = \"{SELECTED_HOME}\"\nsocket_path = \"{SOCKET_PATH}\"\n"
                )
            );

            for invalid in [
                format!(
                    "schema_version = 2\ncodex_executable = \"/usr/bin/codex\"\n\
                     codex_home = \"{SELECTED_HOME}\"\nsocket_path = \"{SOCKET_PATH}\"\nunknown = true\n"
                ),
                format!(
                    "schema_version = 3\ncodex_executable = \"/usr/bin/codex\"\n\
                     codex_home = \"{SELECTED_HOME}\"\nsocket_path = \"{SOCKET_PATH}\"\n"
                ),
                format!(
                    "schema_version = 2\ncodex_executable = \"relative/codex\"\n\
                     codex_home = \"{SELECTED_HOME}\"\nsocket_path = \"{SOCKET_PATH}\"\n"
                ),
            ] {
                if let Ok(parsed) = toml::from_str::<ProductConfig>(&invalid) {
                    assert!(
                        parsed
                            .validate(Path::new(SELECTED_HOME), Path::new(SOCKET_PATH))
                            .is_err(),
                        "{invalid}"
                    );
                }
            }
        }

        #[test]
        fn schema_two_manifest_requires_explicit_nullable_desktop_attachment() {
            let manifest = json!({
                "schemaVersion": 2,
                "productVersion": "0.2.0",
                "target": "x86_64-unknown-linux-gnu",
                "binarySha256": "a".repeat(64),
                "serviceUnitSha256": "b".repeat(64),
                "projectionSha256": "c".repeat(64),
                "pluginVersion": "0.2.0",
                "codexExecutable": "/usr/bin/codex",
                "codexVersion": "0.145.0",
                "codexHome": SELECTED_HOME,
                "socketPath": SOCKET_PATH,
                "desktopAttachment": null
            });

            let parsed: InstalledRelease = serde_json::from_value(manifest.clone()).unwrap();
            parsed
                .validate(Path::new(SELECTED_HOME), Path::new(SOCKET_PATH))
                .unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap(), manifest);

            let mut missing = manifest;
            missing.as_object_mut().unwrap().remove("desktopAttachment");
            assert!(serde_json::from_value::<InstalledRelease>(missing).is_err());
        }

        #[test]
        fn desktop_attachment_is_strict_and_path_safe() {
            let valid = json!({
                "schemaVersion": 2,
                "productVersion": "0.2.0",
                "target": "x86_64-unknown-linux-gnu",
                "binarySha256": "a".repeat(64),
                "serviceUnitSha256": "b".repeat(64),
                "projectionSha256": "c".repeat(64),
                "pluginVersion": "0.2.0",
                "codexExecutable": "/usr/bin/codex",
                "codexVersion": "0.145.0",
                "codexHome": SELECTED_HOME,
                "socketPath": SOCKET_PATH,
                "desktopAttachment": {
                    "launcherPath": "/opt/codex-desktop",
                    "appId": "codex-desktop",
                    "descriptorPath": "/home/test/.config/codex-desktop/app-server-attachment.json"
                }
            });
            let parsed: InstalledRelease = serde_json::from_value(valid.clone()).unwrap();
            parsed
                .validate(Path::new(SELECTED_HOME), Path::new(SOCKET_PATH))
                .unwrap();

            for (field, value) in [
                ("launcherPath", json!("relative/launcher")),
                ("appId", json!("../other-app")),
                ("descriptorPath", json!("relative/descriptor.json")),
            ] {
                let mut invalid = valid.clone();
                invalid["desktopAttachment"][field] = value;
                let parsed: InstalledRelease = serde_json::from_value(invalid).unwrap();
                assert!(
                    parsed
                        .validate(Path::new(SELECTED_HOME), Path::new(SOCKET_PATH))
                        .is_err(),
                    "{field} must be rejected"
                );
            }

            let mut unknown = valid;
            unknown["desktopAttachment"]["unknown"] = json!(true);
            assert!(serde_json::from_value::<InstalledRelease>(unknown).is_err());
        }
    }
}
