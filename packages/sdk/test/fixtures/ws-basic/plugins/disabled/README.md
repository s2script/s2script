`plugins/disabled/` matches the `plugins/*` glob but carries no package.json, so §4.3 rule 1
skips it silently. Operators move a built `.s2sp` up one level to enable it.
