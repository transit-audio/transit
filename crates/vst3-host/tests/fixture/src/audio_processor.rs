use super::*;

impl IAudioProcessorTrait for GainProcessor {
    unsafe fn setBusArrangements(&self, inputs: *mut SpeakerArrangement, n_in: i32, outputs: *mut SpeakerArrangement, n_out: i32) -> tresult {
        if n_in != 1 || n_out != 1 {
            return kResultFalse;
        }
        let (i, o) = unsafe { (*inputs, *outputs) };
        if i != o || (i != SpeakerArr::kStereo && i != SpeakerArr::kMono) {
            return kResultFalse;
        }
        self.arrangement.store(i, Ordering::Relaxed);
        kResultTrue
    }
    unsafe fn getBusArrangement(&self, _dir: BusDirection, index: i32, arr: *mut SpeakerArrangement) -> tresult {
        if index != 0 {
            return kInvalidArgument;
        }
        unsafe { *arr = self.arrangement.load(Ordering::Relaxed) };
        kResultOk
    }
    unsafe fn canProcessSampleSize(&self, size: i32) -> tresult {
        if size == SymbolicSampleSizes_::kSample32 as i32 { kResultOk } else { kNotImplemented }
    }
    unsafe fn getLatencySamples(&self) -> u32 {
        3
    }
    unsafe fn setupProcessing(&self, setup: *mut ProcessSetup) -> tresult {
        let s = unsafe { &*setup };
        assert_eq!(s.symbolicSampleSize, SymbolicSampleSizes_::kSample32 as i32);
        assert!(s.maxSamplesPerBlock > 0 && s.sampleRate > 0.0);
        kResultOk
    }
    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        kNotImplemented
    }
    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        if f64::from_bits(self.gain.load(Ordering::Relaxed)) == -3.0 {
            return kInternalError;
        }
        let d = unsafe { &*data };
        assert!(!d.processContext.is_null());
        let probes = probes();
        if probes {
            note_process_thread();
        }
        if let Some(changes) = unsafe { ComRef::from_raw(d.inputParameterChanges) } {
            for i in 0..unsafe { changes.getParameterCount() } {
                let Some(q) = (unsafe { ComRef::from_raw(changes.getParameterData(i)) }) else { continue };
                let (mut off, mut v) = (0, 0.0);
                let n = unsafe { q.getPointCount() };
                let id = unsafe { q.getParameterId() };
                if n > 0 && unsafe { q.getPoint(n - 1, &mut off, &mut v) } == kResultOk {
                    if id == 7 {
                        self.gain.store(v.to_bits(), Ordering::Relaxed);
                    } else if id == 12 {
                        LAST_FREQ.store(v.to_bits(), Ordering::Relaxed);
                    } else if id == FINE {
                        LAST_FINE.store(v.to_bits(), Ordering::Relaxed);
                    }
                }
            }
        }
        if d.numInputs != 1 || d.numOutputs != 1 {
            return kResultOk;
        }
        let frames = d.numSamples.max(0) as usize;
        let g = 2.0 * f64::from_bits(self.gain.load(Ordering::Relaxed)) as f32;
        let (ins, outs) = unsafe { (channels(&*d.inputs, frames), channels(&*d.outputs, frames)) };
        for (c, (i, o)) in ins.iter().zip(outs).enumerate() {
            if probes && c == 1 {
                if many() > 0 {
                    // See `many`: controller calls so far, then the last Fine point.
                    o.fill(0.0);
                    if let [a, b, ..] = o {
                        *a = CTRL_CALLS.load(Ordering::Relaxed) as f32;
                        *b = f64::from_bits(LAST_FINE.load(Ordering::Relaxed)) as f32;
                    }
                } else {
                    o.fill(f64::from_bits(LAST_FREQ.load(Ordering::Relaxed)) as f32);
                }
                continue;
            }
            for (x, y) in i.iter().zip(o.iter_mut()) {
                *y = g * x;
            }
        }
        kResultOk
    }
    unsafe fn getTailSamples(&self) -> u32 {
        0
    }
}

