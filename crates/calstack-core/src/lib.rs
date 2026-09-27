pub mod calendar;
pub mod config;
pub mod feeds;
pub mod layout;
pub mod meeting;
mod zones;

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub title: String,
    pub calendar: String,
    /// Local wall-clock minutes relative to the displayed day's midnight.
    pub start: i32,
    pub end: i32,
    pub meeting: Option<String>,
    pub color: Option<[u8; 3]>,
}

/// Explicit demo mode's static calendar. Times repeat each local day.
/// assets/demo.ics is the equivalent fixture for the ICS feed loader.
pub fn demo_events() -> Vec<Event> {
    [
        ("Early focus", 330, 390, None),
        ("Morning focus", 510, 600, None),
        (
            "Team standup",
            570,
            600,
            Some("https://meet.google.com/abc-defg-hjk"),
        ),
        ("Three-way overlap", 585, 615, None),
        ("Lunch break", 720, 780, None),
        (
            "Design review",
            870,
            915,
            Some("https://us02web.zoom.us/j/84261573920"),
        ),
        (
            "Sprint planning",
            1020,
            1050,
            Some("https://meet.google.com/mnp-qrst-vwx"),
        ),
        ("Evening reading", 1230, 1290, None),
        ("Overnight work", 1410, 1470, None),
        ("Overnight work (from yesterday)", -30, 30, None),
    ]
    .into_iter()
    .map(|(title, start, end, meeting)| Event {
        title: title.into(),
        calendar: "Calstack Demo".into(),
        start,
        end,
        meeting: meeting.map(str::to_owned),
        color: None,
    })
    .collect()
}

pub fn time_label(minutes: i32) -> String {
    let day = minutes.div_euclid(1440);
    let m = minutes.rem_euclid(1440);
    let suffix = match day {
        0 => "",
        1 => " (+1 day)",
        -1 => " (-1 day)",
        _ => "",
    };
    format!("{:02}:{:02}{suffix}", m / 60, m % 60)
}
