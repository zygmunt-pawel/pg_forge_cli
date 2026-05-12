use pgforge::commands::schedule::{launchctl_is_already_gone, launchctl_is_soft_install_failure};

// ---------------------------------------------------------------------------
// install() soft-failure recognition
// ---------------------------------------------------------------------------

#[test]
fn install_soft_failure_on_headless_domain() {
    // Real macOS stderr samples that launchctl bootstrap emits on a headless
    // box where gui/<uid> is unreachable.
    assert!(launchctl_is_soft_install_failure(
        "Bootstrap failed: 5: Input/output error"
    ));
    assert!(launchctl_is_soft_install_failure(
        "Could not find domain for"
    ));
    assert!(launchctl_is_soft_install_failure(
        "Bootstrap failed: 112"
    ));
    assert!(launchctl_is_soft_install_failure(
        "Bootstrap failed: 113"
    ));
    assert!(launchctl_is_soft_install_failure(
        "domain target gui/501 not available"
    ));
    // Case-insensitive tolerance
    assert!(launchctl_is_soft_install_failure(
        "INPUT/OUTPUT ERROR on domain"
    ));
    assert!(launchctl_is_soft_install_failure(
        "BOOTSTRAP FAILED: 5"
    ));
}

#[test]
fn install_hard_failure_propagates() {
    // These should NOT be swallowed — they indicate real misconfiguration.
    assert!(!launchctl_is_soft_install_failure(
        "Load failed: cant parse plist"
    ));
    assert!(!launchctl_is_soft_install_failure(""));
    assert!(!launchctl_is_soft_install_failure(
        "launchctl: no such file or directory"
    ));
}

// ---------------------------------------------------------------------------
// uninstall() already-gone recognition
// ---------------------------------------------------------------------------

#[test]
fn uninstall_treats_missing_service_as_already_gone() {
    assert!(launchctl_is_already_gone(
        "Could not find specified service"
    ));
    assert!(launchctl_is_already_gone("No such process"));
    assert!(launchctl_is_already_gone(
        "Service is disabled and cannot be loaded"
    ));
    assert!(launchctl_is_already_gone("ESRCH"));
    // Case-insensitive
    assert!(launchctl_is_already_gone(
        "could not find specified service"
    ));
    assert!(launchctl_is_already_gone("no such process"));
}

#[test]
fn uninstall_propagates_real_errors() {
    assert!(!launchctl_is_already_gone("Bootstrap failed: 5"));
    assert!(!launchctl_is_already_gone(""));
    assert!(!launchctl_is_already_gone(
        "Load failed: cant parse plist"
    ));
}
