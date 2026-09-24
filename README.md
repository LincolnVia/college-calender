# College Cal

A small Rust desktop dashboard for ICS class calendars. It shows the class in
progress, a live time-remaining bar, the next class, and a weekly schedule.

## Run

```sh
cargo run --release
```

Select **Calendar** to add one or more calendars. Enter a calendar name first,
then use **Browse…** for a local `.ics` file or paste an `https://` or `webcal://`
address. The sources, names, and main-calendar choice are saved locally as soon
as they change and the calendars are refreshed every 15 minutes.

Any `.ics` or `.ical` files placed in the project's `ics/` directory are added
automatically at startup and whenever the calendars refresh.

Use the **Main calendar** dropdown on the Dashboard tab to choose which named
calendar supplies its current class, next class, and weekly schedule.

The **Compare timeline** tab shows the current week's time ranges where at least
one named calendar has an event while another calendar is free.

You can also provide the source on first launch:

```sh
COLLEGE_CAL_ICS="https://example.edu/my-calendar.ics" cargo run --release
```

Calendar URLs are effectively private credentials. Avoid committing yours to
source control or sharing it publicly.

## macOS application

The packaged application is created at `dist/College Cal.app`. It is built for
the Mac architecture that ran the release build and uses an ad-hoc local
signature.
