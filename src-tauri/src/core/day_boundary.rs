use chrono::{DateTime, Days, Local, TimeZone};

pub fn local_day_bounds(now: DateTime<Local>) -> (DateTime<Local>, DateTime<Local>) {
    let date = now.date_naive();
    let start = Local
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).expect("midnight"))
        .single()
        .expect("local midnight");
    let next_date = date.checked_add_days(Days::new(1)).expect("next day");
    let next = Local
        .from_local_datetime(&next_date.and_hms_opt(0, 0, 0).expect("next midnight"))
        .single()
        .expect("next local midnight");
    (start, next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, TimeZone};

    #[test]
    fn returns_current_local_day_bounds() {
        let now = Local.with_ymd_and_hms(2026, 6, 20, 13, 45, 10).unwrap();

        let (start, next) = local_day_bounds(now);

        assert_eq!(
            start.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2026-06-20 00:00:00"
        );
        assert_eq!(
            next.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2026-06-21 00:00:00"
        );
    }
}
