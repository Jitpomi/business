//! UTC clock values for native TypeQL inputs and provider Unix timestamps.
//! Authorization remains in TypeDB; this module only formats trusted time.
use time::OffsetDateTime;

pub fn unix_seconds() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

pub fn native_utc() -> String {
    native_literal(OffsetDateTime::now_utc())
}

fn native_literal(at: OffsetDateTime) -> String {
    let at = at.to_offset(time::UtcOffset::UTC);
    format!(
        "{}T{:02}:{:02}:{:02}.{:06}",
        at.date(),
        at.hour(),
        at.minute(),
        at.second(),
        at.microsecond()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_clock_keeps_utc_and_truncates_to_existing_microsecond_precision() {
        let at = OffsetDateTime::from_unix_timestamp_nanos(1_700_000_000_123_456_789).unwrap();
        assert_eq!(native_literal(at), "2023-11-14T22:13:20.123456");
        assert_eq!(
            native_literal(at.to_offset(time::UtcOffset::from_hms(5, 30, 0).unwrap())),
            "2023-11-14T22:13:20.123456"
        );
        assert_eq!(
            native_literal(OffsetDateTime::from_unix_timestamp(-1).unwrap()),
            "1969-12-31T23:59:59.000000"
        );
    }
}
