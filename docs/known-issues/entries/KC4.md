<!-- transit-index
status: Open, safe
follow-up: Slice 3 if a caller ever needs it: install the callbacks for both directions at the first activation.
order: 77
-->

Native JACK: after one direction is stopped and dropped while the other was opened but never started, starting the remaining one fails ("callbacks are installed once per client"). Safe (no JACK call after activation) but surfaces as an open error; the Player does not do this today.
