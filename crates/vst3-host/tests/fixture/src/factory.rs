use super::*;

impl IPluginFactoryTrait for Factory {
    unsafe fn getFactoryInfo(&self, info: *mut PFactoryInfo) -> tresult {
        let i = unsafe { &mut *info };
        copy_c8("SoundCraft tests", &mut i.vendor);
        copy_c8("https://example.invalid", &mut i.url);
        copy_c8("tests@example.invalid", &mut i.email);
        i.flags = PFactoryInfo_::FactoryFlags_::kUnicode as i32;
        kResultOk
    }
    unsafe fn countClasses(&self) -> i32 {
        CLASSES.len() as i32
    }
    unsafe fn getClassInfo(&self, index: i32, info: *mut PClassInfo) -> tresult {
        let Some(&(cid, cat, name, _)) = CLASSES.get(index as usize) else { return kInvalidArgument };
        let i = unsafe { &mut *info };
        i.cid = cid;
        i.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as i32;
        copy_c8(cat, &mut i.category);
        copy_c8(name, &mut i.name);
        kResultOk
    }
    unsafe fn createInstance(&self, cid: FIDString, iid: FIDString, obj: *mut *mut c_void) -> tresult {
        set_up_late_state();
        let cid = unsafe { *(cid as *const TUID) };
        // A heavy library: the synth takes half a second to make (the other classes do not), or
        // `VST3_FIXTURE_CREATE_MS` milliseconds.
        #[cfg(feature = "slow-create")]
        if cid == SYNTH_CID {
            let ms = std::env::var("VST3_FIXTURE_CREATE_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(500);
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
        let unknown = if cid == GAIN_CID {
            ComWrapper::new(GainProcessor {
                gain: AtomicU64::new(0.25f64.to_bits()),
                arrangement: AtomicU64::new(SpeakerArr::kStereo),
                host: AtomicPtr::new(std::ptr::null_mut()),
                _live: Live::new(&OBJECTS),
            })
            .to_com_ptr::<FUnknown>()
        } else if cid == GAIN_CTRL_CID {
            ComWrapper::new(GainController {
                gain: AtomicU64::new(0.5f64.to_bits()),
                freq: AtomicU64::new(0.5f64.to_bits()),
                preset: AtomicU64::new(u64::MAX),
                handler: AtomicPtr::new(std::ptr::null_mut()),
                hello: AtomicBool::new(false),
                _live: Live::new(&OBJECTS),
            })
            .to_com_ptr::<FUnknown>()
        } else if cid == SYNTH_CID {
            ComWrapper::new(Synth::new()).to_com_ptr::<FUnknown>()
        } else if cid == KEYED_CID {
            ComWrapper::new(Keyed::new(false)).to_com_ptr::<FUnknown>()
        } else if cid == PICKY_CID {
            ComWrapper::new(Keyed::new(true)).to_com_ptr::<FUnknown>()
        } else if cid == EVENTS_CID {
            ComWrapper::new(Synth { events: true, ..Synth::new() }).to_com_ptr::<FUnknown>()
        } else {
            None
        };
        match unknown {
            Some(u) => {
                let p = u.as_ptr();
                unsafe { ((*(*p).vtbl).queryInterface)(p, iid as *const TUID, obj) }
            }
            None => kInvalidArgument,
        }
    }
}

impl IPluginFactory2Trait for Factory {
    unsafe fn getClassInfo2(&self, index: i32, info: *mut PClassInfo2) -> tresult {
        let Some(&(cid, cat, name, subs)) = CLASSES.get(index as usize) else { return kInvalidArgument };
        let i = unsafe { &mut *info };
        i.cid = cid;
        i.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as i32;
        copy_c8(cat, &mut i.category);
        copy_c8(name, &mut i.name);
        i.classFlags = 0;
        copy_c8(subs, &mut i.subCategories);
        copy_c8("", &mut i.vendor);
        copy_c8("1.2.3", &mut i.version);
        copy_c8("VST 3.8.0", &mut i.sdkVersion);
        kResultOk
    }
}

