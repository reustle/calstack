# Demo calendar

`demo.ics` is one fictional calendar with ten daily recurring events starting
September 25, 2026. Timed events use floating local times so the demo follows
the system timezone (currently Asia/Tokyo).

It covers morning and evening events, a three-way overlap at 09:45, clipping
at 06:00 and midnight, and an all-day event that should be hidden by default.
Google Meet and Zoom links with fictional meeting IDs appear in URL, location,
and description fields. These are not working meeting invitations;
opening one can reach the provider's website.

For the initial static prototype, use these events as the demo schedule.
Loading this file itself requires the local ICS and daily recurrence milestones.

Once config loading and ICS support exist, add this feed using the schema from
PLAN.md section 22 (replace the path if the repository moves):

```toml
[[calendar.feeds]]
name = "Calstack Demo"
path = "/home/reustle/sync/projects/calstack/assets/demo.ics"
color = "#7F9BB3"
enabled = true
```

The file is a development fixture, not an installed or subscribed calendar.
