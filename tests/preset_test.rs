use pgforge::domain::preset::{Preset, Tuning};

#[test]
fn tiny_preset_tuning() {
    let t = Preset::Tiny.tuning();
    assert_eq!(t.ram_mb, 1024);
    assert_eq!(t.max_connections, 50);
    assert_eq!(t.shared_buffers_mb, 256);
    assert_eq!(t.effective_cache_size_mb, 768);
    assert_eq!(t.work_mem_mb, 5);
    assert_eq!(t.max_wal_size_mb, 1024);
}

#[test]
fn medium_preset_tuning() {
    let t = Preset::Medium.tuning();
    assert_eq!(t.ram_mb, 4096);
    assert_eq!(t.max_connections, 200);
    assert_eq!(t.shared_buffers_mb, 1024);
    assert_eq!(t.effective_cache_size_mb, 3072);
}

#[test]
fn preset_parses_from_lowercase_str() {
    use std::str::FromStr;
    assert_eq!(Preset::from_str("tiny").unwrap(), Preset::Tiny);
    assert_eq!(Preset::from_str("small").unwrap(), Preset::Small);
    assert_eq!(Preset::from_str("medium").unwrap(), Preset::Medium);
    assert_eq!(Preset::from_str("large").unwrap(), Preset::Large);
    assert!(Preset::from_str("huge").is_err());
}

#[test]
fn tuning_struct_is_serializable() {
    let t = Preset::Small.tuning();
    let s = toml::to_string(&t).unwrap();
    let parsed: Tuning = toml::from_str(&s).unwrap();
    assert_eq!(t, parsed);
}
