# College Cal

A small Rust desktop dashboard for an ICS class calendar. It shows the class in
progress, a live time-remaining bar, and the next class.

## Run

```sh
cargo run --release
```

Paste an `https://` or `webcal://` address into **Calendar source**, or select
**Browse…** to choose a local `.ics` file. The value is saved locally by the app
and refreshed every 15 minutes.

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