impl IAudioProcessorTrait for Keyed {
    unsafe fn setBusArrangements(&self, inputs: *mut SpeakerArrangement, n_in: i32, outputs: *mut SpeakerArrangement, n_out: i32) -> tresult {
        if self.picky || n_in != 2 || n_out != 1 {
            return kResultFalse;
        }
        // SAFETY: the host passes n_in (2) input and n_out (1) output arrangements, checked above.
        let (i0, i1, o0) = unsafe { (*inputs, *inputs.add(1), *outputs) };
        let ok = |a: SpeakerArrangement| a == SpeakerArr::kStereo || a == SpeakerArr::kMono;
        if i0 != o0 || !ok(i0) || !ok(i1) {
            return kResultFalse;
        }
        self.main.store(i0, Ordering::Relaxed);
        self.aux.store(i1, Ordering::Relaxed);
        kResultTrue
    }
    unsafe fn getBusArrangement(&self, dir: BusDirection, index: i32, arr: *mut SpeakerArrangement) -> tresult {
        let input = dir == BusDirections_::kInput as BusDirection;
        if index < 0 || index > i32::from(input) {
            return kInvalidArgument;
        }
        let v = if input && index == 1 { self.aux.load(Ordering::Relaxed) } else { self.main.load(Ordering::Relaxed) };
        // SAFETY: the host passes a writable SpeakerArrangement.
        unsafe { *arr = v };
        kResultOk
    }
    unsafe fn canProcessSampleSize(&self, size: i32) -> tresult {
        if size == SymbolicSampleSizes_::kSample32 as i32 { kResultOk } else { kNotImplemented }
    }
    unsafe fn getLatencySamples(&self) -> u32 {
        0
    }
    unsafe fn setupProcessing(&self, _setup: *mut ProcessSetup) -> tresult {
        kResultOk
    }
    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        kNotImplemented
    }
    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        // SAFETY: the host passes a valid ProcessData for this call.
        let d = unsafe { &*data };
        if d.numInputs != 2 || d.numOutputs != 1 {
            return kResultFalse;
        }
        let frames = d.numSamples.max(0) as usize;
        // SAFETY: two input and one output bus (checked above), each with numChannels channel
        // pointers of `frames` valid samples, owned by the host for this call.
        let (main, aux, outs) = unsafe { (channels(&*d.inputs, frames), channels(&*d.inputs.add(1), frames), channels(&*d.outputs, frames)) };
        // SAFETY: as above; only the flags word is read.
        let flags = unsafe { (*d.inputs.add(1)).silenceFlags };
        let active = self.aux_active.load(Ordering::SeqCst);
        for (c, (i, o)) in main.iter().zip(outs).enumerate() {
            if !active || aux.is_empty() {
                o.copy_from_slice(i);
            } else if flags & (1 << c.min(aux.len() - 1)) != 0 {
                o.fill(KEY_MARK);
            } else {
                let k = &aux[c.min(aux.len() - 1)];
                let rms = (k.iter().map(|v| v * v).sum::<f32>() / frames.max(1) as f32).sqrt();
                for (y, x) in o.iter_mut().zip(i.iter()) {
                    *y = x * rms;
                }
            }
        }
        kResultOk
    }
    unsafe fn getTailSamples(&self) -> u32 {
        0
    }
}

