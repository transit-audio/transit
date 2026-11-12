<!-- transit-index
status: Open
follow-up: Clear the published voice positions when rendering stops; today they are cleared only when a Sampler window opens.
-->

Slices playhead after rendering stops (#470). The Slices view draws a playhead line at each sounding voice from the positions the audio render last published. If audio rendering stops while a slice is sounding (for example after an audio device error), the last playhead line stays drawn until rendering resumes or the Sampler window is reopened.
