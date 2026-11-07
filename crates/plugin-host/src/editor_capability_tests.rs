//! #289: GUI capability never suppresses controller servicing.
#![allow(clippy::unwrap_used)]

use super::*;
use transit_dsp::{Category, PluginInfo};

static INFO: PluginInfo = PluginInfo {
    id: "vst3:test.no-gui",
    name: "No GUI controller",
    short_name: "Controller",
    category: Category::Other,
    params: &[],
    is_instrument: false,
    audiosuite: false,
    pages: &[],
};
struct Controller(Arc<AtomicUsize>);
impl PluginEditor for Controller {
    fn has_gui(&self) -> bool {
        false
    }
    fn open(&mut self) -> Result<(), String> {
        Err("no gui extension".into())
    }
    fn close(&mut self) {}
    fn is_open(&self) -> bool {
        false
    }
    fn idle(&mut self) -> Vec<(String, f32)> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Vec::new()
    }
}
struct Headless(Arc<AtomicUsize>);
impl Plugin for Headless {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        false
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
    fn editor(&mut self) -> Option<Box<dyn PluginEditor>> {
        Some(Box::new(Controller(Arc::clone(&self.0))))
    }
}
#[test]
fn a_controller_without_a_gui_is_adopted_and_idled_but_its_proxy_has_no_gui() {
    let host = Host::new(Config { inline: false, ..Config::default() });
    let (send, receive) = mpsc::channel();
    let job = host.submit(move |ctx| {
        let calls = Arc::new(AtomicUsize::new(0));
        let proxy = ctx.adopt(Box::new(Headless(Arc::clone(&calls))));
        let before = calls.load(Ordering::SeqCst);
        ctx.idle();
        let after = calls.load(Ordering::SeqCst);
        send.send((proxy, before, after)).unwrap();
    });
    let (mut proxy, before, after) = receive.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(!proxy.editor().unwrap().has_gui());
    assert_eq!((before, after), (0, 1), "the explicit host tick serviced the controller");
    drop(proxy);
    drop(job);
    assert!(host.shutdown(Duration::from_secs(5)));
}
