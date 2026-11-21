<!-- transit-index
status: Open (#442)
follow-up: Audio played during the count-in is kept in the take like MIDI pickups once audio clips can start before bar 1; a decision record comes first.
-->

Audio clips still start at bar 1 (#442). An audio take that starts in the pre-count zone keeps that audio in its file, before the clip's start, and the status line says so; it is not placed before bar 1 as a MIDI take is. Bounce in Place of a take with count-in pickups therefore prints from bar 1.
