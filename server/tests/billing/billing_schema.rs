use super::*;
#[test]
fn months_keep_anchor_and_once_ends() {
    let a = DateTime::parse_from_rfc3339("2027-01-31T15:00:00Z")
        .unwrap()
        .timestamp();
    assert_eq!(
        DateTime::from_timestamp(occurrence(a, Interval::Month, 1).unwrap().unwrap(), 0)
            .unwrap()
            .to_rfc3339(),
        "2027-02-28T15:00:00+00:00"
    );
    assert_eq!(
        DateTime::from_timestamp(occurrence(a, Interval::Month, 2).unwrap().unwrap(), 0)
            .unwrap()
            .to_rfc3339(),
        "2027-03-31T15:00:00+00:00"
    );
    assert_eq!(occurrence(a, Interval::Once, 1).unwrap(), None);
}
