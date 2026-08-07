use std::fs;

use super::support::Fixture;
use super::*;

#[test]
fn enablement_and_activity_accept_only_exact_systemctl_evidence() {
    for (code, stdout, expected) in [
        (Some(0), b"enabled\n".as_slice(), ServiceEnablement::Enabled),
        (
            Some(1),
            b"disabled\n".as_slice(),
            ServiceEnablement::Disabled,
        ),
        (
            Some(4),
            b"not-found\n".as_slice(),
            ServiceEnablement::Absent,
        ),
        (Some(0), b"enabled".as_slice(), ServiceEnablement::Unproven),
        (
            Some(0),
            b"enabled\nextra\n".as_slice(),
            ServiceEnablement::Unproven,
        ),
        (
            Some(3),
            b"disabled\n".as_slice(),
            ServiceEnablement::Unproven,
        ),
        (None, b"enabled\n".as_slice(), ServiceEnablement::Unproven),
    ] {
        assert_eq!(classify_service_enablement(code, stdout), expected);
    }
    for (code, stdout, expected) in [
        (Some(0), b"active\n".as_slice(), ServiceActivity::Active),
        (Some(3), b"inactive\n".as_slice(), ServiceActivity::Inactive),
        (
            Some(3),
            b"activating\n".as_slice(),
            ServiceActivity::Unproven,
        ),
        (
            Some(3),
            b"deactivating\n".as_slice(),
            ServiceActivity::Unproven,
        ),
        (Some(0), b"active".as_slice(), ServiceActivity::Unproven),
        (None, b"inactive\n".as_slice(), ServiceActivity::Unproven),
    ] {
        assert_eq!(classify_service_activity(code, stdout), expected);
    }
}

#[test]
fn whoami_is_the_only_independence_authority() {
    let fixture = Fixture::new();
    let mut target = LifecycleTarget::suffixed(fixture.paths.clone(), "Setup1");

    fs::write(&fixture.whoami_unit, format!("{}\n", target.unit_name)).unwrap();
    assert_eq!(
        inspect_caller_unit(&fixture.fake_bin.join("systemctl"), &target),
        CallerUnitInspection::SelfHosted(CallerUnitEvidence::WhoAmI)
    );

    fs::write(&fixture.whoami_unit, b"session-42.scope\n").unwrap();
    assert_eq!(
        inspect_caller_unit(&fixture.fake_bin.join("systemctl"), &target),
        CallerUnitInspection::Independent
    );

    fs::write(&fixture.systemctl_fail, b"--user whoami").unwrap();
    fs::write(
        &fixture.control_group,
        format!("/user.slice/app.slice/{}\n", target.unit_name),
    )
    .unwrap();
    let caller_cgroup = format!(
        "0::/user.slice/app.slice/{}/child.scope\n",
        target.unit_name
    );
    target = target.with_caller_cgroup_snapshot(caller_cgroup.into_bytes());
    assert_eq!(
        inspect_caller_unit(&fixture.fake_bin.join("systemctl"), &target),
        CallerUnitInspection::SelfHosted(CallerUnitEvidence::ControlGroup)
    );

    target = target.with_caller_cgroup_snapshot(b"0::/user.slice/session-42.scope\n".to_vec());
    assert!(matches!(
        inspect_caller_unit(&fixture.fake_bin.join("systemctl"), &target),
        CallerUnitInspection::Unknown { .. }
    ));
}

#[test]
fn cgroup_fallback_only_proves_exact_or_descendant_self_hosting() {
    for (control_group, proc_self_cgroup, expected) in [
        (
            b"/user.slice/app.slice/csc.service\n".as_slice(),
            b"0::/user.slice/app.slice/csc.service\n".as_slice(),
            true,
        ),
        (
            b"/user.slice/app.slice/csc.service\n".as_slice(),
            b"0::/user.slice/app.slice/csc.service/child.scope\n".as_slice(),
            true,
        ),
        (
            b"/user.slice/app.slice/csc.service\n".as_slice(),
            b"0::/user.slice/app.slice/csc.service-sibling.scope\n".as_slice(),
            false,
        ),
        (
            b"/user.slice/app.slice/csc.service\n".as_slice(),
            b"0::/user.slice/session-42.scope\n".as_slice(),
            false,
        ),
        (
            b"\n".as_slice(),
            b"0::/user.slice/app.slice/csc.service\n".as_slice(),
            false,
        ),
        (
            b"/\n".as_slice(),
            b"0::/user.slice/app.slice/csc.service\n".as_slice(),
            false,
        ),
        (
            b"user.slice/app.slice/csc.service\n".as_slice(),
            b"0::/user.slice/app.slice/csc.service\n".as_slice(),
            false,
        ),
        (
            b"/user.slice/app.slice/../csc.service\n".as_slice(),
            b"0::/user.slice/app.slice/csc.service\n".as_slice(),
            false,
        ),
        (
            b"/user.slice/app.slice/csc.service\n".as_slice(),
            b"0::/user.slice/app.slice/csc.service\n0::/user.slice/other.scope\n".as_slice(),
            false,
        ),
        (
            b"/user.slice/app.slice/csc\xff.service\n".as_slice(),
            b"0::/user.slice/app.slice/csc.service\n".as_slice(),
            false,
        ),
        (
            b"/user.slice/app.slice/csc.service\n".as_slice(),
            b"0::/user.slice/app.slice/csc.service".as_slice(),
            false,
        ),
    ] {
        assert_eq!(
            cgroup_proves_self_hosted(control_group, proc_self_cgroup),
            expected,
            "control_group={control_group:?}, proc_self_cgroup={proc_self_cgroup:?}"
        );
    }
}
