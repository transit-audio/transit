//! Facts collected during an editor open. Never query GUI size before `create`.

use super::*;

#[derive(Default)]
pub(super) struct Probe {
    api: String,
    embedded: Option<bool>,
    floating: Option<bool>,
    preferred: Option<String>,
    pub size: Option<(u32, u32)>,
    resizable: Option<bool>,
    timer: Option<bool>,
    fd: Option<bool>,
    scale: Option<f64>,
}

impl Probe {
    pub fn support(&mut self, g: &GuiInner, vt: clap_plugin_gui, api: &CStr, embedded: bool, floating: bool) {
        self.api = api.to_string_lossy().into_owned();
        self.embedded = Some(embedded);
        self.floating = Some(floating);
        // SAFETY: the caller holds the live instance's GUI lock on its main thread. Extension
        // identifiers are static strings; preferred API out-pointers are writable locals and
        // a successful call supplies a static, NUL-terminated API name (CLAP gui.h).
        unsafe {
            if let Some(f) = vt.get_preferred_api {
                let mut preferred = std::ptr::null();
                let mut floating = false;
                if f(g.plugin, &mut preferred, &mut floating) && !preferred.is_null() {
                    self.preferred = Some(format!("{} (floating={floating})", CStr::from_ptr(preferred).to_string_lossy()));
                }
            }
            if let Some(ext) = (*g.plugin).get_extension {
                self.timer = Some(!ext(g.plugin, clap_sys::ext::timer_support::CLAP_EXT_TIMER_SUPPORT.as_ptr()).is_null());
                self.fd = Some(!ext(g.plugin, clap_sys::ext::posix_fd_support::CLAP_EXT_POSIX_FD_SUPPORT.as_ptr()).is_null());
            }
        }
    }

    pub fn created(&mut self, g: &GuiInner, vt: clap_plugin_gui, scale: Option<f64>, floating: bool) {
        self.size = GuiLink::size(g, vt);
        self.scale = scale;
        // SAFETY: live, created GUI, locked on the instance's main thread. CLAP gui.h permits
        // can_resize only for embedded GUIs; floating windows manage their own resizing.
        self.resizable = vt.can_resize.filter(|_| !floating).map(|f| unsafe { f(g.plugin) });
    }

    pub fn log(&self, name: &str) {
        let size = self.size.map_or_else(|| "unknown".into(), |(w, h)| format!("{w} x {h}"));
        log::info!(
            "plug-in editor probe: {name}: format=CLAP api={} embedded={} floating={} preferred_api={} size={size} resizable={} timer-support={} posix-fd-support={} scale={}",
            self.api,
            fact(self.embedded),
            fact(self.floating),
            self.preferred.as_deref().unwrap_or("not offered"),
            fact(self.resizable),
            fact(self.timer),
            fact(self.fd),
            fact(self.scale)
        );
    }
}

fn fact<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "unknown".into(), |v| v.to_string())
}
