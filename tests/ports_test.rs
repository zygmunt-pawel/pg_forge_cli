use pgforge::error::PgForgeError;
use pgforge::ports::{IsBindable, allocate_port};
use std::collections::HashSet;

struct AllFree;
impl IsBindable for AllFree {
    fn is_bindable(&self, _port: u16) -> bool { true }
}

struct NoneFree;
impl IsBindable for NoneFree {
    fn is_bindable(&self, _port: u16) -> bool { false }
}

struct OnlyOddFree;
impl IsBindable for OnlyOddFree {
    fn is_bindable(&self, port: u16) -> bool { port % 2 == 1 }
}

#[test]
fn allocates_first_port_in_range_when_all_free() {
    let p = allocate_port(5433, 5500, &HashSet::new(), &AllFree).unwrap();
    assert_eq!(p, 5433);
}

#[test]
fn skips_taken_ports() {
    let taken: HashSet<u16> = [5433, 5434].iter().copied().collect();
    let p = allocate_port(5433, 5500, &taken, &AllFree).unwrap();
    assert_eq!(p, 5435);
}

#[test]
fn skips_unbindable_ports() {
    let p = allocate_port(5433, 5500, &HashSet::new(), &OnlyOddFree).unwrap();
    assert_eq!(p, 5433);
    let p = allocate_port(5434, 5500, &HashSet::new(), &OnlyOddFree).unwrap();
    assert_eq!(p, 5435);
}

#[test]
fn errors_when_no_port_available() {
    let err = allocate_port(5433, 5435, &HashSet::new(), &NoneFree).unwrap_err();
    matches!(err, PgForgeError::NoFreePort { start: 5433, end: 5435 });
}
