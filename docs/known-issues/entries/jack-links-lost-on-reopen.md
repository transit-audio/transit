<!-- transit-index
status: Closed (#431)
follow-up: Done (ADR 0015, amendment #431, rule 6): a reopen reads the links other clients made to transit's ports before the old client closes and makes them again on the new one, under the name the server gave it. Not restored, and said in the status line and Audio Devices: links on a side whose device changed, links on a port the new device lacks, links to a port that went away. Nothing is restored after a server shutdown, and a link changed in the instant of the reopen may be missed.
-->

Native JACK: every reopen (a server buffer size or rate change, Reconnect, Apply) closed transit's JACK client, and with it every link another program or the user had made to transit's ports. The new ports got only transit's own links, so a hand-made patch into `transit:in_1` was lost without a word.
