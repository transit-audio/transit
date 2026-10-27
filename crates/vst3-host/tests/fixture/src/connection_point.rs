use super::*;

impl IConnectionPointTrait for GainProcessor {
    /// # Safety
    /// other must be null or a live connection point; this component must be initialized.
    unsafe fn connect(&self, other: *mut IConnectionPoint) -> tresult {
        // SAFETY: the SDK passes a live peer (or null, rejected); this call only borrows it.
        let Some(peer) = (unsafe { ComRef::from_raw(other) }) else { return kInvalidArgument };
        // SAFETY: initialize stored a host context that lives until terminate; cast owns an
        // additional reference, released after say_hello returns.
        let host = unsafe { ComRef::from_raw(self.host.load(Ordering::SeqCst)) }.and_then(|h| h.cast::<IHostApplication>());
        let Some(host) = host else { violation("the host context has no IHostApplication") };
        say_hello(&host, &peer);
        kResultOk
    }
    unsafe fn disconnect(&self, _other: *mut IConnectionPoint) -> tresult {
        kResultOk
    }
    unsafe fn notify(&self, _message: *mut IMessage) -> tresult {
        kResultOk
    }
}

impl IConnectionPointTrait for GainController {
    unsafe fn connect(&self, other: *mut IConnectionPoint) -> tresult {
        if other.is_null() { kInvalidArgument } else { kResultOk }
    }
    unsafe fn disconnect(&self, _other: *mut IConnectionPoint) -> tresult {
        kResultOk
    }
    /// # Safety
    /// message must be null or a live IMessage borrowed for this call, with no concurrent mutation.
    unsafe fn notify(&self, message: *mut IMessage) -> tresult {
        // SAFETY: forwards the SDK notify borrow; hello_ok does not retain the message or its data.
        if !unsafe { hello_ok(message) } {
            violation("the component's IMessage reached the controller garbled");
        }
        self.hello.store(true, Ordering::SeqCst);
        kResultOk
    }
}
