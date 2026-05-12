use pgforge::commands::schedule::{
    launchctl_is_already_gone, launchctl_is_soft_install_failure, render_plist,
};

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

// ---------------------------------------------------------------------------
// render_plist() XML-escape tests
// Real signature: render_plist(exe: &str, log_path: &str) -> String
// ---------------------------------------------------------------------------

#[test]
fn plist_xml_escapes_paths_with_ampersand() {
    let plist = render_plist("/Users/me/A&B/pgforge", "/tmp/log&log");
    assert!(
        plist.contains("/Users/me/A&amp;B/pgforge"),
        "ampersand in exe path must be XML-escaped"
    );
    assert!(
        !plist.contains("A&B/pgforge"),
        "raw & must not appear inside plist body"
    );
    assert!(
        plist.contains("log&amp;log"),
        "ampersand in log path must be XML-escaped"
    );
}

#[test]
fn plist_xml_escapes_angle_brackets_and_quotes() {
    let plist = render_plist("/tmp/<weird>'path", "/tmp/log");
    assert!(plist.contains("&lt;weird&gt;"));
    assert!(plist.contains("&apos;path") || plist.contains("&#39;path"));
}
