---
type: changed
---
From 1.0 on, `jp release cut --version` refuses a minor release while an
unreleased fragment has an upgrade section, as `pick` already does: only a major
release ships a change that requires users to act.
