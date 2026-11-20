<!-- transit-index
status: Closed (#431)
follow-up: Done (ADR 0015, amendment #431, rule 6): transit never explicitly disconnects a link; a reopen reads the links other clients made to transit's ports before the old client closes and makes them again on the new one, under the name the server gave it. Not restored, and said in the status line and Audio Devices: links on a side whose device changed, links on a port whose device channel changed or that the new device lacks, links to a name with no audio port of that direction now, links to a port whose name is not valid UTF-8, and every link when the server stopped during the reopen. Limits: a server restart wholly between the old client's close and the new one's open is not seen, and a link changed in the instant of the reopen may be missed.
-->

Native JACK: every reopen (a server buffer size or rate change, Reconnect, Apply) closed transit's JACK client, and with it every link another program or the user had made to transit's ports. The new ports got only transit's own links, so a hand-made patch into `transit:in_1` was lost without a word.
