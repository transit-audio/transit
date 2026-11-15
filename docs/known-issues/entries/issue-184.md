<!-- transit-index
status: Open, cause not found; now reported
follow-up: A loaded recording run on a build with the loss reports names the cause: the Stop message and the log say whether the take writer fell behind or the audio device lost input, and a stretch of exact digital silence between signal that neither explains points at the interface or its driver.
-->

Recording under very heavy load (#184). One run, with two compiles and a Linux gate on the same machine, recorded a 4.7 s stretch of exact digital silence in both armed inputs at the same place, and nothing was logged; it has not recurred in a longer run under heavier load. The cause is not known: either the take writer fell behind until the input queue was full (transit then records the lost input as silence so the take keeps its timing), or the interface delivered zeros while still running. transit now counts every stretch of input it or the audio system replaced with silence, says on the status line while recording and in the log when it begins, and names how long and from where in the take at Stop; separately, the log notes any stretch of exact digital silence of 100 ms or more between signal in a take.
