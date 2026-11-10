<!-- transit-index
status: Open, stopgap
follow-up: A zone before bar 1 (#441) gives count-in notes room at the session start; it removes this rule.
-->

MIDI count-in notes at the session start (#440). Notes played during the count-in (pickups, #327) are placed where they were played, so they need room before the record start. Recording from the very start of the session there is none: a note released during the count-in, at or before the session start, is left out of the take, and the status line says how many were left out; a note still held when recording starts begins at the session start with the rest of its length. Journal recovery of such a take leaves out the same notes. Controllers and other non-note events played during the count-in are kept as before and land on the first tick, in the order they were played.