impl IAudioProcessorTrait for Synth {
    unsafe fn setBusArrangements(&self, _i: *mut SpeakerArrangement, n_in: i32, outputs: *mut SpeakerArrangement, n_out: i32) -> tresult {
        if n_in == 0 && n_out == 1 && unsafe { *outputs } == SpeakerArr::kStereo { kResultTrue } else { kResultFalse }
    }
    unsafe fn getBusArrangement(&self, dir: BusDirection, index: i32, arr: *mut SpeakerArrangement) -> tresult {
        if dir != BusDirections_::kOutput as BusDirection || index != 0 {
            return kInvalidArgument;
        }
        unsafe { *arr = SpeakerArr::kStereo };
        kResultOk
    }
    unsafe fn canProcessSampleSize(&self, size: i32) -> tresult {
        if size == SymbolicSampleSizes_::kSample32 as i32 { kResultOk } else { kNotImplemented }
    }
    unsafe fn getLatencySamples(&self) -> u32 {
        self.note_thread("latency");
        self.latency.load(Ordering::SeqCst)
    }
    unsafe fn setupProcessing(&self, _setup: *mut ProcessSetup) -> tresult {
        kResultOk
    }
    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        kResultOk
    }
    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        self.note_thread("process");
        let d = unsafe { &*data };
        if d.numOutputs != 1 {
            return kResultOk;
        }
        let frames = d.numSamples.max(0) as usize;
        #[deny(clippy::undocumented_unsafe_blocks)]
        if self.events {
            // Allocation-free, so hosts can measure their own audio path around it.
            // SAFETY: IAudioProcessor::process supplies the one output bus checked above.
            let bus = unsafe { &*d.outputs };
            let n = bus.numChannels.max(0) as usize;
            // SAFETY: The SDK bus has n channel pointers; this fixture negotiates kSample32.
            let table = unsafe { std::slice::from_raw_parts(bus.__field0.channelBuffers32, n) };
            let Some(&first) = table.first() else { return kResultOk };
            // SAFETY: The SDK provides frames writable f32 samples on the first output channel.
            let out = unsafe { std::slice::from_raw_parts_mut(first, frames) };
            out.fill(0.0);
            let mut w = 0;
            let mut put = |r: [f32; 4]| {
                for x in r {
                    if let Some(o) = out.get_mut(w) {
                        *o = x;
                    }
                    w += 1;
                }
            };
            // SAFETY: inputEvents is null or a borrowed IEventList valid for this process call.
            if let Some(list) = unsafe { ComRef::from_raw(d.inputEvents) } {
                // SAFETY: list is the live SDK interface borrowed above.
                for i in 0..unsafe { list.getEventCount() } {
                    // SAFETY: SDK Event contains C scalars and a union, all valid as zero bits; getEvent fills its tagged payload.
                    let mut e: Event = unsafe { std::mem::zeroed() };
                    // SAFETY: i is within the SDK list and e is writable Event storage.
                    if unsafe { list.getEvent(i, &mut e) } != kResultOk {
                        continue;
                    }
                    let ty = e.r#type;
                    let (code, ch, pitch, id) = if ty == Event_::EventTypes_::kNoteOnEvent as u16 {
                        // SAFETY: The successful SDK event read has the kNoteOnEvent union tag.
                        let n = unsafe { e.__field0.noteOn };
                        (1.0, n.channel, n.pitch, n.noteId)
                    } else if ty == Event_::EventTypes_::kNoteOffEvent as u16 {
                        // SAFETY: The successful SDK event read has the kNoteOffEvent union tag.
                        let n = unsafe { e.__field0.noteOff };
                        (2.0, n.channel, n.pitch, n.noteId)
                    } else if ty == Event_::EventTypes_::kPolyPressureEvent as u16 {
                        // SAFETY: The successful SDK event read has the kPolyPressureEvent union tag.
                        let n = unsafe { e.__field0.polyPressure };
                        (3.0, n.channel, n.pitch, n.noteId)
                    } else {
                        continue;
                    };
                    put([code, e.sampleOffset as f32, f32::from(ch) * 1000.0 + f32::from(pitch), id as f32]);
                }
            }
            // SAFETY: inputParameterChanges is null or a borrowed SDK interface valid in process.
            if let Some(changes) = unsafe { ComRef::from_raw(d.inputParameterChanges) } {
                // SAFETY: changes is the live IParameterChanges interface borrowed above.
                for i in 0..unsafe { changes.getParameterCount() } {
                    // SAFETY: i is in the SDK changes list; its queue remains borrowed for process.
                    let Some(q) = (unsafe { ComRef::from_raw(changes.getParameterData(i)) }) else { continue };
                    // SAFETY: q is the live IParamValueQueue returned by the SDK interface.
                    let pid = unsafe { q.getParameterId() };
                    // SAFETY: q remains valid for this process call.
                    for k in 0..unsafe { q.getPointCount() } {
                        let (mut at, mut v) = (0i32, 0f64);
                        // SAFETY: k is in the SDK queue; at and v are writable output values.
                        if unsafe { q.getPoint(k, &mut at, &mut v) } == kResultOk {
                            put([4.0, at as f32, pid as f32, (v * 1e6).round() as f32]);
                        }
                    }
                }
            }
            // What the host told the editor's side since the last block.
            let ctrl = self.probe_ctrl.load(Ordering::SeqCst);
            if self.probe_seen.swap(ctrl, Ordering::SeqCst) != ctrl {
                put([5.0, 0.0, PROBE_PARAM as f32, (f64::from_bits(ctrl) * 1e6).round() as f32]);
            }
            for &p in table.iter().skip(1) {
                // SAFETY: This SDK channel has frames writable samples distinct from the first channel (no input buses).
                let o = unsafe { std::slice::from_raw_parts_mut(p, frames) };
                // SAFETY: The first SDK output channel remains live and has frames initialized samples.
                o.copy_from_slice(unsafe { std::slice::from_raw_parts(first, frames) });
            }
            return kResultOk;
        }
        let mut events = Vec::new();
        if let Some(list) = unsafe { ComRef::from_raw(d.inputEvents) } {
            for i in 0..unsafe { list.getEventCount() } {
                let mut e: Event = unsafe { std::mem::zeroed() };
                if unsafe { list.getEvent(i, &mut e) } == kResultOk {
                    events.push(e);
                }
            }
        }
        let mut volume = None;
        if let Some(changes) = unsafe { ComRef::from_raw(d.inputParameterChanges) } {
            for i in 0..unsafe { changes.getParameterCount() } {
                let Some(q) = (unsafe { ComRef::from_raw(changes.getParameterData(i)) }) else { continue };
                let (mut at, mut v) = (0i32, 0f64);
                if unsafe { q.getParameterId() } == VOLUME_PARAM && unsafe { q.getPoint(0, &mut at, &mut v) } == kResultOk {
                    volume = Some((at.max(0) as usize, v as f32));
                }
            }
        }
        let outs = unsafe { channels(&*d.outputs, frames) };
        let mut level = f32::from_bits(self.level.load(Ordering::Relaxed));
        let mut out = vec![0.0f32; frames];
        for (i, o) in out.iter_mut().enumerate() {
            for e in events.iter().filter(|e| e.sampleOffset as usize == i) {
                if e.r#type == Event_::EventTypes_::kNoteOnEvent as u16 {
                    level = unsafe { e.__field0.noteOn.velocity };
                } else if e.r#type == Event_::EventTypes_::kNoteOffEvent as u16 {
                    level = 0.0;
                } else if e.r#type == Event_::EventTypes_::kPolyPressureEvent as u16 {
                    level = unsafe { e.__field0.polyPressure.pressure };
                }
            }
            if let Some((at, v)) = volume
                && at == i
            {
                level = v;
            }
            *o = level;
        }
        self.level.store(level.to_bits(), Ordering::Relaxed);
        for ch in outs {
            ch.copy_from_slice(&out);
        }
        kResultOk
    }
    unsafe fn getTailSamples(&self) -> u32 {
        0
    }
}
