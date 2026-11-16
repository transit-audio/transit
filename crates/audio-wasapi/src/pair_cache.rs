//! One cached answer per open: which capture endpoint pairs with a render endpoint. The answer
//! costs two enumerations on two fresh MTA threads (`com::on_mta`), and one open asks it several
//! times (`capture_with_render`, `capture_format`, `paired_input_name`). Control thread only; the
//! audio callbacks never see it. Pure data, so the rule is tested without COM.

use std::sync::{Mutex, PoisonError};

/// The pairing answer of the endpoint most recently asked about.
pub(crate) struct PairCache<T> {
    slot: Mutex<Option<(String, T)>>,
}

impl<T> Default for PairCache<T> {
    fn default() -> Self {
        Self { slot: Mutex::new(None) }
    }
}

impl<T: Clone> PairCache<T> {
    /// The cached answer for `key`, else `compute()` stored in its place. `compute` runs outside the
    /// lock, so a slow enumeration never blocks another caller.
    pub(crate) fn get_or_compute(&self, key: &str, compute: impl FnOnce() -> T) -> T {
        if let Some((cached, value)) = self.slot.lock().unwrap_or_else(PoisonError::into_inner).as_ref()
            && cached == key
        {
            return value.clone();
        }
        let value = compute();
        *self.slot.lock().unwrap_or_else(PoisonError::into_inner) = Some((key.to_owned(), value.clone()));
        value
    }

    /// Forgets the answer: a new open (or a fresh listing) asks the system again.
    pub(crate) fn clear(&self) {
        *self.slot.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn one_computation_per_open_and_a_reopen_computes_again() {
        let cache = PairCache::<u32>::default();
        let runs = Cell::new(0);
        let ask = |key: &str| {
            cache.get_or_compute(key, || {
                runs.set(runs.get() + 1);
                runs.get()
            })
        };
        assert_eq!((ask("out"), ask("out"), ask("out")), (1, 1, 1), "repeated queries of one open compute once");
        assert_eq!(ask("other"), 2, "another endpoint is its own question");
        assert_eq!(ask("out"), 3, "the cache holds one answer: the earlier endpoint is asked again");
        cache.clear();
        assert_eq!((ask("out"), ask("out")), (4, 4), "a reopen computes again, once");
    }
}
