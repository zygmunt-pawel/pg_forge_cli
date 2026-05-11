use pgforge::domain::platform::{Platform, current_platform};

#[test]
fn current_platform_matches_compile_target() {
    let p = current_platform();
    if cfg!(target_os = "macos") {
        assert_eq!(p, Platform::MacOs);
    } else if cfg!(target_os = "linux") {
        assert_eq!(p, Platform::Linux);
    } else {
        // unsupported targets fall back to Linux for now
        assert_eq!(p, Platform::Linux);
    }
}

#[test]
fn platform_short_name_is_stable() {
    assert_eq!(Platform::MacOs.short_name(), "macos");
    assert_eq!(Platform::Linux.short_name(), "linux");
}
