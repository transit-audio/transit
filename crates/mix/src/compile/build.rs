use super::*;

impl PlanCompiler {
    pub(super) fn build(
        &mut self,
        s: &Session,
        description: Description,
        make: &mut dyn FnMut(&InstanceSpec, SlotGen) -> Option<NewInstance>,
        need0: usize,
        mode: CompileMode,
    ) -> Result<BuiltPlan, CompileError> {
        let Description { mut desc, order, specs } = description;
        let RenderOrder { rows, phase1, aux_end, .. } = order;
        let block = self.target.block();
        let main_ch = main_channels(s);
        let sr = self.target.sample_rate as f32;
        // Slots first: their instances tell the delay the strips need.
        let mut per_strip: Vec<PlannedStrip> = Vec::new();
        // The factory and the instances run plug-in code that may unwind: report what was made (or
        // taken from the pool) before the unwind drops it, then let it continue.
        let planned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.plan_slots(s, &rows, &specs, make, &mut per_strip, mode)));
        match planned {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                self.release_planned(per_strip);
                return Err(e);
            }
            Err(panic) => {
                self.release_planned(per_strip);
                std::panic::resume_unwind(panic);
            }
        }
        let latency_of = |p: &PlannedSlot, me: &Self| match &p.plan {
            SlotPlan::Move { old, .. } => me.active.slots.get(old).map_or(0, |o| o.raw_latency.load(std::sync::atomic::Ordering::Acquire)),
            SlotPlan::Fresh(_) => p.latency,
            SlotPlan::Absent => 0,
        };
        let mut raw: SlotLatencies = per_strip
            .iter()
            .flat_map(|(_, _, ps, _)| ps.iter())
            .filter(|p| matches!(p.kind, CellKind::Insert | CellKind::Instrument) && !matches!(p.plan, SlotPlan::Absent))
            .map(|p| (p.key, latency_of(p, self)))
            .collect();
        for (track, spec) in &desc.external {
            raw.insert((*track, INSTRUMENT_SLOT), if self.live_input && !spec.src.is_empty() { spec.latency as usize } else { 0 });
        }
        let selection_target = RenderTarget { in_channels: if self.live_input { self.target.in_channels } else { 0 }, ..self.target };
        desc.bypass =
            self.low_latency_limit.map_or_else(|| self.bypass.clone(), |limit| low_latency_bypass(s, &self.io, selection_target, &raw, limit));
        let baseline = crate::low_latency::shared_latency(s, &self.io, selection_target, &raw, &BTreeSet::new());
        let effective = crate::low_latency::shared_latency(s, &self.io, selection_target, &raw, &desc.bypass);
        // Each track's planned instrument and insert latency, then the slowest path (F05).
        let planned: HashMap<TrackId, usize> = per_strip
            .iter()
            .filter_map(|(ti, _, ps, _)| {
                let t = s.tracks.get(*ti)?;
                let l = ps
                    .iter()
                    .filter(|p| {
                        p.kind == CellKind::Instrument
                            || (p.kind == CellKind::Insert
                                && t.mixer
                                    .inserts
                                    .get(p.index as usize)
                                    .and_then(Option::as_ref)
                                    .is_some_and(|ins| ins.active && !ins.bypass && !desc.bypass.contains(&ins.uid.get())))
                    })
                    .fold(t.freeze.as_ref().map_or(0, |f| f.latency as usize), |a, p| a.saturating_add(latency_of(p, self)));
                let hw = crate::hardware::latency_of(t, &self.io, self.target.sample_rate);
                Some((t.id, l.saturating_add(hw).saturating_add(crate::external::latency_of(t, &self.io, self.target.sample_rate))))
            })
            .collect();
        let need1 = path_need(s, &|t: &Track| planned.get(&t.id).copied().unwrap_or(0));
        let need = need0.max(need1);
        if need > need0 {
            // Fresh instances need longer lines than measured before they existed: check again.
            let cap_of = |t: &Track, ch: usize| match self.strip_cap(t, ch, need, true, mode) {
                (_, true) => 0,
                (cap, false) => cap,
            };
            let key_cap = |t: &Track, ch: usize| self.strip_cap(t, ch, need, true, mode).0;
            if let Err(e) = measure(s, &self.limits, block, &cap_of, &key_cap) {
                self.release_planned(per_strip);
                return Err(e);
            }
        }

        // Strips and cells.
        let mut tables = PlanTables {
            low_latency: desc.low_latency,
            saved_latency: baseline.saturating_sub(effective),
            phase1,
            aux_end,
            main_fmt: s.main_format(),
            layers: desc.layers.clone().into_boxed_slice(),
            ..PlanTables::default()
        };
        let mut cells: Vec<CellLoc> = Vec::new();
        let mut moves = Vec::new();
        let mut attach = Vec::new();
        let mut shadow = Shadow { desc: None, ..Shadow::default() };
        let mut strip_moves = Vec::new();
        let mut key_caps: Vec<usize> = Vec::new();
        desc.strips.clear();
        for (si, (ti, layout, _, clips)) in per_strip.iter().enumerate() {
            let Some(t) = s.tracks.get(*ti) else { continue };
            let ch = strip_channels(t, main_ch);
            let ch16 = u16::try_from(ch).unwrap_or(u16::MAX);
            let (cap, keep) = self.strip_cap(t, ch, need, true, mode);
            let mut st = Strip::new();
            st.buf = vec![vec![0.0; block]; ch];
            st.pre = (0..ch).map(|_| Vec::with_capacity(block)).collect();
            st.scratch = vec![0.0; block];
            st.mode_bypass = t.mixer.inserts.iter().map(|ins| ins.as_ref().is_some_and(|ins| desc.bypass.contains(&ins.uid.get()))).collect();
            st.plugins = (0..t.mixer.inserts.len()).map(|_| None).collect();
            st.hw = desc.hardware.iter().filter(|(id, _)| *id == t.id).map(|(_, sp)| crate::hardware::HwSlot::new(sp.clone(), block)).collect();
            st.ext = desc.external.iter().find(|(id, _)| *id == t.id).map(|(_, sp)| crate::external::ExtSlot::new(sp.clone()));
            st.clipbuf = vec![vec![0.0; block]; ch];
            st.sink_routes = desc.sink_routes.get(si).cloned().unwrap_or_default();
            st.key_pre = desc.keys.iter().any(|k| k.pre && k.from == KeyEnd::Strip(si as u16));
            // A line for the strip's own signal before each keyed insert, in case its key arrives late.
            st.key_in = desc.keys.iter().filter(|k| usize::from(k.keyed) == si).map(|k| KeyIn::new(k.slot, ch, cap, block)).collect();
            key_caps.push(cap);
            // A strip that keeps its live lines gets placeholders: the move brings the lines.
            let line = || if keep { DelayLine::placeholder() } else { DelayLine::with_capacity(ch, cap, block) };
            st.tap = line();
            st.tap_pre = line();
            st.outbuf = (0..ch).map(|_| Vec::with_capacity(block)).collect();
            st.track = u16::try_from(*ti).unwrap_or(u16::MAX);
            st.id = t.id;
            st.layout = *layout;
            st.meter = StripMeter::prepared(ch, self.target.sample_rate);
            st.clip_fx = ClipFxTable(clips.iter().map(|(c, _, _)| (*c, ClipFx::new(sr, ch, false))).collect());
            let generation = match self.active.movable_strip(t.id, ch16, *layout).filter(|_| mode == CompileMode::Update) {
                Some((from, old)) => {
                    strip_moves.push(CellMove { kind: CellKind::Strip, from: from as u16, to: si as u16, expect: old.generation });
                    old.generation
                }
                None => match self.fresh_gen() {
                    Ok(g) => {
                        st.generation = g;
                        g
                    }
                    Err(e) => {
                        self.release_planned(per_strip);
                        return Err(e);
                    }
                },
            };
            let region = if si < phase1 {
                0
            } else if si < aux_end {
                1
            } else {
                2
            };
            desc.strips.push((
                t.id,
                st.track,
                region,
                ch16,
                *layout,
                t.mixer.inserts.len(),
                cap,
                clips.iter().map(|(c, e, k)| (*c, e.is_some(), k.is_some())).collect(),
            ));
            cells.push(CellLoc { strip: si as u16, index: 0, shape: Shape { kind: CellKind::Strip, ch: ch16, layout: *layout } });
            shadow.strips.push(ShadowStrip { track: t.id, generation, ch: ch16, layout: *layout, cap });
            tables.strips.push(st);
        }
        tables.keys = KeyBank::new(&desc.keys, block, |keyed| key_caps.get(usize::from(keyed)).copied().unwrap_or(0));
        tables.graph = crate::graph::strip_graph(phase1, aux_end.saturating_sub(phase1), &desc.layers, &desc.keys);
        tables.graph.tail = crate::graph::tail::build(&tables.graph, &desc.tail);
        tables.inputs = crate::graph::snapshots::Snapshots::new(&desc.tail, phase1, block);
        moves.extend(strip_moves);
        for (si, (_, layout, planned, _)) in per_strip.into_iter().enumerate() {
            for p in planned {
                let cell = u16::try_from(cells.len()).unwrap_or(u16::MAX);
                let loc = CellLoc {
                    strip: si as u16,
                    index: p.index,
                    shape: Shape { kind: p.kind, ch: u16::try_from(p.spec.ch).unwrap_or(u16::MAX), layout },
                };
                cells.push(loc);
                let long = if needs_storage(p.params) { p.params } else { 0 };
                let mut sh = ShadowSlot {
                    cell,
                    generation: SlotGen::VACANT,
                    spec: p.spec,
                    kind: p.kind,
                    layout,
                    heap: long,
                    latency: 0,
                    raw_latency: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                };
                match p.plan {
                    SlotPlan::Move { old, from, generation, applied } => {
                        let old = self.active.slots.get(&old);
                        sh.generation = generation;
                        if let Some(o) = old {
                            sh.raw_latency = Arc::clone(&o.raw_latency);
                        }
                        sh.latency = old.map_or(0, |o| o.latency);
                        sh.heap = old.map_or(0, |o| o.heap);
                        let from_move = u16::try_from(moves.len()).ok();
                        moves.push(CellMove { kind: p.kind, from, to: cell, expect: generation });
                        if let Some(n) = applied {
                            attach.push(Attach::Applied { cell, from_move, storage: vec![f32::NAN; n] });
                            sh.heap = n;
                        }
                    }
                    SlotPlan::Fresh(slot) => {
                        sh.generation = slot.generation;
                        sh.raw_latency = Arc::clone(&slot.raw_latency);
                        sh.latency = p.latency;
                        sh.heap = slot.applied.heap.len();
                        if let Some(c) = tables.slot_mut(&loc) {
                            *c = Some(*slot);
                        }
                    }
                    SlotPlan::Absent => {}
                }
                shadow.slots.insert(p.key, sh);
            }
        }
        let mut tracks: Vec<(TrackId, u16)> = tables.strips.iter().enumerate().map(|(i, st)| (st.id, i as u16)).collect();
        tracks.sort_unstable();
        tables.tracks = tracks.into_boxed_slice();
        let mut busses: Vec<(BusId, BusBuf)> =
            s.busses.iter().map(|b| (b.id, BusBuf::new(b.format, vec![vec![0.0; block]; b.format.channels().clamp(1, MAX_CHANNELS)]))).collect();
        busses.sort_by_key(|(id, _)| *id);
        busses.dedup_by_key(|(id, _)| *id);
        tables.busses = BusTable(busses);
        tables.main = vec![vec![0.0; block]; main_ch];
        tables.pre_click = vec![vec![0.0; block]; main_ch];
        tables.sinks = desc.sinks.iter().map(|sp| Sink::new(sp, block)).collect();
        tables.io = Arc::clone(&self.io);
        tables.main_meter = StripMeter::prepared(main_ch, self.target.sample_rate);
        shadow.cells = cells.len();
        tables.cells = cells.into_boxed_slice();
        shadow.tracks = tables.tracks.clone();
        shadow.desc = Some(desc);
        Ok(BuiltPlan { tables, moves, attach, shadow })
    }
}
