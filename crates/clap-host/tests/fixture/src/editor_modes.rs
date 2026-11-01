//! Commands for editor diagnostics. `no_gui` configures the next instance at init.

use super::*;
use clap_sys::ext::timer_support::*;

thread_local! {
    pub static NEXT_NO_GUI: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    NoGui,
    NoX11,
    Neither,
    CreateFalse,
    SetParentFalse,
    ShowFalse,
    ZeroSize,
    TimerRequired,
}

impl Mode {
    pub fn parse(command: &str) -> Option<Self> {
        Some(match command {
            "no_gui" => Self::NoGui,
            "no_x11" => Self::NoX11,
            "neither" => Self::Neither,
            "create_false" => Self::CreateFalse,
            "set_parent_false" => Self::SetParentFalse,
            "show_false" => Self::ShowFalse,
            "zero_size" => Self::ZeroSize,
            "timer_required" => Self::TimerRequired,
            _ => return None,
        })
    }
}

pub fn register_timer(f: &mut Fx) -> bool {
    // SAFETY: the fixture's host pointer and extension vtable remain valid until destroy;
    // this runs from gui.create on the instance's main thread; id is a writable local.
    unsafe {
        let host = &*f.host;
        let ext = (host.get_extension.unwrap())(f.host, CLAP_EXT_TIMER_SUPPORT.as_ptr());
        let Some(timer) = ext.cast::<clap_host_timer_support>().as_ref() else { return false };
        let mut id = 0;
        if !(timer.register_timer.unwrap())(f.host, 10, &mut id) {
            return false;
        }
        f.gui_timer = Some(id);
    }
    true
}

pub fn unregister_timer(f: &mut Fx) {
    if let Some(id) = f.gui_timer.take() {
        // SAFETY: same live host as at registration; gui.destroy runs on its main thread.
        unsafe {
            let host = &*f.host;
            let ext = (host.get_extension.unwrap())(f.host, CLAP_EXT_TIMER_SUPPORT.as_ptr());
            let timer = &*ext.cast::<clap_host_timer_support>();
            assert!((timer.unregister_timer.unwrap())(f.host, id));
        }
    }
}

unsafe extern "C" fn on_timer(p: *const clap_plugin, id: clap_id) {
    // SAFETY: the host calls with this live instance on its main thread.
    let f = unsafe { fx(p) };
    assert_eq!(f.gui_timer, Some(id));
    // A testable paint readout: the fixture has no renderer or real child window.
    f.gui_painted = f.gui_visible;
}

pub static TIMER: clap_plugin_timer_support = clap_plugin_timer_support { on_timer: Some(on_timer) };
