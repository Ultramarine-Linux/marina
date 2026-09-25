use chrono::Timelike;

pub fn format_clock(hours: u32, minutes: u32, twelve_hour: bool) -> String {
    if !twelve_hour {
        return format!("{hours:02}:{minutes:02}");
    }
    let period = if hours < 12 { "AM" } else { "PM" };
    let hour = match hours % 12 {
        0 => 12,
        hour => hour,
    };
    format!("{hour}:{minutes:02} {period}")
}

pub fn current_time_string(twelve_hour: bool) -> String {
    let now = chrono::Local::now();
    format_clock(now.hour(), now.minute(), twelve_hour)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_24_hour_time() {
        assert_eq!(format_clock(9, 5, false), "09:05");
        assert_eq!(format_clock(23, 59, false), "23:59");
    }

    #[test]
    fn formats_12_hour_time() {
        assert_eq!(format_clock(0, 0, true), "12:00 AM");
        assert_eq!(format_clock(12, 0, true), "12:00 PM");
        assert_eq!(format_clock(21, 5, true), "9:05 PM");
    }
}
