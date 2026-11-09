<!-- transit-index
status: Open, intended break
follow-up: Transpose an old session's pad clips down 24 semitones, or set the pads' notes explicitly; ADR 0041's amendment of 2026-11-09 records the decision and no migration is planned while the project is in alpha.
-->

Sampler pad trigger notes now default to 36 plus the pad index (36 to 99) instead of 60 plus the index (60 to 123), following the drum machine convention ([ADR 0041](../../adr/0041-sampler-one-instrument-three-views.md)). A session or kit saved by an earlier build opens with its default pads on the new notes, so MIDI clips and patterns written against 60 to 123 no longer hit those pads. Pads whose note was set explicitly are unaffected.
